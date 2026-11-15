//! An A2UI surface painted over AG-UI.
//!
//! `ag-ui-a2ui` is transport-agnostic, and `adapters::agui` is the one place in
//! it that knows what AG-UI is. Here an agent paints a surface with the
//! adapter, and the client has to receive what an AG-UI frontend paints from:
//! the tool call, its result, and the `a2ui-surface` activity, intact enough
//! that the surface still validates against the catalog it was authored for.
//!
//! It is also where the two views of a conversation meet. A client holds the
//! tool results and the activities; a server is sent the tool results alone.
//! Both have to read the same surfaces.

mod common;

use std::sync::{Arc, Mutex};

use ag_ui::client::{Thread, Update};
use ag_ui::server::{Agent, Error, Result, RunContext};
use ag_ui::{Message, RunOutcome};
use ag_ui_a2ui::adapters::agui::{
    Attempt, Intent, RECOVERY_EXHAUSTED_CODE, RenderArgs, SURFACE_ACTIVITY_TYPE, SurfacePainter,
    SurfaceSpec, SurfaceStatus, assemble_ops, is_operations_envelope, operations_in,
    recovery_exhausted_envelope, surface_activity_id, surfaces_in,
};
use ag_ui_a2ui::catalog::Catalog;
use ag_ui_a2ui::constants::{
    A2UI_OPERATIONS_KEY, BASIC_CATALOG_ID, GENERATE_A2UI_TOOL_NAME, PROTOCOL_VERSION,
    RENDER_A2UI_TOOL_NAME,
};
use ag_ui_a2ui::message::{AgentMessage, AgentPayload, Component};
use ag_ui_a2ui::validate::Validator;
use ag_ui_a2ui::{SurfaceState, SurfaceStore};
use common::{serve, transport};
use futures_util::StreamExt as _;
use serde_json::{Value, json};

const SURFACE_ID: &str = "cart";

/// The surface the agent authors: a heading bound to the data model and a
/// templated list over it.
fn spec(items: &[&str]) -> SurfaceSpec {
    SurfaceSpec::new(SURFACE_ID)
        .with_components(vec![
            Component::new("root", "Column").with("children", json!(["heading", "items"])),
            Component::new("heading", "Text")
                .with("text", json!({"path": "/title"}))
                .with("variant", json!("h2")),
            Component::new("items", "List")
                .with("children", json!({"componentId": "row", "path": "/items"})),
            Component::new("row", "Text").with(
                "text",
                json!({"call": "formatString", "args": {"value": "${@index(offset: 1)}. ${name}"}}),
            ),
        ])
        .with_data_model(json!({
            "title": "Your cart",
            "items": items.iter().map(|name| json!({"name": name})).collect::<Vec<_>>(),
        }))
}

const FIRST: [&str; 2] = ["Espresso", "Croissant"];
const SECOND: [&str; 3] = ["Espresso", "Croissant", "Baguette"];

/// Paints the cart: creates it the first time the conversation shows none,
/// updates it afterwards. Keeps the messages each request carried, so a test
/// can read what a server is sent.
#[derive(Default)]
struct Merchant {
    requests: Arc<Mutex<Vec<Vec<Message>>>>,
}

impl Agent for Merchant {
    type State = ();

    async fn run(&self, ctx: &mut RunContext<()>) -> Result<RunOutcome> {
        self.requests
            .lock()
            .expect("the request log")
            .push(ctx.messages().to_vec());

        // The agent keeps nothing: what the frontend holds is in the request.
        let mut painter = SurfacePainter::from_history(ctx.messages()).map_err(Error::agent)?;
        if painter.surfaces().get(SURFACE_ID).is_none() {
            painter.paint(ctx, &assemble_ops(Intent::Create, &spec(&FIRST)))?;
            ctx.say("Here is your cart.")?;
        } else {
            painter.paint(ctx, &assemble_ops(Intent::Update, &spec(&SECOND)))?;
            ctx.say("Added to your cart.")?;
        }
        Ok(RunOutcome::Success)
    }
}

/// Fails to author a surface, and reports the failure the way a generation
/// that used up its attempts does: on the tool result and on the activity.
struct Butterfingers;

