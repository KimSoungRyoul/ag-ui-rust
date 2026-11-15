//! The runner for `core/rpc_functions.yaml` (`handle_rpc`): the renderer's
//! side of a function call.
//!
//! This crate mirrors a renderer and is none. It checks a
//! `callRendererFunction` as a renderer checks one before running anything,
//! and answers a call it refuses; it keeps the table of the calls a side has
//! made. It runs no function, holds no listener and evaluates nothing on a
//! surface. A case that needs one of those is skipped, with which.

use std::time::Duration;

use ag_ui_a2ui::catalog::{AllowedCallers, Catalog, FunctionDef};
use ag_ui_a2ui::message::{CallAgentFunction, RendererMessage, RendererPayload};
use ag_ui_a2ui::processor::MessageProcessor;
use ag_ui_a2ui::rpc::{FunctionOutcome, PendingCall, PendingCalls};
use ag_ui_a2ui::{A2uiVersion, AgentMessage, AgentPayload};
use serde_json::Value;

use super::Miss;

/// The label of a case that is the renderer's own to pass.
const RENDERER: &str = "out of scope: renderer";

/// Why a case of the suite is not one this crate can be held to, when it is
/// not.
pub(crate) fn skip(case: &Value) -> Option<String> {
    let args = case.get("args").unwrap_or(&Value::Null);
    let expected = case
        .pointer("/expect/response/rendererFunctionResponse")
        .unwrap_or(&Value::Null);
    let on_a_surface = ["uiBindingScenario", "uiActionFallbackScenario"]
        .iter()
        .any(|key| args.get(*key).is_some());
    let why = if on_a_surface {
        "evaluating a function call in a binding, a check rule or an action on a surface"
    } else if args.get("outboundListener") == Some(&Value::Bool(false)) {
        "the outbound listener of a renderer, which this crate does not have: a call it would \
         send is a value handed to the caller"
    } else if expected.get("value").is_some() {
        "running a renderer function and answering with its value"
    } else if expected.pointer("/error/code").and_then(Value::as_str) == Some("EXECUTION_ERROR")
        && !resolved_by_the_renderer(
            args.pointer("/message/callRendererFunction/callFunction/args"),
        )
    {
        // An argument a renderer cannot resolve is an `EXECUTION_ERROR` too,
        // and that one the crate answers: the call is refused before it runs.
        "running a renderer function, which fails as it runs"
    } else if args
        .pointer("/functionMetadata")
        .and_then(Value::as_object)
        .is_some_and(|functions| {
            functions
                .values()
                .any(|stated| stated.get("parameters").is_some())
        })
    {
        return Some(
            "later: schema validation in v1.0 (#97): the arguments of a call are held to the \
             names their function declares, and their types to a JSON Schema the processor \
             does not apply to a call"
                .to_string(),
        );
    } else {
        return None;
    };
    Some(format!("{RENDERER}: {why}"))
}

/// Whether arguments hold something a renderer resolves before it runs the
/// function: a binding, or a call.
fn resolved_by_the_renderer(args: Option<&Value>) -> bool {
    let mut stack: Vec<&Value> = args.into_iter().collect();
    while let Some(value) = stack.pop() {
        match value {
            Value::Array(items) => stack.extend(items),
            Value::Object(map) => {
                if map.contains_key("@call") || map.contains_key("@path") {
                    return true;
                }
                stack.extend(map.values());
            }
            _ => {}
        }
    }
    false
}

fn text<'a>(value: &'a Value, pointer: &str) -> Option<&'a str> {
    value.pointer(pointer).and_then(Value::as_str)
}

