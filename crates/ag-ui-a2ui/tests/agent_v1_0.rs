//! The agent layer on A2UI v1.0, through its public API.
//!
//! The conformance suite runs upstream's v1.0 cases as they are written
//! (`tests/conformance.rs`). These are the things a framework meets when it
//! moves a session to v1.0: which keys a renderer's capabilities travel
//! under, what the prompt shows, a surface that draws from two catalogs, how
//! a stream holds a component, and what a refusal says.

#![cfg(feature = "agent")]

use std::time::Duration;

use ag_ui_a2ui::agent::{
    A2uiGenerator, A2uiRequestProcessor, AgentFunction, CatalogConfig, CatalogDocument, DirectJson,
    RendererCall, RendererCapabilities, ResponsePart, RetryDriver, Routed, StreamOptions,
    StreamPart, Verdict, validate_payload,
};
use ag_ui_a2ui::rpc::{RpcError, code};
use ag_ui_a2ui::{A2uiVersion, AgentMessage, Error, ErrorCode};
use serde_json::{Value, json};

const BASIC: &str = "https://a2ui.org/specification/v1_0/catalogs/basic/catalog.json";
const CHARTS: &str = "example.com:charts";

/// A second catalog, written the way an author writes one: its keys in an
/// order no sort would give.
const CHARTS_DOCUMENT: &str = r#"{
  "title": "Charts",
  "catalogId": "example.com:charts",
  "protocolVersion": "1.0",
  "components": {
    "LineChart": {
      "type": "object",
      "properties": {
        "component": {"const": "LineChart"},
        "series": {"$ref": "common_types.json#/$defs/DynamicValue"},
        "caption": {"$ref": "common_types.json#/$defs/DynamicString"}
      },
      "required": ["component", "series"]
    },
    "Gauge": {"type": "object", "properties": {"value": {"type": "number"}}}
  },
  "functions": {
    "rollingMean": {"returnType": "number"},
    "fetchQuote": {"returnType": "number", "allowedCallers": "agentOnly"}
  }
}"#;

fn charts() -> CatalogConfig {
    CatalogConfig::new(CatalogDocument::parse(CHARTS_DOCUMENT, None, None).unwrap())
}

fn generator() -> A2uiGenerator {
    A2uiGenerator::new(vec![
        CatalogConfig::basic(A2uiVersion::V1_0).unwrap(),
        charts(),
    ])
}

fn processor() -> A2uiRequestProcessor {
    let renderer = RendererCapabilities::supporting(A2uiVersion::V1_0, [BASIC, CHARTS]);
    generator().create_processor(Some(&renderer)).unwrap()
}

/// An answer that creates `dash` whole: its data, a column, a title bound to
/// the data, and a chart from the second catalog.
fn dashboard() -> String {
    format!(
        r#"Here it is.
<a2ui-json>[{{"version": "1.0", "createSurface": {{
  "surfaceId": "dash",
  "catalogId": "{BASIC}",
  "dataModel": {{"title": "Sales", "points": [1, 2, 3]}},
  "components": [
    {{"id": "root", "component": "Column", "children": ["title", "chart"]}},
    {{"id": "title", "component": "Text", "text": {{"@path": "/title"}}}},
    {{"id": "chart", "component": "LineChart", "@catalogId": "{CHARTS}",
      "series": {{"@path": "/points"}},
      "caption": {{"@call": "formatString", "args": {{"value": "Mean of ${{/title}}"}}}}}}
  ]
}}}}]</a2ui-json>"#
    )
}

fn located(error: &Error) -> Vec<(ErrorCode, &str)> {
    match error {
        Error::Validation { errors } => errors
            .as_slice()
            .iter()
            .map(|error| (error.code, error.path.as_str()))
            .collect(),
        other => panic!("expected a validation error, got {other:?}"),
    }
}

