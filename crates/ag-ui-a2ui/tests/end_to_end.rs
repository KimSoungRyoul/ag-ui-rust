//! The whole agent-side loop, end to end.
//!
//! Each unit test covers one module. These cover the seams between them: a turn
//! that creates a surface, a follow-up turn that edits it, the failure path
//! where a model's output is rejected and the errors go back out as an error
//! envelope, and a surface shown while it is still being written.
//!
//! Generation is the agent layer's; the envelope, the surface activity and
//! reading a conversation back are the AG-UI adapter's.

#![cfg(feature = "ag-ui")]

use ag_ui::{Event, Message};
use ag_ui_a2ui::A2uiVersion;
use ag_ui_a2ui::adapters::agui::{
    Attempt, Intent, SurfacePainter, SurfaceSpec, SurfaceStatus, assemble_ops, latest_surface,
    operations_in, recovery_exhausted_envelope, surfaces_in, wrap_as_operations_envelope,
};
use ag_ui_a2ui::agent::{
    A2uiGenerator, A2uiRequestProcessor, CatalogConfig, CatalogDocument, RendererCapabilities,
    ResponsePart, RetryDriver, StreamOptions, StreamPart, Verdict,
};
use ag_ui_a2ui::binding::Scope;
use ag_ui_a2ui::catalog::Catalog;
use ag_ui_a2ui::constants::{A2UI_OPERATIONS_KEY, OFFICIAL_BASIC_CATALOG_ID, PROTOCOL_VERSION};
use ag_ui_a2ui::message::{AgentMessage, AgentPayload, Component};
use ag_ui_a2ui::validate::{ErrorCode, Validator};
use serde_json::{Value, json};

const CREATE_RESPONSE: &str = r#"Here is your order.
<a2ui-json>[
  {"version": "v0.9", "createSurface": {"surfaceId": "order", "catalogId": "https://a2ui.org/specification/v0_9/catalogs/basic/catalog.json"}},
  {"version": "v0.9", "updateComponents": {"surfaceId": "order", "components": [
    {"id": "root", "component": "Column", "children": ["heading", "items"]},
    {"id": "heading", "component": "Text", "text": {"path": "/title"}, "variant": "h2"},
    {"id": "items", "component": "List", "children": {"componentId": "row", "path": "/items"}},
    {"id": "row", "component": "Text", "text": {"call": "formatString", "args": {"value": "${@index(offset: 1)}. ${name}"}}}
  ]}},
  {"version": "v0.9", "updateDataModel": {"surfaceId": "order", "path": "/", "value": {
    "title": "Your order", "items": [{"name": "Espresso"}, {"name": "Croissant"}]
  }}}
]</a2ui-json>"#;

/// The processor of a request from a renderer that draws the basic catalog.
fn processor() -> A2uiRequestProcessor {
    let generator = A2uiGenerator::new(vec![CatalogConfig::basic(A2uiVersion::V0_9).unwrap()]);
    let renderer = RendererCapabilities::supporting(A2uiVersion::V0_9, [OFFICIAL_BASIC_CATALOG_ID]);
    generator.create_processor(Some(&renderer)).unwrap()
}

/// The messages of an answer that stood.
fn messages(parts: Vec<ResponsePart>) -> Vec<AgentMessage> {
    parts
        .into_iter()
        .flat_map(|part| match part {
            ResponsePart::A2ui(messages) => messages,
            ResponsePart::Text(_) => Vec::new(),
        })
        .collect()
}

