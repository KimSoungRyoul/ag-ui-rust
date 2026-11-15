//! Runners for the function-call suites of the agent tier:
//! `agent/agent_functions.yaml` and `agent/renderer_function_calls.yaml`.
//!
//! The crate carries a call and routes it, and runs no function. Where a case
//! scripts a handler (`returns`, `returns_args`, `returns_context`, `raises`),
//! the runner stands where the application stands: it is handed the call,
//! does what the script says, and gives the outcome back for the message to
//! send. Where a case moves a clock, the runner's clock is the `now` the
//! crate is told.

use std::collections::BTreeMap;
use std::time::Duration;

use ag_ui_a2ui::agent::{
    AgentFunction, AgentFunctionCall, CatalogDocument, RendererCall, RendererCalls,
    RendererMessageKind, RendererMessageRouter, Routed, validate_payload,
};
use ag_ui_a2ui::catalog::{AllowedCallers, Catalog, FunctionDef};
use ag_ui_a2ui::message::{RendererMessage, RendererPayload};
use ag_ui_a2ui::rpc::{FunctionOutcome, RpcError, SettledCall};
use ag_ui_a2ui::{A2uiVersion, AgentMessage, Error};
use serde_json::{Value, json};

use super::{Step, case_keys, category, expect_error, fail, known_keys, same, settle};

/// A catalog of a case with the agent functions declared on it, and the
/// scripted behaviour of each by name.
struct Declared {
    catalog: CatalogDocument,
    behaviors: BTreeMap<String, Value>,
}

/// Builds one `AgentFunctionCatalog` entry: the catalog, then the agent
/// functions the agent adds to its copy of it.
fn declared(entry: &Value) -> Result<Declared, Error> {
    let stop = |stop: super::Stop| match stop {
        super::Stop::Fail(detail) | super::Stop::Skip(detail) => Error::parse(detail),
    };
    known_keys(
        entry,
        &["catalog", "agent_functions"],
        "an agent function catalog",
    )
    .map_err(stop)?;
    // The catalog is loaded here and not by the shared helper, so that a
    // catalog the crate refuses is the crate's error and not the harness's.
    let base = match entry.get("catalog").unwrap_or(entry) {
        Value::String(path) => {
            let document = super::read_fixture(path).map_err(stop)?;
            let fallback_id = document.get("catalogId").is_none().then_some(path.as_str());
            CatalogDocument::read(
                super::conformance_dir().join(path),
                fallback_id,
                super::fixture_version(&document).map_err(stop)?,
            )?
        }
        document @ Value::Object(_) => CatalogDocument::load(
            document.clone(),
            None,
            super::fixture_version(document).map_err(stop)?,
        )?,
        other => return Err(Error::parse(format!("not a catalog entry: {other}"))),
    };
    let mut functions = Vec::new();
    let mut behaviors = BTreeMap::new();
    for (key, declaration) in entry
        .get("agent_functions")
        .and_then(Value::as_object)
        .into_iter()
        .flatten()
    {
        known_keys(
            declaration,
            &[
                "name",
                "description",
                "returnType",
                "args",
                "requiresUserActivation",
                "behavior",
                "from_api",
            ],
            "an agent function",
        )
        .map_err(stop)?;
        let name = declaration
            .get("name")
            .and_then(Value::as_str)
            .unwrap_or(key);
        let function = if declaration.get("from_api") == Some(&Value::Bool(true)) {
            AgentFunction::declared(name)
        } else {
            let mut function = AgentFunction::new(
                name,
                declaration
                    .get("args")
                    .cloned()
                    .unwrap_or(json!({"type": "object"})),
            );
            if let Some(return_type) = declaration.get("returnType").and_then(Value::as_str) {
                function = function.returning(return_type);
            }
            if let Some(description) = declaration.get("description").and_then(Value::as_str) {
                function = function.described(description);
            }
            function.requiring_user_activation(
                declaration.get("requiresUserActivation") == Some(&Value::Bool(true)),
            )
        };
        functions.push(function);
        behaviors.insert(
            name.to_string(),
            declaration.get("behavior").cloned().unwrap_or(Value::Null),
        );
    }
    Ok(Declared {
        catalog: base.with_agent_functions(functions)?,
        behaviors,
    })
}