#[test]
fn a_v1_0_renderer_negotiates_under_its_own_keys() {
    let metadata = json!({
        // What a renderer that also speaks v0.9 sends beside it.
        "a2uiClientCapabilities": {"v0.9": {"supportedCatalogIds": ["something/else"]}},
        "a2uiRendererCapabilities": {"1.0": {"supportedCatalogIds": [CHARTS, BASIC]}},
    });
    let capabilities = RendererCapabilities::from_metadata(Some(&metadata), A2uiVersion::V1_0)
        .unwrap()
        .unwrap();
    let processor = generator().create_processor(Some(&capabilities)).unwrap();
    // In the renderer's order of preference.
    let active: Vec<&str> = processor
        .active_catalogs()
        .iter()
        .map(CatalogDocument::catalog_id)
        .collect();
    assert_eq!(active, [CHARTS, BASIC]);
    assert!(
        processor
            .active_catalogs()
            .iter()
            .all(|catalog| catalog.version() == A2uiVersion::V1_0)
    );

    // The v0.9 entry is not this session's, and a v0.9 catalog does not sit
    // beside a v1.0 one.
    let old = RendererCapabilities::from_metadata(Some(&metadata), A2uiVersion::V0_9)
        .unwrap()
        .unwrap();
    assert!(matches!(
        generator().create_processor(Some(&old)),
        Err(Error::Validation { .. })
    ));
    let mixed = A2uiGenerator::new(vec![
        CatalogConfig::basic(A2uiVersion::V1_0).unwrap(),
        CatalogConfig::basic(A2uiVersion::V0_9_1).unwrap(),
    ]);
    assert!(matches!(
        mixed.create_processor(Some(&capabilities)),
        Err(Error::Catalog(_))
    ));
}

