//! An A2UI surface shipped over AG-UI.
//!
//! `ag-ui-a2ui` produces the `a2ui_operations` envelope and stops there — it is
//! transport-agnostic on purpose. AG-UI is one of the transports that carries
//! it, and that binding is otherwise only unit-tested. Here the agent builds a
//! surface with the toolkit, returns the envelope as a tool result, and the
//! client has to receive it intact enough that it still validates against the
//! catalog it was authored for.

mod common;

use ag_ui_a2ui::catalog::Catalog;
use ag_ui_a2ui::constants::{
    A2UI_OPERATIONS_KEY, BASIC_CATALOG_ID, PROTOCOL_VERSION, RENDER_A2UI_TOOL_NAME,
};
use ag_ui_a2ui::message::{AgentMessage, AgentPayload, Component};
use ag_ui_a2ui::toolkit::envelope::{
    is_operations_envelope, unwrap_operations_envelope, wrap_as_operations_envelope,
    wrap_error_envelope,
};
use ag_ui_a2ui::toolkit::ops::{Intent, SurfaceSpec, assemble_ops};
use ag_ui_a2ui::validate::Validator;
use ag_ui_client::{Session, Update};
use ag_ui_core::{Message, RunOutcome};
use ag_ui_server::{Agent, Error, Result, RunContext};
use common::{serve, transport};
use futures_util::StreamExt as _;
use serde_json::{Value, json};

const SURFACE_ID: &str = "cart";

/// The surface the agent authors: a heading bound to the data model and a
/// templated list over it.
fn spec() -> SurfaceSpec {
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
            "items": [{"name": "Espresso"}, {"name": "Croissant"}],
        }))
}

/// Builds the surface and hands it back as a tool result.
struct Merchant;

impl Agent for Merchant {
    type State = ();

    async fn run(&self, ctx: &mut RunContext<()>) -> Result<RunOutcome> {
        let operations = assemble_ops(Intent::Create, &spec());
        let envelope = wrap_as_operations_envelope(&operations).map_err(Error::agent)?;

        let mut call = ctx.tool_call(RENDER_A2UI_TOOL_NAME)?;
        call.args_json(&json!({ "surfaceId": SURFACE_ID }))?;
        call.result(envelope)?;

        ctx.say("Here is your cart.")?;
        Ok(RunOutcome::Success)
    }
}

/// Authors a surface that does not validate, and ships the failure in the
/// shape a renderer keyed on the envelope key can still recognise.
struct Butterfingers;

impl Agent for Butterfingers {
    type State = ();

    async fn run(&self, ctx: &mut RunContext<()>) -> Result<RunOutcome> {
        let broken = vec![Component::new("root", "Card").with("child", json!("gone"))];
        let report = Validator::new(&Catalog::basic()).validate(&broken);
        assert!(!report.is_valid(), "this surface is meant to be invalid");

        let envelope =
            wrap_error_envelope(SURFACE_ID, "could not build the surface", &report.errors)
                .map_err(Error::agent)?;

        let mut call = ctx.tool_call(RENDER_A2UI_TOOL_NAME)?;
        call.args_json(&json!({ "surfaceId": SURFACE_ID }))?;
        call.result(envelope)?;

        Ok(RunOutcome::Success)
    }
}

/// Runs one turn and returns the tool result the agent produced, parsed.
async fn ship(agent: impl Agent + 'static) -> (Vec<Message>, Value) {
    let url = serve(agent).await;
    let mut session = Session::<_>::new(transport(&url), "cart");

    {
        let mut run = session.send("show me my cart");
        while let Some(update) = run.next().await {
            if let Update::Error(error) = update {
                panic!("shipping a surface should not error: {error}");
            }
        }
    }

    let payload = session
        .messages()
        .iter()
        .find_map(|message| match message {
            Message::Tool(tool) => Some(tool.content.clone()),
            _ => None,
        })
        .expect("the agent should have produced a tool result");

    let value = serde_json::from_str(&payload).expect("the tool result should be JSON");
    (session.messages().to_vec(), value)
}

#[tokio::test(flavor = "multi_thread")]
async fn the_operations_envelope_arrives_byte_for_byte_intact() {
    let (_messages, value) = ship(Merchant).await;

    assert!(
        is_operations_envelope(&value),
        "the frontend's content sniff must still match: {value}"
    );

    let received: Vec<AgentMessage> =
        unwrap_operations_envelope(&value).expect("the envelope should unwrap");
    assert_eq!(
        received,
        assemble_ops(Intent::Create, &spec()),
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
    let (_messages, value) = ship(Merchant).await;
    let received = unwrap_operations_envelope(&value).expect("the envelope should unwrap");

    let mut components = Vec::new();
    let mut data_model = Value::Null;
    for operation in &received {
        match &operation.payload {
            AgentPayload::UpdateComponents(payload) => {
                assert_eq!(payload.surface_id, SURFACE_ID);
                components.clone_from(&payload.components);
            }
            AgentPayload::UpdateDataModel(payload) => data_model = payload.value.clone(),
            _ => {}
        }
    }

    let report = Validator::new(&Catalog::basic()).validate_surface(&components, Some(&data_model));
    assert!(report.is_valid(), "{:?}", report.errors);
    assert!(report.unreachable.is_empty(), "{:?}", report.unreachable);

    // The templated binding survived: a child template is not a child list.
    assert_eq!(
        components[2].prop("children"),
        Some(&json!({"componentId": "row", "path": "/items"}))
    );
}

/// The envelope rides on a tool call, so the call itself has to arrive too —
/// otherwise the renderer has a payload it cannot attribute.
#[tokio::test(flavor = "multi_thread")]
async fn the_tool_call_carrying_the_surface_arrives_with_it() {
    let (messages, _value) = ship(Merchant).await;

    let call = messages
        .iter()
        .find_map(|message| match message {
            Message::Assistant(assistant) => assistant.tool_calls.as_ref()?.first(),
            _ => None,
        })
        .expect("the surface should have arrived as a tool call");

    assert_eq!(call.function.name, RENDER_A2UI_TOOL_NAME);
    assert_eq!(
        serde_json::from_str::<Value>(&call.function.arguments).expect("JSON arguments"),
        json!({ "surfaceId": SURFACE_ID })
    );

    let result = messages
        .iter()
        .find_map(|message| match message {
            Message::Tool(tool) => Some(tool),
            _ => None,
        })
        .expect("a tool result");
    assert_eq!(result.tool_call_id, call.id, "result answers the call");
}

/// A surface that could not be built still has to reach the renderer as A2UI,
/// or the frontend is left with pending state it can never clear.
#[tokio::test(flavor = "multi_thread")]
async fn a_failed_surface_ships_as_an_error_envelope_the_frontend_still_recognises() {
    let (_messages, value) = ship(Butterfingers).await;

    assert!(is_operations_envelope(&value), "{value}");
    assert_eq!(value[A2UI_OPERATIONS_KEY], json!([]));
    assert_eq!(value["error"]["code"], "VALIDATION_FAILED");
    assert_eq!(value["error"]["surfaceId"], SURFACE_ID);
    assert_eq!(value["error"]["details"][0]["code"], "unresolved_child");
}
