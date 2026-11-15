//! Function calls between an agent and a renderer, at the places a review of
//! the first version found them wrong.
//!
//! Each test states one rule: an id the table gives is new to the table; a
//! call that leaves its arguments out passes none; what `call` refuses,
//! `track` and the check of an answer refuse; a renderer answers an argument
//! it cannot resolve with `EXECUTION_ERROR`; a refusal is reported in the
//! version of the message it refuses; a catalog that cannot take an agent
//! function is an error and not a panic; a refused call takes nothing.

#![cfg(feature = "agent")]

use std::time::Duration;

use ag_ui_a2ui::agent::{
    AgentFunction, CatalogDocument, RendererCall, RendererCalls, validate_payload,
};
use ag_ui_a2ui::message::{CallRendererFunction, FunctionCall, FunctionResponse};
use ag_ui_a2ui::rpc::{PendingCall, PendingCalls, code};
use ag_ui_a2ui::{A2uiVersion, AgentMessage, Error, MessageProcessor};
use serde_json::{Value, json};

const DEVICE: &str = "example.com:device";
const T0: Duration = Duration::from_secs(100);

/// A catalog with functions of each kind of caller, one of which requires an
/// argument.
fn device() -> CatalogDocument {
    CatalogDocument::new(json!({
        "catalogId": DEVICE,
        "protocolVersion": "1.0",
        "components": {
            "Text": {"type": "object", "properties": {
                "text": {"$ref": "common_types.json#/$defs/DynamicString"}
            }}
        },
        "functions": {
            "getLocale": {"returnType": "string", "allowedCallers": "rendererOrAgent"},
            "getLocation": {"returnType": "object", "allowedCallers": "rendererOrAgent"},
            "lookup": {"returnType": "string", "allowedCallers": "rendererOrAgent"},
            "echoArgs": {"returnType": "object", "allowedCallers": "rendererOrAgent"},
            "formatLocal": {"returnType": "string"},
            "playMedia": {
                "returnType": "void",
                "allowedCallers": "rendererOrAgent",
                "properties": {"args": {
                    "type": "object",
                    "properties": {"mediaId": {"type": "string"}},
                    "required": ["mediaId"],
                    "additionalProperties": false
                }}
            },
            "label": {
                "returnType": "string",
                "properties": {"args": {
                    "type": "object",
                    "properties": {"value": {"type": "string"}},
                    "required": ["value"],
                    "additionalProperties": false
                }}
            }
        }
    }))
    .unwrap()
}

fn processor() -> MessageProcessor {
    let mut processor = MessageProcessor::new();
    processor.register(device().catalog().clone()).unwrap();
    processor
}

fn call_message(function_call_id: &str, call_function: Value) -> Value {
    json!({"version": "1.0", "callRendererFunction": {
        "functionCallId": function_call_id, "callFunction": call_function
    }})
}

fn id_of(message: &AgentMessage) -> String {
    serde_json::to_value(message).unwrap()["callRendererFunction"]["functionCallId"]
        .as_str()
        .unwrap()
        .to_string()
}

/// F1. An id the table gives is one the table has not had, whoever gave the
/// earlier one, so a late answer to a call that ended does not land on a
/// later call.
#[test]
fn an_id_the_table_gives_is_one_it_has_not_had() {
    // A call a model wrote, under an id of the form the table's own have.
    let mut calls = RendererCalls::new();
    let written = AgentMessage::call_renderer_function(CallRendererFunction::new(
        "call-2",
        FunctionCall::new("getLocation").with_catalog_id(DEVICE),
    ));
    assert!(calls.track(&written, T0).unwrap());
    let late = calls.expire(T0 + Duration::from_secs(30));
    assert_eq!(late[0].outcome.error().unwrap().code, code::TIMEOUT);

    // The agent's own calls count on from it.
    let later = T0 + Duration::from_secs(60);
    let first = calls
        .call(RendererCall::new(DEVICE, "getLocale"), later)
        .unwrap();
    let second = calls
        .call(RendererCall::new(DEVICE, "getLocale"), later)
        .unwrap();
    assert_eq!([id_of(&first), id_of(&second)], ["call-3", "call-4"]);
    // The answer to the model's call comes after all: nothing waits on it.
    let answer = FunctionResponse::value("call-2", json!({"city": "Paris"}));
    assert_eq!(calls.resolve(&answer), None);
    assert_eq!(calls.pending_count(), 2);

    // The same for an id a caller gave.
    let mut calls = RendererCalls::new();
    calls
        .call(
            RendererCall::new(DEVICE, "getLocation").with_id("call-1"),
            T0,
        )
        .unwrap();
    calls.expire(T0 + Duration::from_secs(30));
    let own = calls
        .call(RendererCall::new(DEVICE, "getLocale"), later)
        .unwrap();
    assert_eq!(id_of(&own), "call-2");
    assert_eq!(
        calls.resolve(&FunctionResponse::value("call-1", json!("late"))),
        None
    );

    // And in the table itself.
    let mut table = PendingCalls::new();
    table
        .register(PendingCall::new("call-7", "given"), T0)
        .unwrap();
    table
        .settle(&FunctionResponse::value("call-7", json!(1)))
        .unwrap();
    assert_eq!(table.next_id(), "call-8");
}