#[test]
fn the_prompt_shows_the_v1_0_schemas_as_their_authors_wrote_them() {
    let processor = processor();
    let snippet = processor.prompt_snippet();

    // The envelope and the common types of v1.0, once.
    assert_eq!(
        snippet
            .matches(r#""$id":"https://a2ui.org/specification/v1_0/agent_to_renderer.json""#)
            .count(),
        1
    );
    assert!(snippet.contains(r#""@path":{"type":"string""#));
    assert!(!snippet.contains("server_to_client.json"));

    // Each catalog in the order it was written in, not sorted.
    let shown = snippet
        .split("### Catalog Schema:\n")
        .find(|block| block.contains(CHARTS))
        .unwrap();
    assert!(
        shown.starts_with(
            r#"{"title":"Charts","catalogId":"example.com:charts","protocolVersion":"1.0","components":{"LineChart":{"type":"object","properties":{"component""#
        ),
        "{}",
        &shown[..160]
    );

    // A pruned catalog keeps the order of what is left, and lists it again
    // in no union the author wrote, since the document has none.
    let pruned = charts().keeping_components(["Gauge"]);
    assert!(
        pruned
            .transformed()
            .written()
            .contains(r#""components":{"Gauge":{"type":"object","properties":{"value""#)
    );

    // A v0.9.1 session is still shown the v0.9.1 documents.
    let old = A2uiRequestProcessor::new(
        vec![CatalogDocument::basic(A2uiVersion::V0_9_1).unwrap()],
        Vec::new(),
        std::sync::Arc::new(DirectJson::new()),
    )
    .unwrap();
    assert!(old.prompt_snippet().contains("server_to_client.json"));
    assert!(!old.prompt_snippet().contains("@path"));
}

#[test]
fn an_answer_draws_one_surface_from_two_catalogs() {
    let processor = processor();
    let mut state = processor.create_state();
    let parts = processor
        .parse_response_in(&dashboard(), &mut state)
        .unwrap();
    let [ResponsePart::Text(text), ResponsePart::A2ui(messages)] = parts.as_slice() else {
        panic!("text, then one block: {parts:?}");
    };
    assert_eq!(text, "Here it is.");
    // The message is the one the model wrote: a creation that carries its
    // surface, in v1.0.
    let sent = serde_json::to_value(messages).unwrap();
    assert_eq!(sent.as_array().unwrap().len(), 1);
    assert_eq!(sent[0]["version"], "1.0");
    assert_eq!(
        sent[0]["createSurface"]["components"][2]["@catalogId"],
        CHARTS
    );

    let surface = state.surface("dash").unwrap();
    assert_eq!(surface.spoken_version(), A2uiVersion::V1_0);
    let read_from: Vec<_> = surface
        .components
        .iter()
        .map(|component| surface.catalog_of(component))
        .collect();
    assert_eq!(read_from, [Some(BASIC), Some(BASIC), Some(CHARTS)]);

    // A later turn updates it, and removes a value with `null`.
    let update = format!(
        r#"<a2ui-json>[
  {{"version": "1.0", "updateComponents": {{"surfaceId": "dash", "components": [
    {{"id": "chart", "component": "Gauge", "@catalogId": "{CHARTS}", "value": 3}}]}}}},
  {{"version": "1.0", "updateDataModel": {{"surfaceId": "dash", "path": "/points", "value": null}}}}
]</a2ui-json>"#
    );
    processor.parse_response_in(&update, &mut state).unwrap();
    let surface = state.surface("dash").unwrap();
    assert_eq!(surface.components[2].component, "Gauge");
    assert_eq!(
        surface.data_model.to_json().unwrap(),
        json!({"title": "Sales"})
    );
}

/// What a v1.0 answer gets wrong is the model's to fix, said where the answer
/// wrote it.
#[test]
fn a_refusal_names_what_the_answer_wrote() {
    let processor = processor();
    let answer = |component: Value| -> String {
        format!(
            r#"<a2ui-json>[{{"version": "1.0", "createSurface": {{"surfaceId": "s", "catalogId": "{BASIC}",
  "components": [{{"id": "root", "component": "Column", "children": ["part"]}}, {component}]}}}}]</a2ui-json>"#
        )
    };
    let refused = |component: Value| -> Error {
        let mut state = processor.create_state();
        let before = state.clone();
        let error = processor
            .parse_response_in(&answer(component), &mut state)
            .unwrap_err()
            .error;
        assert_eq!(state, before, "a refused answer leaves no trace");
        error
    };
    let part = "messages[0].createSurface.components[1]";

    // A catalog the session does not hold, named by a component.
    let error =
        refused(json!({"id": "part", "component": "Gauge", "@catalogId": "example.com:maps"}));
    assert_eq!(
        located(&error),
        [(
            ErrorCode::UnknownCatalog,
            format!("{part}.@catalogId").as_str()
        )]
    );
    assert!(error.to_string().contains("example.com:maps"), "{error}");

    // A component its catalog does not have: `Gauge` is in the other one.
    let error = refused(json!({"id": "part", "component": "Gauge", "value": 3}));
    assert_eq!(
        located(&error),
        [(
            ErrorCode::UnknownComponent,
            format!("{part}.component").as_str()
        )]
    );

    // A function of another catalog than the one the call is read from, and
    // one only the agent may call.
    for (call, says) in [
        (
            json!({"@call": "rollingMean", "args": {}}),
            "Unrecognized function 'rollingMean'",
        ),
        (
            json!({"@call": "fetchQuote", "catalogId": CHARTS, "args": {}}),
            "agentOnly",
        ),
    ] {
        let error = refused(json!({"id": "part", "component": "Text", "text": call}));
        assert!(matches!(error, Error::Validation { .. }), "{error:?}");
        assert!(error.to_string().contains(says), "{error}");
    }

    // A key that starts with one `@` is not one a model may invent: the
    // object is no binding, so it is no text either, and its key is reserved.
    let error = refused(json!({"id": "part", "component": "Text", "text": {"@if": true}}));
    assert_eq!(
        located(&error),
        [
            (ErrorCode::TypeMismatch, format!("{part}.text").as_str()),
            (ErrorCode::ReservedKey, format!("{part}.text.@if").as_str()),
        ]
    );

    // A message of the other family.
    let mut state = processor.create_state();
    let old = format!(
        r#"<a2ui-json>[{{"version": "v0.9", "createSurface": {{"surfaceId": "s", "catalogId": "{BASIC}"}}}}]</a2ui-json>"#
    );
    let error = processor
        .parse_response_in(&old, &mut state)
        .unwrap_err()
        .error;
    assert_eq!(
        located(&error),
        [(ErrorCode::InvalidValue, "messages[0].version")]
    );
}