/// A harness failure that is not the crate's answer: a fixture that does not
/// read. It is told apart from an expected error by its kind.
fn broken(error: &Error) -> Option<String> {
    match error {
        Error::Parse(detail) => Some(detail.to_string()),
        _ => None,
    }
}

fn sorted(names: impl IntoIterator<Item = String>) -> Vec<String> {
    let mut names: Vec<String> = names.into_iter().collect();
    names.sort();
    names
}

fn listed(value: Option<&Value>) -> Vec<String> {
    sorted(
        value
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(Value::as_str)
            .map(str::to_string),
    )
}

pub(super) fn declare_agent_functions(case: &Value, args: &Value) -> Step {
    case_keys(case, &["args", "expect", "expect_error"])?;
    known_keys(args, &["catalog"], "args")?;
    let built = declared(args.get("catalog").unwrap_or(&Value::Null));
    if let Err(error) = &built {
        if let Some(detail) = broken(error) {
            return fail(detail);
        }
    }
    let Some(built) = settle(case, built, "declaring the agent functions")? else {
        return Ok(());
    };
    let expect = case.get("expect").unwrap_or(&Value::Null);
    known_keys(
        expect,
        &["functions", "agent_functions", "document_functions"],
        "expect",
    )?;
    let catalog = &built.catalog;
    for (key, actual) in [
        (
            "functions",
            sorted(catalog.catalog().functions.keys().cloned()),
        ),
        (
            "agent_functions",
            sorted(catalog.agent_functions().map(str::to_string)),
        ),
        (
            "document_functions",
            sorted(
                catalog
                    .document()
                    .get("functions")
                    .and_then(Value::as_object)
                    .into_iter()
                    .flat_map(|functions| functions.keys().cloned()),
            ),
        ),
    ] {
        if let Some(expected) = expect.get(key) {
            let expected = listed(Some(expected));
            if expected != actual {
                return fail(format!("{key}: expected {expected:?}, got {actual:?}"));
            }
        }
    }
    Ok(())
}

/// The catalogs of a case and a router over them that checks schemas.
fn router(entries: &[&Value]) -> Result<(Vec<Declared>, RendererMessageRouter), Error> {
    let built = entries
        .iter()
        .map(|entry| declared(entry))
        .collect::<Result<Vec<_>, _>>()?;
    let documents: Vec<CatalogDocument> = built.iter().map(|built| built.catalog.clone()).collect();
    let router = RendererMessageRouter::new(&documents)?.checking_schemas()?;
    Ok((built, router))
}

/// Does what a case scripts in place of a handler, and gives the message the
/// crate writes for the outcome.
fn run_behavior(built: &[Declared], call: &AgentFunctionCall) -> Step<AgentMessage> {
    let behavior = built
        .iter()
        .find(|built| built.catalog.catalog_id() == call.catalog_id)
        .and_then(|built| built.behaviors.get(&call.name));
    let Some(behavior) = behavior else {
        return fail(format!(
            "the case scripts no behaviour for '{}' of '{}'",
            call.name, call.catalog_id
        ));
    };
    known_keys(
        behavior,
        &["returns", "returns_args", "returns_context", "raises"],
        "a behaviour",
    )?;
    if let Some(raises) = behavior.get("raises") {
        known_keys(raises, &["message", "code", "validation"], "raises")?;
        let message = raises
            .get("message")
            .and_then(Value::as_str)
            .unwrap_or_default();
        return Ok(match raises.get("code").and_then(Value::as_str) {
            // A handler that chooses its own code.
            Some(code) => call.error(RpcError::new(code, message)),
            // Any other failure of a handler is an execution error.
            None => call.failed(message),
        });
    }
    if behavior.get("returns_args") == Some(&Value::Bool(true)) {
        return Ok(call.value(Value::Object(call.args.clone())));
    }
    if behavior.get("returns_context") == Some(&Value::Bool(true)) {
        return Ok(call.value(json!({
            "catalogId": call.catalog_id,
            "surfaceId": call.surface_id,
            "functionCallId": call.function_call_id,
        })));
    }
    Ok(call.value(behavior.get("returns").cloned().unwrap_or(Value::Null)))
}