/// The catalog a case registers, by the convention of the reference harness:
/// the id the case states, or the one its message names unless the case is
/// about a catalog that is not there, or `basic`; the functions are the ones
/// `functionMetadata` lists, built in code.
fn catalog(case: &Value) -> Result<Catalog, Miss> {
    let args = case.get("args").unwrap_or(&Value::Null);
    let named = text(args, "/message/callRendererFunction/callFunction/catalogId");
    let wants_missing = text(
        case,
        "/expect/response/rendererFunctionResponse/error/message",
    )
    .is_some_and(|message| message.contains("Catalog not found"));
    let id = text(args, "/catalogId")
        .or(named.filter(|_| !wants_missing))
        .or_else(|| text(args, "/outboundCall/callFunction/catalogId"))
        .unwrap_or("basic");
    let stated = text(args, "/catalogVersion")
        .or_else(|| text(case, "/catalog/protocolVersion"))
        .unwrap_or("1.0");
    let Some(version) = A2uiVersion::from_number(stated) else {
        return Err(Miss::new(
            format!("the case's catalog version {stated} is not one this crate speaks"),
            "a catalog to register",
        ));
    };
    let mut catalog = Catalog::empty(id).with_version(version);
    for (name, stated) in args
        .get("functionMetadata")
        .and_then(Value::as_object)
        .into_iter()
        .flatten()
    {
        let mut function = FunctionDef::new(name);
        if let Some(callers) = stated
            .get("allowedCallers")
            .and_then(Value::as_str)
            .and_then(AllowedCallers::parse)
        {
            function.allowed_callers = callers;
        }
        function.requires_user_activation =
            stated.get("requiresUserActivation") == Some(&Value::Bool(true));
        catalog.functions.insert(name.clone(), function);
    }
    Ok(catalog)
}

/// Whether an error a renderer sends says what a case expects of it: the code
/// exactly, and the message as text the one sent contains.
fn says(expected: &Value, sent: &Value) -> bool {
    let code = expected
        .get("code")
        .is_none_or(|code| Some(code) == sent.get("code"));
    let id = expected
        .get("functionCallId")
        .is_none_or(|id| Some(id) == sent.get("functionCallId"));
    let message = expected
        .get("message")
        .and_then(Value::as_str)
        .is_none_or(|wanted| {
            sent.get("message")
                .and_then(Value::as_str)
                .is_some_and(|said| said.contains(wanted.trim_end_matches('.')))
        });
    code && id && message
}

/// A message an agent sent, taken in as a renderer takes it.
fn inbound(case: &Value, message: &Value) -> Result<(), Miss> {
    let mut processor = MessageProcessor::new();
    processor.register(catalog(case)?).map_err(|error| {
        Miss::new(
            format!("the case's catalog does not register: {error}"),
            "a registered catalog",
        )
    })?;
    // The surface a case sets up, under the catalog it registers.
    let args = case.get("args").unwrap_or(&Value::Null);
    if let Some(surface_id) = text(args, "/surfaceId") {
        let mut create = serde_json::json!({
            "surfaceId": surface_id,
            "catalogId": catalog(case)?.catalog_id,
        });
        if let Some(data) = args.get("dataModel").filter(|data| data.is_object()) {
            create["dataModel"] = data.clone();
        }
        processor
            .process(&serde_json::json!({"version": "1.0", "createSurface": create}))
            .map_err(|refused| {
                Miss::new(
                    format!("the case's surface was refused: {}", refused.error),
                    "a surface to call on",
                )
            })?;
    }
    let outcome = processor.process(message);
    let wants_response = case.pointer("/expect/response/rendererFunctionResponse");
    let wants_error = case
        .pointer("/expectError/rendererError/error")
        .or_else(|| case.pointer("/expect/error"));
    match (outcome, wants_response, wants_error) {
        // An answer nothing waits on, or a call a renderer would go on to
        // run: taken, and nothing is sent back.
        (Ok(()), None, None) => Ok(()),
        (Ok(()), _, _) => Err(Miss::new(
            "the message was accepted",
            format!("{}", case.get("expect").unwrap_or(&Value::Null)),
        )),
        // A call that reads and is not run is answered.
        (Err(refused), Some(expected), _) => {
            let Some(response) = refused.renderer_response() else {
                return Err(Miss::new(
                    format!("refused without an answer: {}", refused.error),
                    format!("{expected}"),
                ));
            };
            let sent = serde_json::to_value(&response).unwrap_or(Value::Null);
            let body = &sent["rendererFunctionResponse"];
            let same_call = expected.get("functionCallId") == body.get("functionCallId");
            let expected_error = expected.get("error").unwrap_or(&Value::Null);
            if sent["version"] == "1.0" && same_call && says(expected_error, &body["error"]) {
                Ok(())
            } else {
                Err(Miss::new(format!("{sent}"), format!("{expected}")))
            }
        }
        // A message that is not a call is reported, by the id it has.
        (Err(refused), None, Some(expected)) => {
            if let Some(response) = refused.renderer_response() {
                let sent = serde_json::to_value(&response).unwrap_or(Value::Null);
                return Err(Miss::new(
                    format!("answered as a call: {sent}"),
                    format!("an error message: {expected}"),
                ));
            }
            let sent = serde_json::to_value(refused.renderer_error()).unwrap_or(Value::Null);
            if sent["version"] == "1.0" && says(expected, &sent["error"]) {
                Ok(())
            } else {
                Err(Miss::new(format!("{sent}"), format!("{expected}")))
            }
        }
        (Err(refused), None, None) => Err(Miss::new(
            format!("refused: {}", refused.error),
            "the message taken, and nothing sent",
        )),
    }
}