#[test]
fn a_surface_with_no_catalog_of_its_own_has_each_component_name_one() {
    let processor = processor();
    let mut state = processor.create_state();
    let named = format!(
        r#"<a2ui-json>[{{"version": "1.0", "createSurface": {{"surfaceId": "s", "components": [
  {{"id": "root", "component": "Text", "@catalogId": "{BASIC}", "text": "Hi"}}]}}}}]</a2ui-json>"#
    );
    processor.parse_response_in(&named, &mut state).unwrap();
    assert_eq!(state.surface("s").unwrap().catalog_id, None);

    let unnamed = r#"<a2ui-json>[{"version": "1.0", "updateComponents": {"surfaceId": "s", "components": [
  {"id": "root", "component": "Text", "text": "Hi"}]}}]</a2ui-json>"#;
    let error = processor
        .parse_response_in(unnamed, &mut state)
        .unwrap_err()
        .error;
    assert_eq!(
        located(&error),
        [(
            ErrorCode::UnknownCatalog,
            "messages[0].updateComponents.components[0].component"
        )]
    );
    assert!(
        error.to_string().contains("no default catalogId"),
        "{error}"
    );
}

#[test]
fn a_retry_is_asked_for_in_the_words_of_the_refusal() {
    let processor = processor();
    let mut state = processor.create_state();
    let mut retry = RetryDriver::new(&processor, 2);
    let wrong = format!(
        r#"<a2ui-json>[{{"version": "1.0", "createSurface": {{"surfaceId": "s", "catalogId": "{BASIC}",
  "components": [{{"id": "root", "component": "Text", "text": {{"path": "/title"}}, "@style": "bold"}}]}}}}]</a2ui-json>"#
    );
    let Verdict::Retry(feedback) = retry.judge(&wrong, &mut state) else {
        panic!("a reserved key is the model's to fix");
    };
    let prompt = feedback.prompt("Show the title.");
    assert!(prompt.contains("@style"), "{prompt}");
    assert!(prompt.contains("reserved"), "{prompt}");

    let Verdict::Done(parts) = retry.judge(&dashboard(), &mut state) else {
        panic!("the second answer stands");
    };
    assert_eq!(parts.len(), 2);
    assert!(state.surface("dash").is_some());
}

/// From v1.0 a stream reads a component once it has closed, unless the
/// format is told otherwise; a v0.9 stream still shows one as it arrives.
#[test]
fn a_v1_0_stream_holds_a_component_until_it_has_closed() {
    let chunks = |version: &str, catalog: &str, binding: &str| -> Vec<String> {
        vec![
            format!(
                r#"<a2ui-json>[{{"version": "{version}", "createSurface": {{"surfaceId": "s", "catalogId": "{catalog}"}}}}, "#
            ),
            format!(
                r#"{{"version": "{version}", "updateComponents": {{"surfaceId": "s", "components": [{{"id": "root", "component": "Text", "text": "Hel"#
            ),
            r#"lo", "variant": "body"}]}}, "#.to_string(),
            format!(
                r#"{{"version": "{version}", "updateDataModel": {{"surfaceId": "s", "path": "/t", "value": {binding}}}}}]</a2ui-json>"#
            ),
        ]
    };
    let emitted = |processor: &A2uiRequestProcessor, chunks: &[String]| -> Vec<usize> {
        let mut state = processor.create_state();
        let mut stream = processor
            .open_stream(&state, StreamOptions::default())
            .unwrap();
        let mut per_chunk = Vec::new();
        for chunk in chunks {
            let parts = stream.feed(chunk, &mut state).unwrap();
            per_chunk.push(
                parts
                    .iter()
                    .filter_map(|part| match part {
                        StreamPart::A2ui(messages) => Some(messages.len()),
                        _ => None,
                    })
                    .sum(),
            );
        }
        stream.close(&mut state).unwrap();
        assert_eq!(state.surface("s").unwrap().components.len(), 1);
        per_chunk
    };
    let progressive = |version: A2uiVersion, hold: Option<bool>| -> A2uiRequestProcessor {
        let format = DirectJson::new().with_progressive_keys(["text"]);
        let format = match hold {
            Some(hold) => format.holding_components_until_closed(hold),
            None => format,
        };
        A2uiRequestProcessor::new(
            vec![CatalogDocument::basic(version).unwrap()],
            Vec::new(),
            std::sync::Arc::new(format),
        )
        .unwrap()
    };
    let v1_0 = chunks("1.0", BASIC, "1");
    let v0_9 = chunks(
        "v0.9",
        "https://a2ui.org/specification/v0_9/catalogs/basic/catalog.json",
        "1",
    );

    // v1.0: the creation, nothing while the component is open, the
    // component once it closes, the data.
    assert_eq!(
        emitted(&progressive(A2uiVersion::V1_0, None), &v1_0),
        [1, 0, 1, 1]
    );
    // v0.9: the component shows while its text is still being written.
    assert_eq!(
        emitted(&progressive(A2uiVersion::V0_9, None), &v0_9),
        [1, 1, 1, 1]
    );
    // Either can be told to do as the other does.
    assert_eq!(
        emitted(&progressive(A2uiVersion::V1_0, Some(false)), &v1_0),
        [1, 1, 1, 1]
    );
    assert_eq!(
        emitted(&progressive(A2uiVersion::V0_9, Some(true)), &v0_9),
        [1, 0, 1, 1]
    );
}