/// A reply against what a case expects of it: the code exactly, the message
/// as text the one sent contains.
fn same_response(expected: &Value, actual: &Value) -> bool {
    let (Some(expected), Some(actual)) = (expected.as_object(), actual.as_object()) else {
        return same(expected, actual);
    };
    if expected.get("functionCallId") != actual.get("functionCallId") {
        return false;
    }
    match (expected.get("error"), actual.get("error")) {
        (Some(expected), Some(actual)) => {
            expected.get("code") == actual.get("code")
                && expected
                    .get("message")
                    .and_then(Value::as_str)
                    .is_none_or(|wanted| {
                        actual
                            .get("message")
                            .and_then(Value::as_str)
                            .is_some_and(|sent| sent.contains(wanted))
                    })
        }
        (None, None) => match (expected.get("value"), actual.get("value")) {
            (Some(expected), Some(actual)) => same(expected, actual),
            _ => false,
        },
        _ => false,
    }
}

/// Whether a case expects nothing to have run: an answer the crate gives by
/// itself, or an error.
fn expects_no_run(case: &Value, expected_reply: Option<&Value>) -> bool {
    case.get("expect_error").is_some()
        || expected_reply
            .and_then(|reply| reply.pointer("/error/code"))
            .and_then(Value::as_str)
            .is_some_and(|code| code == "INVALID_FUNCTION_CALL" || code == "UNKNOWN_FUNCTION")
}

pub(super) fn handle_agent_function(case: &Value, args: &Value) -> Step {
    case_keys(case, &["args", "expect", "expect_error"])?;
    known_keys(args, &["catalogs", "message"], "args")?;
    let entries: Vec<&Value> = args
        .get("catalogs")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .collect();
    let message = args.get("message").unwrap_or(&Value::Null);
    let routed = router(&entries).and_then(|(built, router)| Ok((built, router.route(message)?)));
    if let Err(error) = &routed {
        if let Some(detail) = broken(error) {
            return fail(detail);
        }
    }
    let Some((built, routed)) = settle(case, routed, "handling the call")? else {
        return Ok(());
    };
    let expected = case.get("expect").unwrap_or(&Value::Null);
    let (reply, ran) = match routed {
        Routed::Call(call) => (run_behavior(&built, &call)?, true),
        Routed::Reply(reply) => (reply, false),
        other => return fail(format!("a callAgentFunction was routed as {other:?}")),
    };
    if ran && expects_no_run(case, Some(expected)) {
        return fail("the case expects no handler to run, and the call was handed over");
    }
    let sent = serde_json::to_value(&reply)
        .or_else(|error| fail(format!("the reply does not serialize: {error}")))?;
    if sent["version"] != "1.0" {
        return fail(format!("the reply is not a v1.0 message: {sent}"));
    }
    let body = &sent["agentFunctionResponse"];
    if same_response(expected, body) {
        Ok(())
    } else {
        fail(format!("the reply: expected {expected}, got {body}"))
    }
}

pub(super) fn validate(case: &Value, args: &Value) -> Step {
    case_keys(case, &["args", "expect_valid", "expect_error"])?;
    known_keys(args, &["catalogs", "messages"], "args")?;
    let entries: Vec<&Value> = args
        .get("catalogs")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .collect();
    let messages: Vec<Value> = args
        .get("messages")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let checked = entries
        .iter()
        .map(|entry| declared(entry).map(|built| built.catalog))
        .collect::<Result<Vec<_>, _>>()
        .and_then(|catalogs| validate_payload(&catalogs, &messages));
    if let Err(error) = &checked {
        if let Some(detail) = broken(error) {
            return fail(detail);
        }
    }
    match settle(case, checked, "validating the payload")? {
        Some(()) if case.get("expect_valid") == Some(&Value::Bool(true)) => Ok(()),
        Some(()) => fail("the case states neither expect_valid nor expect_error"),
        None => Ok(()),
    }
}