impl Agent for Butterfingers {
    type State = ();

    async fn run(&self, ctx: &mut RunContext<()>) -> Result<RunOutcome> {
        let broken = vec![Component::new("root", "Card").with("child", json!("gone"))];
        let report = Validator::new(&Catalog::basic()).validate(&broken);
        assert!(!report.is_valid(), "this surface is meant to be invalid");
        let attempts = vec![Attempt::failed(1, report.errors)];
        let envelope = recovery_exhausted_envelope(&attempts).map_err(Error::agent)?;

        // The painter that would have painted the surface shows where it is:
        // a skeleton while it is made, and the failure in the skeleton's place.
        let mut painter = SurfacePainter::from_history(ctx.messages()).map_err(Error::agent)?;
        let id = ctx.new_tool_call_id();
        ctx.emit(painter.status(id.as_str(), &SurfaceStatus::Building))?;
        let mut call = ctx.tool_call_with_id(id.clone(), GENERATE_A2UI_TOOL_NAME)?;
        call.args_json(&json!({"intent": "create"}))?;
        call.result(envelope)?;
        ctx.emit(painter.status(
            id.as_str(),
            &SurfaceStatus::Failed {
                error: "Failed to generate valid A2UI after 1 attempt(s)".into(),
                attempts,
                max_attempts: 1,
            },
        ))?;
        Ok(RunOutcome::Success)
    }
}

/// Shows a skeleton for each of two tool calls in one run, and paints the cart
/// with the first and an update of it with the second.
struct Sketcher;

impl Sketcher {
    /// One tool call: a skeleton, then the batch as the call's result and the
    /// surface in the skeleton's place.
    fn call(
        ctx: &mut RunContext<()>,
        painter: &mut SurfacePainter,
        operations: &[AgentMessage],
    ) -> Result<()> {
        let id = ctx.new_tool_call_id();
        ctx.emit(painter.status(id.as_str(), &SurfaceStatus::Building))?;
        let painted = painter
            .settle(id.as_str(), operations)
            .map_err(Error::agent)?;
        let mut call = ctx.tool_call_with_id(id, GENERATE_A2UI_TOOL_NAME)?;
        call.args_json(&json!({"intent": "create"}))?;
        call.result(painted.envelope)?;
        for activity in painted.activities {
            ctx.emit(activity)?;
        }
        Ok(())
    }
}

impl Agent for Sketcher {
    type State = ();

    async fn run(&self, ctx: &mut RunContext<()>) -> Result<RunOutcome> {
        let mut painter = SurfacePainter::from_history(ctx.messages()).map_err(Error::agent)?;
        Self::call(
            ctx,
            &mut painter,
            &assemble_ops(Intent::Create, &spec(&FIRST)),
        )?;
        Self::call(
            ctx,
            &mut painter,
            &assemble_ops(Intent::Update, &spec(&SECOND)),
        )?;
        Ok(RunOutcome::Success)
    }
}

/// Sends one user message and reads the run to its end.
async fn turn<T: ag_ui::client::transport::Transport>(thread: &mut Thread<T>, text: &str) {
    let mut run = thread.send(text).expect("run preflight");
    while let Some(update) = run.next().await {
        if let Update::Error(error) = update {
            panic!("painting a surface should not error: {error}");
        }
    }
}

/// Runs one turn and returns the conversation the client holds after it.
async fn ship(agent: impl Agent + 'static) -> Vec<Message> {
    let url = serve(agent).await;
    let mut thread = Thread::<_>::new(transport(&url), "cart");
    turn(&mut thread, "show me my cart").await;
    thread.messages().to_vec()
}

/// The content of the first tool result, parsed.
fn tool_result(messages: &[Message]) -> Value {
    let payload = messages
        .iter()
        .find_map(|message| match message {
            Message::Tool(tool) => Some(tool.content.clone()),
            _ => None,
        })
        .expect("the agent should have produced a tool result");
    serde_json::from_str(payload.as_text().expect("an A2UI tool result is text"))
        .expect("the tool result should be JSON")
}