#[test]
fn a_payload_is_checked_on_its_own_in_v1_0_too() {
    let catalogs: Vec<CatalogDocument> = processor().active_catalogs().to_vec();
    // An update to a surface the payload does not create: assumed to exist.
    let update = json!([{"version": "1.0", "updateComponents": {"surfaceId": "known", "components": [
        {"id": "extra", "component": "Gauge", "@catalogId": CHARTS, "value": 1}
    ]}}]);
    validate_payload(&catalogs, update.as_array().unwrap()).unwrap();

    let wrong = json!([{"version": "1.0", "updateComponents": {"surfaceId": "known", "components": [
        {"id": "extra", "component": "Gauge", "@catalogId": "example.com:maps", "value": 1}
    ]}}]);
    let error = validate_payload(&catalogs, wrong.as_array().unwrap()).unwrap_err();
    assert_eq!(
        located(&error),
        [(
            ErrorCode::UnknownCatalog,
            "messages[0].updateComponents.components[0].@catalogId"
        )]
    );
}

/// A catalog with a function the agent may call on the renderer, and one the
/// agent runs for it.
fn device() -> CatalogConfig {
    let catalog = CatalogDocument::parse(
        r#"{
          "catalogId": "example.com:device",
          "protocolVersion": "1.0",
          "components": {},
          "functions": {
            "getLocale": {"returnType": "string", "allowedCallers": "rendererOrAgent"},
            "formatLocal": {"returnType": "string"}
          }
        }"#,
        None,
        None,
    )
    .unwrap()
    .with_agent_functions([AgentFunction::new(
        "fetchFxRate",
        json!({
            "type": "object",
            "properties": {"base": {"type": "string"}, "target": {"type": "string", "default": "USD"}},
            "required": ["base"],
            "additionalProperties": false
        }),
    )
    .returning("number")
    .described("Current exchange rate from base to target.")])
    .unwrap();
    CatalogConfig::new(catalog)
}

fn device_processor() -> A2uiRequestProcessor {
    let renderer = RendererCapabilities::supporting(A2uiVersion::V1_0, ["example.com:device"]);
    A2uiGenerator::new(vec![device()])
        .create_processor(Some(&renderer))
        .unwrap()
}