/// A callback of a case: the kind it takes, and whether it declines.
fn callbacks(args: &Value) -> Step<BTreeMap<String, bool>> {
    let mut set = BTreeMap::new();
    for entry in args
        .get("callbacks")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
    {
        match entry {
            Value::String(kind) => {
                set.insert(kind.clone(), false);
            }
            Value::Object(_) => {
                known_keys(entry, &["kind", "declines"], "a callback")?;
                let kind = entry
                    .get("kind")
                    .and_then(Value::as_str)
                    .unwrap_or_default();
                set.insert(
                    kind.to_string(),
                    entry.get("declines") == Some(&Value::Bool(true)),
                );
            }
            other => return fail(format!("not a callback: {other}")),
        }
    }
    Ok(set)
}

/// What processing one renderer message came to.
struct Processed {
    handled: bool,
    replies: Vec<Value>,
    /// The callback that ran, with the message it was handed.
    callback: Option<(String, Value)>,
}

/// One message through the router, with the application's part played by the
/// case: its scripted functions, and its callbacks, each of which records the
/// message it is handed. A kind no callback takes is left unread, for the
/// model's turn.
fn process(
    built: &[Declared],
    router: &RendererMessageRouter,
    callbacks: &BTreeMap<String, bool>,
    message: &Value,
) -> Result<Step<Processed>, Error> {
    let kind = RendererMessageKind::of(message)?;
    if kind != RendererMessageKind::CallAgentFunction && !callbacks.contains_key(kind.as_str()) {
        return Ok(Ok(Processed {
            handled: false,
            replies: Vec::new(),
            callback: None,
        }));
    }
    let payload = match router.route(message)? {
        Routed::Call(call) => {
            return Ok(run_behavior(built, &call).map(|reply| Processed {
                handled: true,
                replies: vec![serde_json::to_value(reply).unwrap_or(Value::Null)],
                callback: None,
            }));
        }
        Routed::Reply(reply) => {
            return Ok(Ok(Processed {
                handled: true,
                replies: vec![serde_json::to_value(reply).unwrap_or(Value::Null)],
                callback: None,
            }));
        }
        Routed::Action(action) => RendererPayload::Action(action),
        Routed::Error(error) => RendererPayload::Error(error),
        Routed::FunctionResponse(response) => RendererPayload::RendererFunctionResponse(response),
        other => return Ok(fail(format!("a message was routed as {other:?}"))),
    };
    let handed = RendererMessage::new(payload).with_version(A2uiVersion::V1_0);
    let declines = callbacks.get(kind.as_str()).copied().unwrap_or(false);
    Ok(Ok(Processed {
        handled: !declines,
        replies: Vec::new(),
        callback: Some((
            kind.as_str().to_string(),
            serde_json::to_value(handed).unwrap_or(Value::Null),
        )),
    }))
}