/// The surface activities a conversation holds, in order.
fn activities(messages: &[Message]) -> Vec<&ag_ui::ActivityMessage> {
    messages
        .iter()
        .filter_map(|message| match message {
            Message::Activity(activity) if activity.activity_type == SURFACE_ACTIVITY_TYPE => {
                Some(activity)
            }
            _ => None,
        })
        .collect()
}

/// What a renderer that holds nothing shows after `operations`.
fn painted_alone(operations: &[AgentMessage]) -> SurfaceState {
    let mut store = SurfaceStore::new();
    for operation in operations {
        store.apply(operation).expect("an operation that applies");
    }
    store.get(SURFACE_ID).expect("the cart").clone()
}

#[tokio::test(flavor = "multi_thread")]
async fn the_operations_envelope_arrives_byte_for_byte_intact() {
    let messages = ship(Merchant::default()).await;
    let value = tool_result(&messages);

    assert!(
        is_operations_envelope(&value),
        "the frontend's content sniff must still match: {value}"
    );

    let received = messages
        .iter()
        .find(|message| matches!(message, Message::Tool(_)))
        .and_then(operations_in)
        .expect("the tool result carries operations");
    assert_eq!(
        received,
        assemble_ops(Intent::Create, &spec(&FIRST)),
        "the operations the client sees must be the ones the agent built"
    );

    // The wire contract every other language's toolkit keys on.
    let operations = value[A2UI_OPERATIONS_KEY]
        .as_array()
        .expect("an operations array");
    assert_eq!(operations.len(), 3, "create, components, data");
    assert_eq!(operations[0]["version"], PROTOCOL_VERSION);
    assert_eq!(
        operations[0]["createSurface"]["catalogId"],
        BASIC_CATALOG_ID
    );
    assert_eq!(operations[0]["createSurface"]["surfaceId"], SURFACE_ID);
}

#[tokio::test(flavor = "multi_thread")]
async fn the_surface_still_validates_after_the_round_trip() {
    let messages = ship(Merchant::default()).await;
    let surfaces = surfaces_in(&messages).expect("a conversation that replays");
    let cart = surfaces.get(SURFACE_ID).expect("the cart");
    let data = cart
        .data_model
        .to_json()
        .expect("a data model without holes");

    let report = Validator::new(&Catalog::basic()).validate_surface(&cart.components, Some(&data));
    assert!(report.is_valid(), "{:?}", report.errors);
    assert!(report.unreachable.is_empty(), "{:?}", report.unreachable);

    // The templated binding survived: a child template is not a child list.
    assert_eq!(
        cart.components[2].prop("children"),
        Some(&json!({"componentId": "row", "path": "/items"}))
    );
    // And it draws: the template once for each item, in the item's scope.
    let rows: Vec<String> = cart
        .outline(&Catalog::basic(), &data)
        .iter()
        .filter(|placed| placed.component.id == "row")
        .map(|placed| {
            placed
                .scope
                .resolve("name")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_owned()
        })
        .collect();
    assert_eq!(rows, FIRST);
}

/// The envelope rides on a tool call, so the call itself has to arrive too —
/// otherwise the renderer has a payload it cannot attribute. Its arguments are
/// `render_a2ui`'s own.
#[tokio::test(flavor = "multi_thread")]
async fn the_tool_call_carrying_the_surface_arrives_with_it() {
    let messages = ship(Merchant::default()).await;

    let call = messages
        .iter()
        .find_map(|message| match message {
            Message::Assistant(assistant) => assistant.tool_calls.as_ref()?.first(),
            _ => None,
        })
        .expect("the surface should have arrived as a tool call");

    assert_eq!(call.function.name, RENDER_A2UI_TOOL_NAME);
    let arguments: RenderArgs =
        serde_json::from_str(&call.function.arguments).expect("render_a2ui arguments");
    let expected = spec(&FIRST);
    assert_eq!(arguments.surface_id, SURFACE_ID);
    assert_eq!(arguments.components, expected.components);
    assert_eq!(arguments.data, expected.data_model);

    let result = messages
        .iter()
        .find_map(|message| match message {
            Message::Tool(tool) => Some(tool),
            _ => None,
        })
        .expect("a tool result");
    assert_eq!(result.tool_call_id, call.id, "result answers the call");
}