#[test]
fn a_turn_generates_validates_and_ships_a_surface() {
    let processor = processor();

    // 1. The instructions for the generating model.
    let system = format!("You build UI surfaces.\n\n{}", processor.prompt_snippet());
    assert!(system.contains("---BEGIN A2UI JSON SCHEMA---"));
    assert!(system.contains(OFFICIAL_BASIC_CATALOG_ID));

    // 2. The model's answer is judged. A clean first answer stands.
    let mut state = processor.create_state();
    let mut retry = RetryDriver::new(&processor, 3);
    let Verdict::Done(parts) = retry.judge(CREATE_RESPONSE, &mut state) else {
        panic!("the canned response is valid A2UI");
    };
    assert_eq!(retry.attempts(), 1);
    assert_eq!(parts[0], ResponsePart::Text("Here is your order.".into()));

    // 3. The state is what the renderer now holds, and it is a whole surface.
    let surface = state.surface("order").expect("the surface was created");
    assert_eq!(surface.components.len(), 4);
    let report = state.processor().report("order").unwrap();
    assert!(report.is_valid(), "{:?}", report.errors);
    assert!(report.unreachable.is_empty());

    // 4. Bindings resolve the way the renderer will resolve them.
    let json_model = surface.data_model.to_json().unwrap();
    let root = Scope::root(&json_model);
    assert_eq!(root.resolve_string("/title"), "Your order");
    assert_eq!(
        root.item("/items", 1)
            .format_string("${@index(offset: 1)}. ${name}")
            .unwrap(),
        "2. Croissant"
    );

    // 5. Ship it: the batch as the tool result, the surface as the activity.
    let painted = SurfacePainter::new()
        .settle("call-1", &messages(parts))
        .unwrap();
    let value: Value = serde_json::from_str(&painted.envelope).unwrap();
    let operations = value[A2UI_OPERATIONS_KEY].as_array().unwrap();
    assert_eq!(operations.len(), 3);
    assert_eq!(operations[0]["version"], PROTOCOL_VERSION);
    assert_eq!(
        operations[0]["createSurface"]["catalogId"],
        OFFICIAL_BASIC_CATALOG_ID
    );
    let [Event::ActivitySnapshot(activity)] = painted.activities.as_slice() else {
        panic!("one surface, one activity");
    };
    assert_eq!(activity.activity_type, "a2ui-surface");
    assert_eq!(
        activity.content[A2UI_OPERATIONS_KEY],
        value[A2UI_OPERATIONS_KEY]
    );
}

#[test]
fn a_follow_up_turn_edits_the_surface_without_re_creating_it() {
    let processor = processor();
    // A canvas: one state for the conversation.
    let mut state = processor.create_state();
    let first = messages(
        processor
            .parse_response_in(CREATE_RESPONSE, &mut state)
            .unwrap(),
    );

    // Turn one is in the next request as the result of the tool call that
    // painted it.
    let painted = SurfacePainter::new().settle("call-1", &first).unwrap();
    let history = vec![
        Message::user("m-1", "Show my coffee order."),
        Message::tool("m-2", "call-1", painted.envelope),
        Message::user("m-3", "Add the total."),
    ];
    assert_eq!(operations_in(&history[1]), Some(first));

    // Turn two recovers what is on screen, which is what the state holds.
    let surfaces = surfaces_in(&history).unwrap();
    let prior = surfaces.get("order").expect("the surface is in history");
    assert_eq!(prior.catalog_id.as_deref(), Some(OFFICIAL_BASIC_CATALOG_ID));
    assert_eq!(prior.components, state.surface("order").unwrap().components);
    assert_eq!(
        prior.data_model.to_json().unwrap()["items"][0]["name"],
        "Espresso"
    );

    // The model's edit is read against the surface as it stands: an update
    // alone is a whole answer, and it may name components it does not send.
    let edit = r#"<a2ui-json>[
      {"version": "v0.9", "updateComponents": {"surfaceId": "order", "components": [
        {"id": "root", "component": "Column", "children": ["heading", "items", "total"]},
        {"id": "total", "component": "Text", "text": {"path": "/total"}}
      ]}},
      {"version": "v0.9", "updateDataModel": {"surfaceId": "order", "path": "/total", "value": "$7.50"}}
    ]</a2ui-json>"#;
    let sent = messages(processor.parse_response_in(edit, &mut state).unwrap());
    assert!(
        !sent
            .iter()
            .any(|op| matches!(op.payload, AgentPayload::CreateSurface(_))),
        "updating an existing surface must never re-create it"
    );
    let surface = state.surface("order").unwrap();
    assert_eq!(surface.components.len(), 5);
    assert_eq!(surface.data_model.to_json().unwrap()["total"], "$7.50");

    // Re-creating the surface is the model's mistake, and the state is left
    // as the edit left it.
    let before = state.clone();
    let rejected = processor
        .parse_response_in(CREATE_RESPONSE, &mut state)
        .unwrap_err();
    assert!(rejected.error.to_string().contains("already exists"));
    assert_eq!(state, before);

    // The same edit built in code ships as an update too.
    let spec = SurfaceSpec::new(&prior.surface_id)
        .with_catalog_id(OFFICIAL_BASIC_CATALOG_ID)
        .with_components(vec![
            Component::new("root", "Column").with("children", json!(["heading", "items", "total"])),
            Component::new("total", "Text").with("text", json!({"path": "/total"})),
        ])
        .with_data_model(json!("$7.50"))
        .with_data_path("/total");
    let operations = assemble_ops(Intent::Update, &spec);
    assert!(
        !operations
            .iter()
            .any(|op| matches!(op.payload, AgentPayload::CreateSurface(_)))
    );
    assert_eq!(operations, sent);
}