pub(super) fn process_renderer_message(case: &Value, args: &Value) -> Step {
    case_keys(case, &["args", "expect", "expect_error"])?;
    known_keys(
        args,
        &["catalog", "catalogs", "callbacks", "message"],
        "args",
    )?;
    let mut entries: Vec<&Value> = Vec::new();
    entries.extend(args.get("catalog"));
    entries.extend(
        args.get("catalogs")
            .and_then(Value::as_array)
            .into_iter()
            .flatten(),
    );
    let callbacks = callbacks(args)?;
    let message = args.get("message").unwrap_or(&Value::Null);
    let processed =
        router(&entries).and_then(|(built, router)| process(&built, &router, &callbacks, message));
    if let Err(error) = &processed {
        if let Some(detail) = broken(error) {
            return fail(detail);
        }
    }
    let Some(processed) = settle(case, processed, "processing the message")? else {
        return Ok(());
    };
    let processed = processed?;
    let expect = case.get("expect").unwrap_or(&Value::Null);
    known_keys(expect, &["handled", "replies", "callback"], "expect")?;
    if expect.get("handled").and_then(Value::as_bool) != Some(processed.handled) {
        return fail(format!(
            "handled: expected {:?}, got {}",
            expect.get("handled"),
            processed.handled
        ));
    }
    let expected_replies: Vec<Value> = expect
        .get("replies")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let replies_match = expected_replies.len() == processed.replies.len()
        && expected_replies
            .iter()
            .zip(&processed.replies)
            .all(|(expected, sent)| {
                expected.get("version") == sent.get("version")
                    && same_response(
                        &expected["agentFunctionResponse"],
                        &sent["agentFunctionResponse"],
                    )
            });
    if !replies_match {
        return fail(format!(
            "the replies: expected {expected_replies:?}, got {:?}",
            processed.replies
        ));
    }
    match (expect.get("callback"), &processed.callback) {
        (None, None) => Ok(()),
        (Some(expected), Some((kind, handed))) => {
            known_keys(expected, &["kind", "message"], "expect.callback")?;
            if expected.get("kind").and_then(Value::as_str) != Some(kind.as_str())
                || !same(expected.get("message").unwrap_or(&Value::Null), handed)
            {
                return fail(format!(
                    "the callback: expected {expected}, got {kind} with {handed}"
                ));
            }
            Ok(())
        }
        (expected, ran) => fail(format!(
            "the callback: expected {expected:?}, and {:?} ran",
            ran.as_ref().map(|(kind, _)| kind)
        )),
    }
}

// --- agent/renderer_function_calls.yaml --------------------------------------------------------

/// A catalog a case builds in code: an id and its functions, each with the
/// callers and the activation the case states and nothing else.
fn coded_catalog(entry: &Value) -> Step<Catalog> {
    known_keys(
        entry,
        &["catalogId", "functions"],
        "a renderer call catalog",
    )?;
    let id = entry
        .get("catalogId")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let mut catalog = Catalog::empty(id).with_version(A2uiVersion::V1_0);
    for (name, stated) in entry
        .get("functions")
        .and_then(Value::as_object)
        .into_iter()
        .flatten()
    {
        known_keys(
            stated,
            &["allowedCallers", "requiresUserActivation"],
            "a function",
        )?;
        let mut function = FunctionDef::new(name);
        if let Some(callers) = stated.get("allowedCallers").and_then(Value::as_str) {
            let Some(callers) = AllowedCallers::parse(callers) else {
                return fail(format!("not an allowedCallers value: {callers}"));
            };
            function.allowed_callers = callers;
        }
        function.requires_user_activation =
            stated.get("requiresUserActivation") == Some(&Value::Bool(true));
        catalog.functions.insert(name.clone(), function);
    }
    Ok(catalog)
}

/// How a labelled call of a case stands.
fn standing(outcome: Option<&FunctionOutcome>) -> Value {
    match outcome {
        None => json!("pending"),
        Some(FunctionOutcome::Value(value)) => json!({"value": value}),
        Some(FunctionOutcome::Error(error)) => {
            json!({"error": {"code": error.code, "message": error.message}})
        }
        Some(_) => json!("an outcome this harness does not know"),
    }
}

fn same_standing(expected: &Value, actual: &Value) -> bool {
    match (expected.get("error"), actual.get("error")) {
        (Some(expected), Some(actual)) => {
            expected.get("code") == actual.get("code")
                && expected
                    .get("message")
                    .and_then(Value::as_str)
                    .is_none_or(|wanted| {
                        actual["message"]
                            .as_str()
                            .is_some_and(|sent| sent.contains(wanted))
                    })
        }
        (None, None) => same(expected, actual),
        _ => false,
    }
}