/// What an AG-UI frontend paints from: the `a2ui-surface` activity, after the
/// tool result, under the id of the call, carrying the surface whole.
#[tokio::test(flavor = "multi_thread")]
async fn the_surface_activity_arrives_after_the_result_and_paints_alone() {
    let messages = ship(Merchant::default()).await;

    let result = messages
        .iter()
        .position(|message| matches!(message, Message::Tool(_)))
        .expect("a tool result");
    let Message::Tool(tool) = &messages[result] else {
        unreachable!()
    };
    let Message::Activity(activity) = &messages[result + 1] else {
        panic!(
            "the activity follows the tool result: {:?}",
            messages[result + 1]
        );
    };
    assert_eq!(activity.activity_type, SURFACE_ACTIVITY_TYPE);
    assert_eq!(activity.id, surface_activity_id(tool.tool_call_id.as_str()));

    let operations = operations_in(&messages[result + 1]).expect("the activity carries operations");
    assert_eq!(
        &painted_alone(&operations),
        surfaces_in(&messages)
            .expect("a conversation that replays")
            .get(SURFACE_ID)
            .expect("the cart")
    );
}

/// A client leaves activity messages out of the next request, as the reference
/// client does. So a server never sees the activity it sent: it rebuilds what
/// the frontend holds from the tool results, and has to reach the surfaces the
/// client reads from its own list, which has both.
#[tokio::test(flavor = "multi_thread")]
async fn a_later_run_is_sent_the_tool_results_and_not_the_activities() {
    let merchant = Merchant::default();
    let requests = merchant.requests.clone();
    let url = serve(merchant).await;
    let mut thread = Thread::<_>::new(transport(&url), "cart");
    turn(&mut thread, "show me my cart").await;
    turn(&mut thread, "add a baguette").await;
    let client = thread.messages().to_vec();

    // What the server was sent for the second run.
    let requests = requests.lock().expect("the request log");
    let [_, second] = requests.as_slice() else {
        panic!("two runs, two requests: {}", requests.len());
    };
    assert!(
        !second
            .iter()
            .any(|message| matches!(message, Message::Activity(_))),
        "an activity never travels back to the agent"
    );
    assert_eq!(
        second.iter().filter_map(operations_in).count(),
        1,
        "the first run's tool result does"
    );
    // It was enough: the second run updated the cart, it did not create one.
    assert!(
        client
            .iter()
            .any(|message| matches!(message, Message::Assistant(said) if said.content.as_deref() == Some("Added to your cart."))),
        "{client:?}"
    );

    // The client holds a tool result and an activity for each run. The second
    // activity is a new one, under the second run's call, and carries the
    // whole cart though the run sent an update.
    let results: Vec<Vec<AgentMessage>> = client
        .iter()
        .filter(|message| matches!(message, Message::Tool(_)))
        .filter_map(operations_in)
        .collect();
    assert_eq!(results.len(), 2);
    assert!(
        results[1]
            .iter()
            .all(|operation| !matches!(operation.payload, AgentPayload::CreateSurface(_))),
        "the second batch is an update"
    );
    let painted = activities(&client);
    assert_eq!(painted.len(), 2, "one activity for each run");
    assert_ne!(painted[0].id, painted[1].id);
    let whole = operations_in(&Message::Activity(painted[1].clone())).expect("operations");
    assert!(
        matches!(whole[0].payload, AgentPayload::CreateSurface(_)),
        "{whole:?}"
    );

    // Three ways to the same cart: the client's list, the server's, and the
    // last activity on its own.
    let seen_by_client = surfaces_in(&client).expect("the client's list replays");
    let server: Vec<Message> = client
        .iter()
        .filter(|message| !matches!(message, Message::Activity(_)))
        .cloned()
        .collect();
    assert_eq!(
        seen_by_client,
        surfaces_in(&server).expect("the server's list replays")
    );
    let cart = seen_by_client.get(SURFACE_ID).expect("the cart");
    assert_eq!(&painted_alone(&whole), cart);
    assert_eq!(
        cart.data_model.to_json().expect("data")["items"]
            .as_array()
            .map(Vec::len),
        Some(SECOND.len())
    );
}