/// F1. Two tables count from the same place, so two of them in one
/// conversation need prefixes of their own.
#[test]
fn a_prefix_keeps_the_ids_of_two_tables_apart() {
    let mut first = RendererCalls::new().with_id_prefix("run-1-");
    let mut second = RendererCalls::new().with_id_prefix("run-2-");
    let a = first
        .call(RendererCall::new(DEVICE, "getLocale"), T0)
        .unwrap();
    let b = second
        .call(RendererCall::new(DEVICE, "getLocale"), T0)
        .unwrap();
    assert_eq!([id_of(&a), id_of(&b)], ["run-1-1", "run-2-1"]);
    // An id of another form does not move the count.
    first
        .call(
            RendererCall::new(DEVICE, "getLocale").with_id("call-40"),
            T0,
        )
        .unwrap();
    let c = first
        .call(RendererCall::new(DEVICE, "getLocale"), T0)
        .unwrap();
    assert_eq!(id_of(&c), "run-1-2");

    // No id that is given can use the count up: one of the table's own form
    // with a number past half of what the table counts to is refused.
    let mut table = PendingCalls::new();
    let far = format!("call-{}", u64::MAX);
    let refused = table
        .register(PendingCall::new(far, "given"), T0)
        .unwrap_err();
    assert_eq!(refused.code, code::INVALID_FUNCTION_CALL);
    assert_eq!(table.next_id(), "call-1");
    // A number that is no number of the table's is an id like any other.
    table
        .register(
            PendingCall::new("call-99999999999999999999999", "given"),
            T0,
        )
        .unwrap();
    table
        .register(PendingCall::new("call-+5", "given"), T0)
        .unwrap();
    assert_eq!(table.next_id(), "call-1");
}

/// F2. A call that leaves `args` out passes no arguments, and is held to what
/// its function requires as one that passes `{}` is: at the call of a
/// `callRendererFunction`, at a call nested in its arguments, and at a call in
/// a component.
#[test]
fn a_call_that_leaves_its_arguments_out_passes_none() {
    let mut processor = processor();
    let refusal = |processor: &mut MessageProcessor, message: Value| -> String {
        match processor.process(&message) {
            Ok(()) => "accepted".to_string(),
            Err(refused) => refused.error.to_string(),
        }
    };
    for call in [
        json!({"@call": "playMedia", "catalogId": DEVICE, "args": {}}),
        json!({"@call": "playMedia", "catalogId": DEVICE}),
    ] {
        let said = refusal(&mut processor, call_message("c-1", call.clone()));
        assert!(
            said.contains("requires the argument 'mediaId'"),
            "{call}: {said}"
        );
    }
    for nested in [
        json!({"@call": "label", "args": {}}),
        json!({"@call": "label"}),
    ] {
        let call = json!({"@call": "echoArgs", "catalogId": DEVICE, "args": {"text": nested}});
        let said = refusal(&mut processor, call_message("c-2", call.clone()));
        assert!(
            said.contains("requires the argument 'value'"),
            "{call}: {said}"
        );
    }
    processor
        .process(
            &json!({"version": "1.0", "createSurface": {"surfaceId": "s", "catalogId": DEVICE}}),
        )
        .unwrap();
    for text in [
        json!({"@call": "label", "args": {}}),
        json!({"@call": "label"}),
    ] {
        let update = json!({"version": "1.0", "updateComponents": {"surfaceId": "s", "components": [
            {"id": "root", "component": "Text", "text": text}
        ]}});
        let said = refusal(&mut processor, update);
        assert!(
            said.contains("requires the argument 'value'"),
            "{text}: {said}"
        );
    }
    // A function that requires nothing is called without arguments as before.
    processor
        .process(&call_message(
            "c-3",
            json!({"@call": "getLocale", "catalogId": DEVICE}),
        ))
        .unwrap();
}