/// The `callAgentFunction` a renderer host sends for a call it registered,
/// and the entry it registers.
fn outbound(call: &Value) -> Result<(RendererMessage, PendingCall), Miss> {
    let typed: CallAgentFunction = serde_json::from_value(serde_json::json!({
        "surfaceId": call.get("surfaceId"),
        "functionCallId": call.get("functionCallId"),
        "callFunction": call.get("callFunction"),
    }))
    .map_err(|error| {
        Miss::new(
            format!("the outbound call does not read: {error}"),
            "a call",
        )
    })?;
    let mut pending = PendingCall::new(
        typed.function_call_id.clone(),
        typed.call_function.call.clone(),
    )
    .on_surface(typed.surface_id.clone());
    pending.timeout = call
        .get("timeoutMs")
        .and_then(Value::as_u64)
        .map(Duration::from_millis);
    let message = RendererMessage::new(RendererPayload::CallAgentFunction(typed))
        .with_version(A2uiVersion::V1_0);
    Ok((message, pending))
}

/// An `agentFunctionResponse` as the agent sent it, read through the
/// processor and handed to the table.
fn answer(
    calls: &mut PendingCalls,
    message: &Value,
) -> Result<Option<(String, FunctionOutcome)>, Miss> {
    MessageProcessor::new()
        .process(message)
        .map_err(|refused| {
            Miss::new(
                format!("the response was refused: {}", refused.error),
                "taken",
            )
        })?;
    let typed: AgentMessage = serde_json::from_value(message.clone())
        .map_err(|error| Miss::new(format!("the response does not read: {error}"), "a response"))?;
    let AgentPayload::AgentFunctionResponse(response) = typed.payload else {
        return Err(Miss::new("not an agentFunctionResponse", "a response"));
    };
    Ok(calls
        .settle(&response)
        .map(|settled| (settled.function_call_id, settled.outcome)))
}

fn code_of(outcome: &FunctionOutcome) -> &str {
    outcome
        .error()
        .map_or("no error", |error| error.code.as_str())
}