/// A skeleton is one activity message a client holds, and only an event under
/// the same message id replaces it. An update of a surface that another call
/// painted would go under that call's activity, and the updating call's
/// skeleton would be on screen for good.
#[tokio::test(flavor = "multi_thread")]
async fn a_skeleton_gives_way_to_the_surface_its_call_paints() {
    let client = ship(Sketcher).await;

    let shown = activities(&client);
    assert_eq!(shown.len(), 2, "one activity for each call: {shown:?}");
    assert!(
        shown
            .iter()
            .all(|activity| !activity.content.contains_key("status")),
        "no call is left showing a skeleton: {shown:?}"
    );
    // Each call's activity is the cart whole, as that call left it.
    let calls: Vec<&str> = client
        .iter()
        .filter_map(|message| match message {
            Message::Tool(tool) => Some(tool.tool_call_id.as_str()),
            _ => None,
        })
        .collect();
    assert_eq!(calls.len(), 2);
    let items = |activity: &ag_ui::ActivityMessage| {
        let operations =
            operations_in(&Message::Activity(activity.clone())).expect("a painted activity");
        painted_alone(&operations)
            .data_model
            .to_json()
            .expect("data")["items"]
            .as_array()
            .map(Vec::len)
    };
    for (activity, (call, held)) in shown
        .iter()
        .zip(calls.iter().zip([&FIRST[..], &SECOND[..]]))
    {
        assert_eq!(activity.id, surface_activity_id(call));
        assert_eq!(items(activity), Some(held.len()));
    }

    // The client's list and the one a server is sent still read the same cart.
    let server: Vec<Message> = client
        .iter()
        .filter(|message| !matches!(message, Message::Activity(_)))
        .cloned()
        .collect();
    let seen = surfaces_in(&client).expect("the client's list replays");
    assert_eq!(
        seen,
        surfaces_in(&server).expect("the server's list replays")
    );
    assert_eq!(
        seen.get(SURFACE_ID)
            .expect("the cart")
            .data_model
            .to_json()
            .expect("data")["items"]
            .as_array()
            .map(Vec::len),
        Some(SECOND.len())
    );
}

/// A surface that could not be built has to reach the frontend as a failure and
/// not as A2UI. The reason it travels the same tool result is exactly why it
/// matters: the sniff is all that separates the two.
#[tokio::test(flavor = "multi_thread")]
async fn a_failed_surface_arrives_as_a_failure_and_not_as_a_surface() {
    let messages = ship(Butterfingers).await;
    let value = tool_result(&messages);

    assert!(!is_operations_envelope(&value), "{value}");
    assert!(value.get(A2UI_OPERATIONS_KEY).is_none(), "{value}");
    assert_eq!(
        value["error"],
        "Failed to generate valid A2UI after 1 attempt(s)"
    );
    assert_eq!(value["code"], RECOVERY_EXHAUSTED_CODE);
    assert_eq!(
        value["attempts"][0]["errors"][0]["code"],
        "unresolved_child"
    );

    // The same failure on the activity, which is what a frontend shows in the
    // surface's place.
    let [failed] = activities(&messages)[..] else {
        panic!("one activity: {messages:?}");
    };
    assert_eq!(failed.content["status"], "failed");
    assert_eq!(failed.content["error"], value["error"]);
    assert!(operations_in(&Message::Activity(failed.clone())).is_none());
}

/// The failure rides in the conversation from here on, so the next turn must
/// not read it as the surface the user is looking at.
#[tokio::test(flavor = "multi_thread")]
async fn a_failed_surface_is_not_recovered_from_history_as_a_prior_surface() {
    let rendered = ship(Merchant::default()).await;
    let failed = ship(Butterfingers).await;
    assert!(
        surfaces_in(&failed)
            .expect("a failure replays as nothing")
            .get(SURFACE_ID)
            .is_none()
    );

    // A failure that got picked up would stand in for the surface that really
    // is on screen.
    let mut then_failed = rendered.clone();
    then_failed.extend(failed);
    let after = surfaces_in(&then_failed).expect("a conversation that replays");
    assert_eq!(after, surfaces_in(&rendered).expect("replays"));
    assert_eq!(
        after.get(SURFACE_ID).expect("the cart").components,
        spec(&FIRST).components
    );
}