/// In a v1.0 session the prompt shows `callRendererFunction`, so an answer may
/// carry one under an id of the model's choosing. It is checked like the rest
/// of the answer, and the table that waits on the agent's own calls waits on
/// it too, so that its response has somewhere to land.
#[test]
fn a_call_the_model_wrote_is_checked_sent_and_waited_on() {
    let processor = device_processor();
    assert!(processor.prompt_snippet().contains("callRendererFunction"));
    let answer = r#"One moment.
<a2ui-json>[{"version": "1.0", "callRendererFunction": {
  "functionCallId": "locale-from-the-model",
  "callFunction": {"@call": "getLocale", "catalogId": "example.com:device", "args": {}}
}}]</a2ui-json>"#;
    let messages: Vec<AgentMessage> = processor
        .parse_response(answer)
        .unwrap()
        .into_iter()
        .flat_map(|part| match part {
            ResponsePart::A2ui(messages) => messages,
            ResponsePart::Text(_) => Vec::new(),
        })
        .collect();
    assert_eq!(messages.len(), 1);

    let mut calls = processor.renderer_calls().unwrap();
    let now = Duration::from_secs(5);
    // Every message of the answer is offered; the calls among them are kept.
    for message in &messages {
        assert!(calls.track(message, now).unwrap());
    }
    assert!(calls.is_pending("locale-from-the-model"));
    // The same rule as for a call the agent makes: one id, one call.
    assert_eq!(
        calls.track(&messages[0], now).unwrap_err().code,
        code::DUPLICATE
    );
    let own = calls
        .call(RendererCall::new("example.com:device", "getLocale"), now)
        .unwrap();
    assert_eq!(
        serde_json::to_value(&own).unwrap()["callRendererFunction"]["functionCallId"],
        "call-1"
    );

    // The renderer answers the model's call; the router reads the answer and
    // the table settles it.
    let router = processor.renderer_messages().unwrap();
    let Routed::FunctionResponse(response) = router
        .route(&json!({"version": "1.0", "rendererFunctionResponse": {
            "functionCallId": "locale-from-the-model", "value": "fr-FR"
        }}))
        .unwrap()
    else {
        panic!("a response");
    };
    let settled = calls.resolve(&response).unwrap();
    assert_eq!(settled.outcome.value(), Some(&json!("fr-FR")));
    assert!(
        calls.resolve(&response).is_none(),
        "a second answer lands nowhere"
    );
    // The agent's own call waits its thirty seconds and no longer.
    let late = calls.expire(now + Duration::from_secs(30));
    assert_eq!(late[0].outcome.error().unwrap().code, code::TIMEOUT);

    // A call the model may not make is refused with the rest of the answer.
    let refused = processor
        .parse_response(
            r#"<a2ui-json>[{"version": "1.0", "callRendererFunction": {
              "functionCallId": "c", "callFunction": {"@call": "formatLocal", "catalogId": "example.com:device"}
            }}]</a2ui-json>"#,
        )
        .unwrap_err();
    assert!(
        refused.error.to_string().contains("rendererOnly"),
        "{}",
        refused.error
    );
}

/// A function the agent declared is in the prompt and in what validates, and
/// a renderer's call to it comes back as a value for the application to run.
#[test]
fn a_function_the_agent_runs_is_declared_called_and_answered() {
    let processor = device_processor();
    assert!(processor.prompt_snippet().contains("fetchFxRate"));
    // The agent runs it, so the agent does not ask the renderer to.
    let mut calls = processor.renderer_calls().unwrap();
    let asked = calls
        .call(
            RendererCall::new("example.com:device", "fetchFxRate"),
            Duration::ZERO,
        )
        .unwrap_err();
    assert_eq!(asked.code, code::UNKNOWN_FUNCTION);

    let router = processor.renderer_messages().unwrap();
    let call = |args: serde_json::Value| {
        json!({"version": "1.0", "callAgentFunction": {
            "surfaceId": "checkout",
            "functionCallId": "fx-1",
            "callFunction": {"@call": "fetchFxRate", "catalogId": "example.com:device", "args": args}
        }})
    };
    let Routed::Call(handed) = router.route(&call(json!({"base": "EUR"}))).unwrap() else {
        panic!("a call to run");
    };
    assert_eq!(
        (
            handed.name.as_str(),
            handed.surface_id.as_str(),
            handed.catalog_id.as_str()
        ),
        ("fetchFxRate", "checkout", "example.com:device")
    );
    // The application runs whatever it likes, and says how it went.
    let answered = serde_json::to_value(handed.value(json!(1.08))).unwrap();
    assert_eq!(
        answered,
        json!({"version": "1.0", "agentFunctionResponse": {"functionCallId": "fx-1", "value": 1.08}})
    );
    let failed =
        serde_json::to_value(handed.error(RpcError::new("PROVIDER_NOT_FOUND", "no rate"))).unwrap();
    assert_eq!(
        failed["agentFunctionResponse"]["error"]["code"],
        "PROVIDER_NOT_FOUND"
    );

    // What the agent does not run it answers for itself, without running
    // anything.
    let Routed::Reply(reply) = router
        .route(&json!({"version": "1.0", "callAgentFunction": {
            "surfaceId": "checkout", "functionCallId": "x-1",
            "callFunction": {"@call": "getLocale", "catalogId": "example.com:device"}
        }}))
        .unwrap()
    else {
        panic!("an answer");
    };
    assert_eq!(
        serde_json::to_value(reply).unwrap()["agentFunctionResponse"]["error"]["code"],
        code::UNKNOWN_FUNCTION
    );
}