/// F3. What `call` refuses, `track` and the check of an answer refuse too: a
/// function the agent runs itself is not one to ask the renderer for, and a
/// message that cannot be sent is not one to wait on.
#[test]
fn a_call_the_agent_would_not_make_is_not_waited_on() {
    // The agent took over a function its catalog declares for either side.
    let catalog = device()
        .with_agent_functions([AgentFunction::declared("lookup")])
        .unwrap();
    let wire = call_message("c-1", json!({"@call": "lookup", "catalogId": DEVICE}));
    let typed: AgentMessage = serde_json::from_value(wire.clone()).unwrap();
    let mut calls = RendererCalls::new()
        .with_catalogs(std::slice::from_ref(&catalog))
        .unwrap();

    let made = calls
        .call(RendererCall::new(DEVICE, "lookup"), T0)
        .unwrap_err();
    assert_eq!(made.code, code::UNKNOWN_FUNCTION);
    let tracked = calls.track(&typed, T0).unwrap_err();
    assert_eq!(tracked.code, code::UNKNOWN_FUNCTION);
    let checked = validate_payload(std::slice::from_ref(&catalog), &[wire]).unwrap_err();
    assert!(matches!(checked, Error::Validation { .. }), "{checked}");
    assert!(checked.to_string().contains("lookup"), "{checked}");
    assert_eq!(calls.pending_count(), 0);

    // The other checks of `call` hold for a call that was written elsewhere.
    for (name, wanted) in [
        ("formatLocal", code::INVALID_FUNCTION_CALL),
        ("vibrate", code::UNKNOWN_FUNCTION),
    ] {
        let message = AgentMessage::call_renderer_function(CallRendererFunction::new(
            "c-2",
            FunctionCall::new(name).with_catalog_id(DEVICE),
        ));
        assert_eq!(
            calls.track(&message, T0).unwrap_err().code,
            wanted,
            "{name}"
        );
    }
    // One the renderer runs is waited on.
    let fine = AgentMessage::call_renderer_function(CallRendererFunction::new(
        "c-3",
        FunctionCall::new("getLocale").with_catalog_id(DEVICE),
    ));
    assert!(calls.track(&fine, T0).unwrap());

    // A message of a version that has no function call cannot be sent.
    let mut unsendable = AgentMessage::call_renderer_function(CallRendererFunction::new(
        "c-4",
        FunctionCall::new("getLocale").with_catalog_id(DEVICE),
    ));
    unsendable.version = A2uiVersion::V0_9_1.as_str().to_string();
    assert!(serde_json::to_value(&unsendable).is_err());
    let mut bare = RendererCalls::new();
    let refused = bare.track(&unsendable, T0).unwrap_err();
    assert_eq!(refused.code, code::INVALID_FUNCTION_CALL);
    assert_eq!(bare.pending_count(), 0);
}

/// F4. A renderer answers a call it does not run with
/// `INVALID_FUNCTION_CALL`, and one whose arguments it cannot resolve with
/// `EXECUTION_ERROR`.
#[test]
fn an_argument_a_renderer_cannot_resolve_is_an_execution_error() {
    let mut processor = processor();
    let answer = |processor: &mut MessageProcessor, call: Value| -> (String, String) {
        let refused = processor.process(&call_message("c-1", call)).unwrap_err();
        let response = serde_json::to_value(refused.renderer_response().unwrap()).unwrap();
        let error = &response["rendererFunctionResponse"]["error"];
        (
            error["code"].as_str().unwrap().to_string(),
            error["message"].as_str().unwrap().to_string(),
        )
    };
    for unresolvable in [
        json!({"idx": {"@call": "@index"}}),
        json!({"value": {"@path": 5}}),
        json!({"value": {"@call": "summonDragon"}}),
        json!({"value": {"inner": {"@bogus": 1}}}),
    ] {
        let call = json!({"@call": "echoArgs", "catalogId": DEVICE, "args": unresolvable});
        let (code, message) = answer(&mut processor, call.clone());
        assert_eq!(code, code::EXECUTION_ERROR, "{call}: {message}");
    }
    for (call, mentions) in [
        (
            json!({"@call": "formatLocal", "catalogId": DEVICE}),
            "rendererOnly",
        ),
        (
            json!({"@call": "vibrate", "catalogId": DEVICE}),
            "Function not found",
        ),
        (
            json!({"@call": "playMedia", "catalogId": DEVICE, "args": {}}),
            "mediaId",
        ),
    ] {
        let (code, message) = answer(&mut processor, call.clone());
        assert_eq!(code, code::INVALID_FUNCTION_CALL, "{call}: {message}");
        assert!(message.contains(mentions), "{call}: {message}");
    }
    // A call that is refused for itself is not run, so what its arguments
    // hold is not what the renderer answers about.
    let both =
        json!({"@call": "formatLocal", "catalogId": DEVICE, "args": {"i": {"@call": "@index"}}});
    assert_eq!(answer(&mut processor, both).0, code::INVALID_FUNCTION_CALL);
}