#[test]
fn a_surface_that_cannot_be_fixed_ships_as_an_error_envelope() {
    let processor = processor();
    // Every attempt names a child that was never defined.
    let broken = CREATE_RESPONSE.replace(
        r#"{"id": "root", "component": "Column", "children": ["heading", "items"]}"#,
        r#"{"id": "root", "component": "Card", "child": "never-defined"}"#,
    );

    // The frontend is shown where the surface is from the start: the painter
    // that would paint it puts the call's activity in each state.
    let mut painter = SurfacePainter::new();
    let mut shown = vec![painter.status("call-1", &SurfaceStatus::Building)];
    let mut attempts: Vec<Attempt> = Vec::new();

    let mut state = processor.create_state();
    let mut retry = RetryDriver::new(&processor, 3);
    let mut prompt = "Show my coffee order.".to_string();
    let error = loop {
        match retry.judge(&broken, &mut state) {
            Verdict::Retry(feedback) => {
                attempts.push(Attempt::rejected(&feedback));
                shown.push(painter.status("call-1", &SurfaceStatus::retrying(&feedback, 3)));
                prompt = feedback.prompt("Show my coffee order.");
            }
            Verdict::GiveUp(error) => break error,
            other => panic!("the answer names a child that does not exist: {other:?}"),
        }
    };
    assert_eq!(
        retry.attempts(),
        3,
        "the driver should use its whole budget"
    );
    assert!(prompt.contains("Correction required"));
    assert!(prompt.contains("unresolved_child"));
    assert!(state.surface("order").is_none(), "nothing was applied");

    let ag_ui_a2ui::Error::RecoveryExhausted { last, .. } = error else {
        panic!("expected RecoveryExhausted");
    };
    assert_eq!(last.as_slice()[0].code, ErrorCode::UnresolvedChild);

    // The frontend receives the reason, and nothing that looks like a surface:
    // the tool result says the attempts are used up, and the surface activity
    // says it failed.
    attempts.push(Attempt::failed(3, last.as_slice().to_vec()));
    assert_eq!(
        attempts,
        (1..=3)
            .map(|attempt| Attempt::failed(attempt, last.as_slice().to_vec()))
            .collect::<Vec<_>>()
    );
    let envelope = recovery_exhausted_envelope(&attempts).unwrap();
    let value: Value = serde_json::from_str(&envelope).unwrap();
    assert!(value.get(A2UI_OPERATIONS_KEY).is_none(), "{value}");
    assert_eq!(
        value["error"],
        "Failed to generate valid A2UI after 3 attempt(s)"
    );
    assert_eq!(value["code"], "a2ui_recovery_exhausted");
    assert_eq!(
        value["attempts"][2]["errors"][0]["path"],
        "messages[1].updateComponents.components[0].child"
    );
    shown.push(painter.status(
        "call-1",
        &SurfaceStatus::Failed {
            error: value["error"].as_str().unwrap().to_string(),
            attempts,
            max_attempts: 3,
        },
    ));
    // One activity, replaced in place: building, the attempt after each
    // rejected one, then failed.
    let states: Vec<(&str, String)> = shown
        .iter()
        .map(|event| {
            let Event::ActivitySnapshot(status) = event else {
                panic!("a status is an activity snapshot");
            };
            let attempt = status.content.get("attempt").map(Value::to_string);
            (
                status.message_id.as_str(),
                format!(
                    "{} {}",
                    status.content["status"].as_str().unwrap(),
                    attempt.unwrap_or_default()
                ),
            )
        })
        .collect();
    assert_eq!(
        states,
        [
            ("a2ui-surface-call-1", "building ".to_string()),
            ("a2ui-surface-call-1", "retrying 2".to_string()),
            ("a2ui-surface-call-1", "retrying 3".to_string()),
            ("a2ui-surface-call-1", "failed ".to_string()),
        ]
    );

    // So the next turn does not offer the surface that was never built as the
    // one the user is looking at.
    let history = [Message::tool("m-1", "call-1", envelope)];
    assert!(surfaces_in(&history).unwrap().get("order").is_none());
}