/// A call a renderer host makes to the agent, and what becomes of it.
fn outbound_case(case: &Value, call: &Value) -> Result<(), Miss> {
    let args = case.get("args").unwrap_or(&Value::Null);
    let start = Duration::from_secs(1_000);
    let mut calls = PendingCalls::new();
    let (message, pending) = outbound(call)?;
    let timeout = pending.timeout;
    let surface_id = pending.surface_id.clone().unwrap_or_default();
    // What the host sends is a message of v1.0.
    serde_json::to_value(&message)
        .map_err(|error| Miss::new(format!("the call does not serialize: {error}"), "a message"))?;
    calls
        .register(pending, start)
        .map_err(|error| Miss::new(format!("the call was refused: {error}"), "registered"))?;
    let wanted = text(case, "/expectError/code");

    if let Some(second) = args.get("secondOutboundCall") {
        let (_, again) = outbound(second)?;
        return match calls.register(again, start) {
            Err(error) if Some(error.code.as_str()) == wanted => Ok(()),
            other => Err(Miss::new(format!("{other:?}"), format!("{wanted:?}"))),
        };
    }
    if let Some(response) = args.get("inboundResponse") {
        let Some((id, outcome)) = answer(&mut calls, response)? else {
            return Err(Miss::new(
                "the response matched no pending call",
                "a settled call",
            ));
        };
        let correlated = text(case, "/expect/correlatedCallId").is_none_or(|wanted| wanted == id);
        let result = case
            .pointer("/expect/result")
            .is_none_or(|wanted| Some(wanted) == outcome.value());
        return if correlated && result && calls.pending_count() == 0 {
            Ok(())
        } else {
            Err(Miss::new(
                format!("call {id}: {outcome:?}"),
                format!("{}", case.get("expect").unwrap_or(&Value::Null)),
            ))
        };
    }
    // How the call ends without an answer.
    let ended = match text(args, "/afterCall") {
        Some("cancel") => calls.cancel_surface(&surface_id, "the surface's calls were cancelled"),
        Some("dispose") => calls.dispose("the calls were disposed of"),
        Some(other) => {
            return Err(Miss::new(
                format!("unknown afterCall {other}"),
                "cancel or dispose",
            ));
        }
        None => match timeout {
            Some(timeout) => calls.expire(start + timeout),
            None => {
                return Err(Miss::new(
                    "nothing ends the call",
                    "a timeout, a cancel or a dispose",
                ));
            }
        },
    };
    let [ended] = ended.as_slice() else {
        return Err(Miss::new(
            format!("{} calls ended", ended.len()),
            "the one call",
        ));
    };
    let in_flight = code_of(&ended.outcome);
    let disposed = text(args, "/afterCall") == Some("dispose");
    if disposed {
        // The call in flight is cancelled, and the same call made again is
        // refused.
        let wanted_in_flight = text(case, "/expect/inFlightError/code");
        if wanted_in_flight.is_some_and(|wanted| wanted != in_flight) {
            return Err(Miss::new(in_flight, format!("{wanted_in_flight:?}")));
        }
        let (_, again) = outbound(call)?;
        match calls.register(again, start) {
            Err(error) if Some(error.code.as_str()) == wanted => {}
            other => return Err(Miss::new(format!("{other:?}"), format!("{wanted:?}"))),
        }
    } else if Some(in_flight) != wanted {
        return Err(Miss::new(in_flight, format!("{wanted:?}")));
    }
    if let Some(late) = args.get("lateInboundResponse") {
        let taken = answer(&mut calls, late)?;
        let ignored = taken.is_none() && calls.pending_count() == 0;
        if case.pointer("/expect/lateResponseIgnored") == Some(&Value::Bool(true)) && !ignored {
            return Err(Miss::new(
                format!("the late response settled {taken:?}"),
                "ignored",
            ));
        }
    }
    Ok(())
}

pub(crate) fn run(case: &Value) -> Result<(), Miss> {
    let args = case.get("args").unwrap_or(&Value::Null);
    if let Some(call) = args.get("outboundCall") {
        return outbound_case(case, call);
    }
    let Some(message) = args.get("message") else {
        return Err(Miss::new(
            "the case has neither a message nor an outbound call",
            "one",
        ));
    };
    if message.get("agentFunctionResponse").is_some() {
        // A response nothing waits on: taken, settles nothing, and nothing
        // is sent back.
        let mut calls = PendingCalls::new();
        let settled = answer(&mut calls, message)?;
        return match (settled, case.pointer("/expect/response")) {
            (None, Some(Value::Null) | None) => Ok(()),
            (settled, expected) => Err(Miss::new(format!("{settled:?}"), format!("{expected:?}"))),
        };
    }
    inbound(case, message)
}