/// F5. What a renderer sends back for a refusal is a message of the version
/// of the message it refused, and valid as one.
#[cfg(feature = "schema-validation")]
#[test]
fn a_refusal_is_reported_as_the_version_of_its_message_writes_one() {
    use ag_ui_a2ui::client_schema::ClientSchemaValidator;

    let call = |version: &str| {
        json!({"version": version, "callRendererFunction": {
            "functionCallId": "c-1",
            "callFunction": {"@call": "formatLocal", "catalogId": DEVICE}
        }})
    };
    for version in A2uiVersion::ALL {
        let schema = ClientSchemaValidator::for_version(*version).unwrap();
        let stamp = version.as_str();
        let refusals = [
            // A function call, which only v1.0 has.
            call(stamp),
            // A call that is no call.
            json!({"version": stamp, "callRendererFunction": {"functionCallId": "c-2"}}),
            // The life of a surface, a catalog nobody holds, a bad envelope.
            json!({"version": stamp, "updateDataModel": {"surfaceId": "s", "path": "/a", "value": 1}}),
            json!({"version": stamp, "createSurface": {"surfaceId": "s", "catalogId": "nobody"}}),
            json!({"version": stamp, "deleteSurface": {}}),
        ];
        for refused in refusals {
            let mut processor = processor();
            let rejection = processor.process(&refused).unwrap_err();
            let error = serde_json::to_value(rejection.renderer_error()).unwrap();
            assert_eq!(error["version"], stamp, "{refused}");
            if let Err(invalid) = schema.validate_message(&error) {
                panic!("{version}: {refused}\nsent {error}\n{invalid}");
            }
            if let Some(response) = rejection.renderer_response() {
                assert!(version.carries_function_calls(), "{version}: {refused}");
                let response = serde_json::to_value(response).unwrap();
                schema.validate_message(&response).unwrap();
            }
        }
    }
    // Before v1.0 there is no call to report by its id: the message carries
    // no operation of its version.
    let mut processor = processor();
    let rejection = processor.process(&call("v0.9")).unwrap_err();
    let error = serde_json::to_value(rejection.renderer_error()).unwrap();
    assert_eq!(error["error"]["code"], "VALIDATION_FAILED");
    assert_eq!(error["error"]["surfaceId"], "");
    assert!(error["error"].get("functionCallId").is_none(), "{error}");
}

/// F6. A catalog document whose definitions are not an object cannot take a
/// function that brings definitions of its own. That is an error.
#[test]
fn a_catalog_that_cannot_take_an_agent_function_is_an_error() {
    let function = || {
        AgentFunction::new(
            "pay",
            json!({
                "type": "object",
                "$defs": {"Money": {"type": "number"}},
                "properties": {"total": {"$ref": "#/$defs/Money"}}
            }),
        )
    };
    for odd in [json!([]), json!("text"), json!(7), json!(null)] {
        let document = CatalogDocument::new(json!({
            "catalogId": "example.com:odd",
            "protocolVersion": "1.0",
            "components": {},
            "functions": {},
            "$defs": odd
        }));
        // Whether such a document loads is the loader's to say; if it does,
        // declaring a function on it must not panic.
        let Ok(document) = document else { continue };
        let declared = document.with_agent_functions([function()]);
        assert!(
            matches!(declared, Err(Error::Catalog(_))),
            "{odd}: {declared:?}"
        );
    }
}

/// F7. A call that is refused keeps nothing: not a place in the table, and
/// not an id.
#[test]
fn a_call_refused_for_its_timeout_takes_no_id() {
    let mut calls = RendererCalls::new();
    let refused = calls
        .call(
            RendererCall::new(DEVICE, "getLocale").with_timeout(Duration::ZERO),
            T0,
        )
        .unwrap_err();
    assert_eq!(refused.code, code::INVALID_FUNCTION_CALL);
    assert_eq!(calls.pending_count(), 0);
    let made = calls
        .call(RendererCall::new(DEVICE, "getLocale"), T0)
        .unwrap();
    assert_eq!(id_of(&made), "call-1");
    // The timeout is what is said first, before the name or the catalog.
    let refused = calls
        .call(RendererCall::new("", "").with_timeout(Duration::ZERO), T0)
        .unwrap_err();
    assert!(refused.message.contains("timeout"), "{}", refused.message);
}