/// A model that tidies up before it draws deletes a surface that was never
/// there. Every layer makes the same thing of that: nothing. The answer
/// stands, the batch is painted, and the conversation that holds the batch
/// replays.
#[test]
fn a_delete_of_a_surface_that_is_not_there_does_nothing_from_the_answer_to_the_replay() {
    let processor = processor();
    let answer = format!(
        r#"<a2ui-json>[
  {{"version": "v0.9", "deleteSurface": {{"surfaceId": "old"}}}},
  {{"version": "v0.9", "createSurface": {{"surfaceId": "s", "catalogId": "{OFFICIAL_BASIC_CATALOG_ID}"}}}},
  {{"version": "v0.9", "updateComponents": {{"surfaceId": "s", "components": [
    {{"id": "root", "component": "Text", "text": "Hello"}}]}}}}
]</a2ui-json>"#
    );

    // The agent layer: the answer stands, and the delete is among the
    // messages to send, since a renderer is told and does nothing.
    let mut state = processor.create_state();
    let Verdict::Done(parts) = RetryDriver::new(&processor, 3).judge(&answer, &mut state) else {
        panic!("the answer is valid A2UI");
    };
    let sent = messages(parts);
    assert_eq!(sent.len(), 3);
    assert_eq!(sent[0], AgentMessage::delete_surface("old"));
    assert!(state.surface("old").is_none() && state.surface("s").is_some());

    // The painter: the batch is not refused for it. The tool result is the
    // batch as sent; the one activity is the surface the batch drew, under
    // the call's own id, where a skeleton for the call would be.
    let mut painter = SurfacePainter::new();
    let building = painter.status("call-1", &SurfaceStatus::Building);
    let painted = painter.settle("call-1", &sent).unwrap();
    assert_eq!(
        painted.envelope,
        wrap_as_operations_envelope(&sent).unwrap()
    );
    let [Event::ActivitySnapshot(activity)] = painted.activities.as_slice() else {
        panic!(
            "one surface changed, so one activity: {:?}",
            painted.activities
        );
    };
    let Event::ActivitySnapshot(building) = building else {
        panic!("a status is an activity snapshot");
    };
    assert_eq!(activity.message_id, building.message_id);
    let drawn: Vec<AgentMessage> =
        serde_json::from_value(activity.content[A2UI_OPERATIONS_KEY].clone()).unwrap();
    // The activity carries surface `s` whole, and nothing of `old`.
    assert_eq!(drawn, painter.surfaces().get("s").unwrap().replay());
    assert_eq!(drawn[..2], sent[1..]);
    assert!(
        drawn
            .iter()
            .all(|message| message.surface_id() == Some("s"))
    );
    assert_eq!(painter.surfaces(), state.store());

    // The conversation that holds the envelope replays, as the server of the
    // next run is sent it and as a client holds it.
    let server = vec![
        Message::user("m-1", "Start over and say hello."),
        Message::tool("m-2", "call-1", painted.envelope.clone()),
    ];
    let mut client = server.clone();
    client.push(Message::Activity(ag_ui::ActivityMessage {
        id: activity.message_id.clone(),
        activity_type: activity.activity_type.clone(),
        content: activity.content.clone(),
        ..ag_ui::ActivityMessage::default()
    }));
    for conversation in [&server, &client] {
        assert_eq!(&surfaces_in(conversation).unwrap(), painter.surfaces());
        assert_eq!(operations_in(&conversation[1]), Some(sent.clone()));
        // What the user is looking at is the surface that was drawn, not the
        // one the first operation names.
        let latest = latest_surface(conversation).unwrap().unwrap();
        assert_eq!(latest.surface_id, "s");
        assert_eq!(
            SurfacePainter::from_history(conversation)
                .unwrap()
                .surfaces(),
            painter.surfaces()
        );
    }

    // And so does one that ends with such a delete: the surface is still the
    // last one the conversation changed.
    let mut later = server.clone();
    later.push(Message::tool(
        "m-3",
        "call-2",
        wrap_as_operations_envelope(&[AgentMessage::delete_surface("old")]).unwrap(),
    ));
    assert_eq!(&surfaces_in(&later).unwrap(), painter.surfaces());
    assert_eq!(latest_surface(&later).unwrap().unwrap().surface_id, "s");
    let lone = [later[2].clone()];
    assert_eq!(surfaces_in(&lone).unwrap().surfaces().count(), 0);
    assert!(latest_surface(&lone).unwrap().is_none());
}