pub(super) fn call_renderer_function(case: &Value, args: &Value) -> Step {
    case_keys(case, &["args", "steps", "expect_error"])?;
    known_keys(
        args,
        &["default_timeout_ms", "max_pending", "catalogs"],
        "args",
    )?;
    let mut calls = RendererCalls::new();
    if let Some(timeout) = args.get("default_timeout_ms").and_then(Value::as_u64) {
        calls = calls.with_default_timeout(Some(Duration::from_millis(timeout)));
    }
    if let Some(limit) = args.get("max_pending").and_then(Value::as_u64) {
        calls = calls.with_max_pending(usize::try_from(limit).unwrap_or(usize::MAX));
    }
    if let Some(entries) = args.get("catalogs").and_then(Value::as_array) {
        let catalogs = entries
            .iter()
            .map(coded_catalog)
            .collect::<Step<Vec<Catalog>>>()?;
        let checked = calls.with_catalog_summaries(catalogs);
        match (case.get("expect_error"), checked) {
            (Some(expected), Err(error)) => {
                return expect_error(expected, &error, "building the calls");
            }
            (Some(expected), Ok(_)) => {
                return fail(format!(
                    "building the calls: expected {expected}, and it succeeded"
                ));
            }
            (None, Err(error)) => {
                return fail(format!("building the calls: {}: {error}", category(&error)));
            }
            (None, Ok(checked)) => calls = checked,
        }
    } else if let Some(expected) = case.get("expect_error") {
        return fail(format!(
            "building the calls: expected {expected}, and it succeeded"
        ));
    }

    // Responses come in the way a renderer's messages do: through the router.
    let router = RendererMessageRouter::new(&[])
        .or_else(|error| fail(format!("a router over no catalogs: {error}")))?;
    let mut clock = Duration::from_secs(1_000);
    let mut ids: BTreeMap<String, String> = BTreeMap::new();
    let mut outcomes: BTreeMap<String, FunctionOutcome> = BTreeMap::new();
    let settled = |outcomes: &mut BTreeMap<String, FunctionOutcome>, ended: Vec<SettledCall>| {
        for call in ended {
            outcomes.insert(call.function_call_id, call.outcome);
        }
    };
    for (index, step) in case
        .get("steps")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .enumerate()
    {
        let at = format!("step {}", index + 1);
        known_keys(
            step,
            &[
                "call",
                "respond",
                "advance_ms",
                "dispose",
                "expect_message",
                "expect_error",
                "expect_calls",
                "expect_pending_count",
            ],
            "a step",
        )?;
        if let Some(stated) = step.get("call") {
            known_keys(
                stated,
                &[
                    "label",
                    "catalogId",
                    "name",
                    "args",
                    "surfaceId",
                    "functionCallId",
                    "timeout_ms",
                ],
                "a call",
            )?;
            let text = |key: &str| stated.get(key).and_then(Value::as_str);
            let mut call = RendererCall::new(
                text("catalogId").unwrap_or_default(),
                text("name").unwrap_or_default(),
            );
            if let Some(args) = stated.get("args") {
                call = call.with_args(args.clone());
            }
            if let Some(surface_id) = text("surfaceId") {
                call = call.on_surface(surface_id);
            }
            if let Some(id) = text("functionCallId") {
                call = call.with_id(id);
            }
            if let Some(timeout) = stated.get("timeout_ms").and_then(Value::as_u64) {
                call = call.with_timeout(Duration::from_millis(timeout));
            }
            match (step.get("expect_error"), calls.call(call, clock)) {
                (None, Ok(message)) => {
                    let mut sent = serde_json::to_value(&message).or_else(|error| {
                        fail(format!("{at}: the call does not serialize: {error}"))
                    })?;
                    let id = sent["callRendererFunction"]["functionCallId"]
                        .as_str()
                        .unwrap_or_default()
                        .to_string();
                    if id.is_empty() || ids.values().any(|other| *other == id) {
                        return fail(format!(
                            "{at}: the functionCallId {id:?} is empty or used twice"
                        ));
                    }
                    ids.insert(text("label").unwrap_or_default().to_string(), id);
                    if let Some(expected) = step.get("expect_message") {
                        if let Some(payload) = sent["callRendererFunction"].as_object_mut() {
                            payload.remove("functionCallId");
                        }
                        if !same(expected, &sent) {
                            return fail(format!(
                                "{at}: expected the message {expected}, got {sent}"
                            ));
                        }
                    }
                }
                (Some(expected), Err(error)) => {
                    known_keys(expected, &["code", "message"], "expect_error")?;
                    let wanted = expected.get("message").and_then(Value::as_str);
                    if expected.get("code").and_then(Value::as_str) != Some(error.code.as_str())
                        || wanted.is_some_and(|wanted| !error.message.contains(wanted))
                    {
                        return fail(format!("{at}: expected {expected}, got {error}"));
                    }
                }
                (None, Err(error)) => return fail(format!("{at}: the call was refused: {error}")),
                (Some(expected), Ok(_)) => {
                    return fail(format!("{at}: expected {expected}, and the call was made"));
                }
            }
        }
        if let Some(stated) = step.get("respond") {
            known_keys(
                stated,
                &["call", "functionCallId", "value", "error"],
                "a response",
            )?;
            let id = match stated.get("call").and_then(Value::as_str) {
                Some(label) => match ids.get(label) {
                    Some(id) => id.clone(),
                    None => return fail(format!("{at}: no call is labelled {label:?}")),
                },
                None => stated
                    .get("functionCallId")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string(),
            };
            let mut body = json!({"functionCallId": id});
            for key in ["value", "error"] {
                if let Some(value) = stated.get(key) {
                    body[key] = value.clone();
                }
            }
            let message = json!({"version": "1.0", "rendererFunctionResponse": body});
            let was_pending = calls.is_pending(&id);
            let response = match router.route(&message) {
                Ok(Routed::FunctionResponse(response)) => response,
                other => return fail(format!("{at}: the response was routed as {other:?}")),
            };
            let ended = calls.resolve(&response);
            // A response is taken exactly when its call waits.
            if ended.is_some() != was_pending {
                return fail(format!(
                    "{at}: the call {} pending, and the response {} taken",
                    if was_pending { "was" } else { "was not" },
                    if ended.is_some() { "was" } else { "was not" },
                ));
            }
            settled(&mut outcomes, ended.into_iter().collect());
        }
        if let Some(advance) = step.get("advance_ms").and_then(Value::as_u64) {
            clock += Duration::from_millis(advance);
            let late = calls.expire(clock);
            settled(&mut outcomes, late);
        }
        if let Some(stated) = step.get("dispose") {
            known_keys(stated, &["reason"], "dispose")?;
            let reason = stated
                .get("reason")
                .and_then(Value::as_str)
                .unwrap_or("The calls were disposed of.");
            let dropped = calls.dispose(reason);
            settled(&mut outcomes, dropped);
        }
        for (label, expected) in step
            .get("expect_calls")
            .and_then(Value::as_object)
            .into_iter()
            .flatten()
        {
            let Some(id) = ids.get(label) else {
                return fail(format!("{at}: no call is labelled {label:?}"));
            };
            let actual = standing(outcomes.get(id));
            let pending = calls.is_pending(id);
            if !same_standing(expected, &actual) || pending != (actual == "pending") {
                return fail(format!(
                    "{at}: call {label}: expected {expected}, got {actual} (pending: {pending})"
                ));
            }
        }
        if let Some(expected) = step.get("expect_pending_count").and_then(Value::as_u64) {
            if expected != calls.pending_count() as u64 {
                return fail(format!(
                    "{at}: expected {expected} pending, got {}",
                    calls.pending_count()
                ));
            }
        }
    }
    Ok(())
}