#[test]
fn a_custom_catalog_drives_validation_the_same_way() {
    // A design system that only allows MenuItem inside Menu.
    let catalog = Catalog::from_schema(&json!({
        "catalogId": "example.com:design-system",
        "components": {
            "Menu": {
                "type": "object",
                "allowedChildren": ["MenuItem"],
                "properties": {
                    "component": {"const": "Menu"},
                    "children": {"$ref": "common_types.json#/$defs/ChildList"}
                },
                "required": ["component", "children"]
            },
            "MenuItem": {
                "type": "object",
                "allowedParents": ["Menu"],
                "properties": {
                    "component": {"const": "MenuItem"},
                    "label": {"type": "string"}
                },
                "required": ["component", "label"]
            },
            "Banner": {"type": "object", "properties": {"component": {"const": "Banner"}}}
        }
    }))
    .unwrap();

    let components = vec![
        Component::new("root", "Menu").with("children", json!(["one", "stray"])),
        Component::new("one", "MenuItem").with("label", json!("First")),
        Component::new("stray", "Banner"),
    ];

    // Structurally sound: every reference resolves, no cycles, root present.
    let report = Validator::new(&catalog).validate(&components);
    assert!(report.is_valid(), "{:?}", report.errors);

    // But composition constraints reject the Banner inside the Menu.
    let violations = catalog.composition_violations(&components);
    assert_eq!(violations.len(), 1);
    assert_eq!(violations[0].code.as_str(), "UNALLOWED_CHILD");
    assert_eq!(violations[0].path, "components[0].children[1]");

    // A missing required property is caught by the validator itself.
    let report = Validator::new(&catalog).validate(&[
        Component::new("root", "Menu").with("children", json!(["one"])),
        Component::new("one", "MenuItem"),
    ]);
    assert_eq!(report.errors.len(), 1);
    assert_eq!(report.errors[0].code, ErrorCode::MissingRequiredProp);
    assert_eq!(report.errors[0].path, "components[1].label");
}

#[test]
fn a_surface_renders_progressively_while_it_is_generated() {
    // The same generation as the first test, but delivered in token-sized
    // chunks: the user sees the tree fill in instead of waiting for the end.
    let processor = processor();
    let mut state = processor.create_state();
    let mut stream = processor
        .open_stream(&state, StreamOptions::default())
        .unwrap();

    let mut seen_text = Vec::new();
    let mut renders: Vec<AgentMessage> = Vec::new();
    let mut take = |parts: Vec<StreamPart>| {
        for part in parts {
            match part {
                StreamPart::Text(text) => seen_text.push(text),
                StreamPart::A2ui(messages) => renders.extend(messages),
                StreamPart::Rejected(error) => {
                    panic!("the canned response streams cleanly: {error}")
                }
                other => panic!("a part this test does not know: {other:?}"),
            }
        }
    };
    let mut cursor = 0;
    while cursor < CREATE_RESPONSE.len() {
        // A chunk boundary at an arbitrary character, including mid-token.
        let mut end = (cursor + 37).min(CREATE_RESPONSE.len());
        while !CREATE_RESPONSE.is_char_boundary(end) {
            end += 1;
        }
        take(
            stream
                .feed(&CREATE_RESPONSE[cursor..end], &mut state)
                .expect("the canned response streams cleanly"),
        );
        cursor = end;
    }
    take(stream.close(&mut state).unwrap());

    assert_eq!(seen_text.concat(), "Here is your order.");

    // The surface was created before any components arrived.
    assert!(matches!(renders[0].payload, AgentPayload::CreateSurface(_)));

    // Something renderable was emitted well before the generation finished:
    // the component list grows, and each time it does it is sent again whole.
    let lists: Vec<usize> = renders
        .iter()
        .filter_map(|message| match &message.payload {
            AgentPayload::UpdateComponents(update) => Some(update.components.len()),
            _ => None,
        })
        .collect();
    assert!(lists.len() > 1, "expected several progressive updates");
    assert!(lists.windows(2).all(|pair| pair[0] <= pair[1]), "{lists:?}");
    assert_eq!(lists.last(), Some(&4));

    // What streamed out is what a whole read leaves, and it is a whole surface.
    let mut whole = processor.create_state();
    processor
        .parse_response_in(CREATE_RESPONSE, &mut whole)
        .unwrap();
    assert_eq!(state, whole);
    state.check_trees(["order"]).unwrap();
}

#[test]
fn a_catalog_is_negotiated_then_pruned_into_the_prompt() {
    // The agent registers a design system and lets the model use part of it.
    let design_system = CatalogDocument::load(
        json!({
            "catalogId": "example.com:design-system",
            "components": {
                "Text": {"type": "object", "properties": {"component": {"const": "Text"}}},
                "Chart": {"type": "object", "properties": {"component": {"const": "Chart"}}}
            }
        }),
        None,
        Some(A2uiVersion::V0_9),
    )
    .unwrap();
    let generator = A2uiGenerator::new(vec![
        CatalogConfig::new(design_system).keeping_components(["Text"]),
    ])
    .accepting_inline_catalogs(true);

    // The renderer draws the agent's catalog, and sends one of its own inline.
    let renderer = RendererCapabilities::supporting(
        A2uiVersion::V0_9,
        ["example.com:extended", "example.com:design-system"],
    )
    .with_inline_catalogs(
        A2uiVersion::V0_9,
        vec![json!({
            "catalogId": "example.com:extended",
            "components": {"Text": {}, "Sparkline": {"type": "object"}}
        })],
    );
    let processor = generator.create_processor(Some(&renderer)).unwrap();
    let active: Vec<&str> = processor
        .active_catalogs()
        .iter()
        .map(CatalogDocument::catalog_id)
        .collect();
    // The catalogs the agent registered come first, then the renderer's own.
    assert_eq!(
        active,
        ["example.com:design-system", "example.com:extended"]
    );

    // The prompt carries the renderer's catalog as it sent it, and of the
    // agent's only the components this agent allows.
    let prompt = processor.prompt_snippet();
    assert!(prompt.contains("---BEGIN A2UI JSON SCHEMA---"));
    assert!(prompt.contains("Sparkline"));
    assert!(
        !prompt.contains("\"Chart\":"),
        "the pruned component must be gone"
    );

    // And a surface is held to the catalog it names.
    let answer = r#"<a2ui-json>[
      {"version": "v0.9", "createSurface": {"surfaceId": "week", "catalogId": "example.com:extended"}},
      {"version": "v0.9", "updateComponents": {"surfaceId": "week", "components": [
        {"id": "root", "component": "Sparkline"}]}}
    ]</a2ui-json>"#;
    processor.parse_response(answer).unwrap();
    let pruned = answer
        .replace("example.com:extended", "example.com:design-system")
        .replace("Sparkline", "Chart");
    let rejected = processor.parse_response(&pruned).unwrap_err();
    assert!(rejected.error.to_string().contains("unknown_component"));
}
