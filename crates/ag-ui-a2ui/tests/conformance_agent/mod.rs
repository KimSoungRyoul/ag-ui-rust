//! Runners for the agent-tier suites: `agent/*.yaml` and
//! `agent/direct_json/*.yaml`.
//!
//! Each runner builds what a case names through the crate's public API, calls
//! the action, and compares. Behaviour does not live here: when a case fails,
//! the crate is what gets fixed, or the case is listed in `KNOWN_FAILURES`.
//!
//! The suites are written against A2UI v1.0. A case whose catalogs declare
//! v1.0 is lowered to v0.9 on the way in and lifted on the way out, by the
//! rules in [`lowering`] and nowhere else; a case that declares v0.9 runs as
//! written. A case v0.9 cannot state is skipped, never bent.

mod lowering;

use std::collections::BTreeSet;
use std::path::PathBuf;
use std::sync::Arc;

use ag_ui_a2ui::agent::{
    A2uiGenerator, A2uiRequestProcessor, AfterRejectedBlock, AnswerPart, CatalogConfig,
    CatalogDocument, DirectJson, OutputFormat, RawPart, RendererCapabilities, RendererState,
    ResponseError, ResponsePart, StreamOptions, StreamPart, StreamedAnswer, resolve_catalogs,
    validate_payload,
};
use ag_ui_a2ui::{A2uiVersion, AgentMessage, Error, Operation};
use serde_json::{Value, json};

use self::lowering::Declared;
use super::{Outcome, case_name, conformance_dir, truncate};

/// The label of a case that needs A2UI v1.0.
pub(crate) const NEEDS_V1_0: &str = "later: A2UI v1.0 (#42)";
/// The label of a case written for the v0.8 wire format.
pub(crate) const NEEDS_V0_8: &str = "out of scope: v0.8";
/// The label of a case that needs a format other than direct JSON.
pub(crate) const NEEDS_FORMAT: &str = "later: other formats (#74)";

/// The catalog a case that names none runs against.
const DEFAULT_CATALOG: &str = "test_data/catalogs/simplified_catalog_v1_0.json";

/// How one case ended, and whether it ran lowered.
pub(crate) struct Ran {
    pub(crate) outcome: Outcome,
    /// The case was written against v1.0 and executed as v0.9.
    pub(crate) lowered: bool,
}

/// Why a runner stopped before the case passed.
enum Stop {
    Skip(String),
    Fail(String),
}

type Step<T = ()> = Result<T, Stop>;

fn fail<T>(detail: impl Into<String>) -> Step<T> {
    Err(Stop::Fail(detail.into()))
}

fn skip<T>(label: &str, why: impl std::fmt::Display) -> Step<T> {
    Err(Stop::Skip(format!("{label}: {why}")))
}

/// Runs one case of an agent-tier suite, and returns its checks: one, or for
/// a transcript one per format it declares, named by the format.
pub(crate) fn run_case(case: &Value) -> Vec<(Option<String>, Ran)> {
    let ended = |step: Step, lowered: bool| match step {
        Ok(()) => Ran {
            outcome: Outcome::Passed,
            lowered,
        },
        Err(Stop::Skip(reason)) => Ran {
            outcome: Outcome::Skipped(reason),
            lowered: false,
        },
        Err(Stop::Fail(detail)) => Ran {
            outcome: Outcome::Failed(detail),
            lowered,
        },
    };
    if case.get("action").and_then(Value::as_str) == Some("e2e_scenario") {
        return names(case.get("formats"))
            .into_iter()
            .map(|format| {
                let step = match format.as_str() {
                    "direct_json" => transcript(case),
                    other => skip(NEEDS_FORMAT, format!("the {other} format")),
                };
                (Some(format), ended(step, false))
            })
            .collect();
    }
    let mut lowered = false;
    let step = dispatch(case, &mut lowered);
    vec![(None, ended(step, lowered))]
}

fn dispatch(case: &Value, lowered: &mut bool) -> Step {
    let action = case.get("action").and_then(Value::as_str).unwrap_or("");
    let args = case.get("args").unwrap_or(&Value::Null);
    // The format a case asks for. This crate has direct JSON.
    for key in ["format", "format_override"] {
        match args.get(key).and_then(Value::as_str) {
            None | Some("direct_json") => {}
            Some(other) => return skip(NEEDS_FORMAT, format!("the {other} format")),
        }
    }
    match action {
        "create_format" => create_format(case, args, lowered),
        "create_processor" => create_processor(case, args, lowered),
        "resolve_catalogs" => resolve(case, args, lowered),
        "provide_catalog" => provide_catalog(case, args, lowered),
        "transform_catalog" => transform_catalog(case, args, lowered),
        "read_renderer_capabilities" => read_capabilities(case, args),
        "renderer_state_turns" => renderer_state_turns(case, lowered),
        "stream_answer" => stream_answer(case, args, lowered),
        "generate_prompt_snippet" => prompt_snippet(case, args, lowered),
        "unwrap" => unwrap(case, args),
        "has_format_content" => has_format_content(case, args),
        "wrap" => wrap(case, args),
        "parse_response" => parse_response(case, args, lowered),
        "compile" => compile(case, args, lowered),
        "decompile" => decompile(case, args, lowered),
        "parse_chunk" => parse_chunk(case, args, lowered),
        other => fail(format!("unknown action '{other}'")),
    }
}

// --- strict dispatch --------------------------------------------------------------------------

/// Keys every case may carry.
const METADATA: [&str; 4] = ["name", "description", "features", "action"];

/// Fails a case that carries a key its runner does not read: a case must not
/// pass with half of it ignored.
fn known_keys(object: &Value, allowed: &[&str], place: &str) -> Step {
    let Some(map) = object.as_object() else {
        return Ok(());
    };
    let unknown: Vec<&String> = map
        .keys()
        .filter(|key| !allowed.contains(&key.as_str()))
        .collect();
    if unknown.is_empty() {
        Ok(())
    } else {
        fail(format!("unknown key(s) {unknown:?} in {place}"))
    }
}

fn case_keys(case: &Value, allowed: &[&str]) -> Step {
    let mut all = METADATA.to_vec();
    all.extend_from_slice(allowed);
    known_keys(case, &all, "the case")
}

// --- catalogs ---------------------------------------------------------------------------------

/// What a case runs against.
struct Setup {
    catalogs: Vec<CatalogConfig>,
    /// The catalogs declare the suite's version, so the case is lowered.
    lowered: bool,
    /// The catalog named in each `createSurface` of a case that names none.
    inject: Option<String>,
}

impl Setup {
    fn active(&self) -> Vec<CatalogDocument> {
        self.catalogs
            .iter()
            .map(|config| config.transformed().clone())
            .collect()
    }

    fn text(&self, text: &str) -> String {
        if self.lowered {
            lowering::lower_text(text, self.inject.as_deref())
        } else {
            text.to_string()
        }
    }

    fn chunks(&self, chunks: &[&str]) -> Vec<String> {
        if self.lowered {
            lowering::lower_chunks(chunks, self.inject.as_deref())
        } else {
            chunks.iter().map(|chunk| (*chunk).to_string()).collect()
        }
    }

    /// Decoded messages a case starts from, as this crate reads them.
    fn messages(&self, messages: &[Value]) -> Step<Vec<AgentMessage>> {
        let messages = if self.lowered {
            lowering::lower_messages(messages, self.inject.as_deref())
        } else {
            messages.to_vec()
        };
        messages
            .into_iter()
            .map(|message| {
                serde_json::from_value(message).or_else(|error| {
                    fail(format!("a message of the case does not decode: {error}"))
                })
            })
            .collect()
    }

    /// Messages the crate emitted, in the vocabulary of the suites.
    fn emitted(&self, messages: &[AgentMessage]) -> Vec<Value> {
        let raw: Vec<Value> = messages
            .iter()
            .map(|message| serde_json::to_value(message).unwrap_or(Value::Null))
            .collect();
        self.written(&raw)
    }

    /// Raw messages, in the vocabulary of the suites.
    fn written(&self, messages: &[Value]) -> Vec<Value> {
        if self.lowered {
            lowering::lift_messages(messages, self.inject.as_deref())
        } else {
            let mut messages = messages.to_vec();
            for message in &mut messages {
                lowering::unstate_defaults(message, None);
            }
            messages
        }
    }

    fn capabilities(&self, capabilities: &Value) -> Value {
        if self.lowered {
            lowering::lower_capabilities(capabilities)
        } else {
            capabilities.clone()
        }
    }
}

fn read_fixture(relative: &str) -> Step<Value> {
    let path = conformance_dir().join(relative);
    let text = std::fs::read_to_string(&path)
        .or_else(|error| fail(format!("cannot read fixture {relative}: {error}")))?;
    serde_json::from_str(&text)
        .or_else(|error| fail(format!("fixture {relative} is not JSON: {error}")))
}

fn names(value: Option<&Value>) -> Vec<String> {
    value
        .and_then(Value::as_array)
        .map(|names| {
            names
                .iter()
                .filter_map(Value::as_str)
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default()
}

/// One catalog a case names: a path under `conformance/`, a document written
/// in the case, or either under `catalog` with the transformers registered
/// with it.
fn catalog_entry(entry: &Value) -> Step<(CatalogConfig, Declared)> {
    let (source, transformers) = match entry.get("catalog") {
        Some(source) => (source, entry.get("transformers")),
        None => (entry, None),
    };
    known_keys(
        if entry.get("catalog").is_some() {
            entry
        } else {
            &Value::Null
        },
        &["catalog", "transformers"],
        "a catalog registration",
    )?;
    let (document, path) = match source {
        Value::String(path) => (read_fixture(path)?, Some(path.as_str())),
        Value::Object(_) => (source.clone(), None),
        other => return fail(format!("not a catalog entry: {other}")),
    };
    let declared = lowering::declared(&document);
    if declared == Declared::V0_8 {
        return skip(NEEDS_V0_8, "the case's catalog declares v0.8");
    }
    // A fixture that names no catalog is given its path as its id, and one
    // that states no version is a v1.0 one, read lowered.
    let fallback_id = path.filter(|_| document.get("catalogId").is_none());
    let fallback_version = document
        .get("protocolVersion")
        .is_none()
        .then_some(A2uiVersion::V0_9);
    let catalog = CatalogDocument::load(
        lowering::lower_catalog(&document),
        fallback_id,
        fallback_version,
    )
    .or_else(|error| fail(format!("the case's catalog does not load: {error}")))?;
    let mut config = CatalogConfig::new(catalog);
    for transformer in transformers.and_then(Value::as_array).into_iter().flatten() {
        known_keys(
            transformer,
            &["component_pruning", "function_pruning"],
            "a transformer",
        )?;
        // An allowlist is read literally: `null` keeps nothing, like `[]`.
        if let Some(allowed) = transformer.get("component_pruning") {
            config = config.keeping_components(names(Some(allowed)));
        }
        if let Some(allowed) = transformer.get("function_pruning") {
            config = config.keeping_functions(names(Some(allowed)));
        }
    }
    Ok((config, declared))
}

/// The catalogs a case names in `catalog` and `catalogs`, or the default
/// fixture when `default` and it names none.
fn setup(case: &Value, container: &Value, default: bool, lowered: &mut bool) -> Step<Setup> {
    let mut entries: Vec<&Value> = Vec::new();
    entries.extend(container.get("catalog"));
    entries.extend(
        container
            .get("catalogs")
            .and_then(Value::as_array)
            .into_iter()
            .flatten(),
    );
    let fallback = json!(DEFAULT_CATALOG);
    if entries.is_empty() && default {
        entries.push(&fallback);
    }
    let mut catalogs = Vec::new();
    // A case that names no catalog at all is still a v1.0 case.
    let mut suite = entries.is_empty();
    for entry in entries {
        let (config, declared) = catalog_entry(entry)?;
        suite |= declared == Declared::Suite;
        catalogs.push(config);
    }
    if suite {
        // Everything the case hands the crate, with chunked input joined, so
        // a marker split across chunks is still found.
        let mut text = case.to_string();
        for step in case
            .get("steps")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
        {
            text.push_str(
                step.get("input")
                    .and_then(Value::as_str)
                    .unwrap_or_default(),
            );
        }
        if let Some(why) = lowering::v1_only(case_name(case), &text) {
            return skip(NEEDS_V1_0, why);
        }
    }
    // A case with no catalog and no capabilities hands the crate nothing a
    // rule would rewrite.
    *lowered = suite && (!catalogs.is_empty() || container.get("renderer_capabilities").is_some());
    let inject = (suite && !case.to_string().contains("catalogId"))
        .then(|| {
            catalogs
                .first()
                .map(|config| config.transformed().catalog_id().to_string())
        })
        .flatten();
    Ok(Setup {
        catalogs,
        lowered: suite,
        inject,
    })
}

/// The direct JSON format a case asks for.
fn direct_json(args: &Value, setup: &Setup) -> DirectJson {
    let mut format = DirectJson::new()
        .with_progressive_keys(names(args.get("progressive_keys")))
        // From v1.0 a component is read only once its object closes. A case
        // written against v1.0 fixes that timing, so it is kept when the case
        // runs lowered.
        .holding_components_until_closed(setup.lowered);
    if let Some(allowed) = args.get("allowed_messages") {
        format = format.allowing_messages(
            names(Some(allowed))
                .iter()
                .filter_map(|name| Operation::parse(name)),
        );
    }
    format
}

fn examples(args: &Value, setup: &Setup) -> Step<Vec<Vec<AgentMessage>>> {
    names(args.get("examples"))
        .iter()
        .map(|path| {
            let turn = read_fixture(path)?;
            setup.messages(turn.as_array().map(Vec::as_slice).unwrap_or_default())
        })
        .collect()
}

fn processor(args: &Value, setup: &Setup) -> Result<A2uiRequestProcessor, Error> {
    A2uiRequestProcessor::new(
        setup.active(),
        Vec::new(),
        Arc::new(direct_json(args, setup)),
    )
}

fn ready_processor(args: &Value, setup: &Setup) -> Step<A2uiRequestProcessor> {
    processor(args, setup).or_else(|error| fail(format!("the processor cannot be built: {error}")))
}

// --- comparing --------------------------------------------------------------------------------

/// Deep equality of JSON as the suites mean it: a number is a number however
/// it is written, and an `updateDataModel` that states the default path `/`
/// is one that states none.
fn same(expected: &Value, actual: &Value) -> bool {
    match (expected, actual) {
        (Value::Number(expected), Value::Number(actual)) => expected.as_f64() == actual.as_f64(),
        (Value::Array(expected), Value::Array(actual)) => {
            expected.len() == actual.len()
                && expected
                    .iter()
                    .zip(actual)
                    .all(|(expected, actual)| same(expected, actual))
        }
        (Value::Object(expected), Value::Object(actual)) => {
            let default_path =
                |key: &String, value: &Value, other: &serde_json::Map<String, Value>| {
                    key == "path"
                        && value == "/"
                        && !other.contains_key("path")
                        && other.contains_key("surfaceId")
                };
            expected.iter().all(|(key, value)| match actual.get(key) {
                Some(found) => same(value, found),
                None => default_path(key, value, actual),
            }) && actual.iter().all(|(key, value)| {
                expected.contains_key(key) || default_path(key, value, expected)
            })
        }
        _ => expected == actual,
    }
}

fn expect_same(expected: &Value, actual: &Value, what: &str) -> Step {
    if same(expected, actual) {
        Ok(())
    } else {
        fail(format!("{what}: expected {expected}, got {actual}"))
    }
}

fn category(error: &Error) -> &'static str {
    match error {
        Error::Parse(_) | Error::Json(_) => "ParseError",
        Error::Validation { .. } => "ValidationError",
        Error::Catalog(_) => "CatalogError",
        _ => "another error",
    }
}

/// An error in one line: for a validation error, the first failure in full,
/// since that is what says why a case ended as it did.
fn brief(error: &Error) -> String {
    match error {
        Error::Validation { errors } => match errors.as_slice() {
            [first, rest @ ..] => format!("{first} (+{} more)", rest.len()),
            [] => "no errors".to_string(),
        },
        other => truncate(&other.to_string()),
    }
}

/// Checks an error against a case's `expect_error`: its category, and the
/// text its message has to contain.
fn expect_error(expected: &Value, error: &Error, what: &str) -> Step {
    known_keys(expected, &["category", "message"], "expect_error")?;
    let wanted = expected
        .get("category")
        .and_then(Value::as_str)
        .unwrap_or("");
    if category(error) != wanted {
        return fail(format!(
            "{what}: expected {wanted}, got {}: {}",
            category(error),
            brief(error)
        ));
    }
    if let Some(message) = expected.get("message").and_then(Value::as_str) {
        if !error
            .to_string()
            .to_lowercase()
            .contains(&message.to_lowercase())
        {
            return fail(format!(
                "{what}: the error does not mention {message:?}: {error}"
            ));
        }
    }
    Ok(())
}

/// Holds a result to a case's `expect_error`, or hands on its value when the
/// case expects none.
fn settle<T>(case: &Value, result: Result<T, Error>, what: &str) -> Step<Option<T>> {
    match (case.get("expect_error"), result) {
        (Some(expected), Err(error)) => expect_error(expected, &error, what).map(|()| None),
        (Some(expected), Ok(_)) => fail(format!("{what}: expected {expected}, and it succeeded")),
        (None, Ok(value)) => Ok(Some(value)),
        (None, Err(error)) => fail(format!("{what}: {}: {}", category(&error), brief(&error))),
    }
}

fn response_parts(setup: &Setup, parts: &[ResponsePart]) -> Value {
    Value::Array(
        parts
            .iter()
            .map(|part| match part {
                ResponsePart::Text(text) => json!({"text": text}),
                ResponsePart::A2ui(messages) => json!({"a2ui": setup.emitted(messages)}),
            })
            .collect(),
    )
}

fn stream_parts(setup: &Setup, parts: &[StreamPart]) -> Value {
    Value::Array(
        parts
            .iter()
            .map(|part| match part {
                StreamPart::Text(text) => json!({"text": text}),
                StreamPart::A2ui(messages) => json!({"a2ui": setup.emitted(messages)}),
                StreamPart::Rejected(error) => json!({"error": {"category": category(error)}}),
                // A kind of part the suite has no word for compares equal to
                // nothing it expects.
                other => json!({"unknown": format!("{other:?}")}),
            })
            .collect(),
    )
}

/// Joins adjacent text parts: a stream emits text chunk by chunk where an
/// answer read whole emits one part.
fn merge_text(parts: &Value) -> Value {
    let mut merged: Vec<Value> = Vec::new();
    for part in parts.as_array().into_iter().flatten() {
        match (
            merged.last_mut().and_then(|last| last.get_mut("text")),
            part.get("text"),
        ) {
            (Some(Value::String(before)), Some(Value::String(text))) => before.push_str(text),
            _ => merged.push(part.clone()),
        }
    }
    Value::Array(merged)
}

fn snippet_checks(snippet: &str, expect: &Value, contains: &str, absent: &str) -> Step {
    for text in names(expect.get(contains)) {
        if !snippet.contains(&text) {
            return fail(format!("the prompt snippet does not contain {text:?}"));
        }
    }
    for text in names(expect.get(absent)) {
        if snippet.contains(&text) {
            return fail(format!("the prompt snippet contains {text:?}"));
        }
    }
    Ok(())
}

/// Checks a catalog against `{catalog_id, protocol_version, components,
/// functions}`. The name lists are exhaustive.
fn expect_catalog(expected: &Value, catalog: &CatalogDocument, lowered: bool) -> Step {
    known_keys(
        expected,
        &["catalog_id", "protocol_version", "components", "functions"],
        "a catalog expectation",
    )?;
    if let Some(id) = expected.get("catalog_id").and_then(Value::as_str) {
        if catalog.catalog_id() != id {
            return fail(format!(
                "expected catalog {id}, got {}",
                catalog.catalog_id()
            ));
        }
    }
    if let Some(version) = expected.get("protocol_version").and_then(Value::as_str) {
        let version = if lowered {
            lowering::swap_version(version)
        } else {
            version
        };
        if catalog.version().as_str() != version {
            return fail(format!(
                "expected protocol version {version}, got {}",
                catalog.version()
            ));
        }
    }
    for (key, found) in [
        (
            "components",
            catalog
                .catalog()
                .components
                .keys()
                .cloned()
                .collect::<BTreeSet<_>>(),
        ),
        (
            "functions",
            catalog
                .catalog()
                .functions
                .keys()
                .cloned()
                .collect::<BTreeSet<_>>(),
        ),
    ] {
        if let Some(expected) = expected.get(key) {
            let expected: BTreeSet<String> = names(Some(expected)).into_iter().collect();
            if expected != found {
                return fail(format!(
                    "catalog {}: expected {key} {expected:?}, got {found:?}",
                    catalog.catalog_id()
                ));
            }
        }
    }
    Ok(())
}

// --- request_processor.yaml ---------------------------------------------------------------------

fn create_format(case: &Value, args: &Value, lowered: &mut bool) -> Step {
    case_keys(case, &["args", "expect", "expect_error"])?;
    known_keys(
        args,
        &["format", "catalogs", "examples", "probe_chunks"],
        "args",
    )?;
    let setup = setup(case, args, false, lowered)?;
    let examples = examples(args, &setup)?;
    let format: Arc<dyn OutputFormat> = Arc::new(direct_json(args, &setup));
    let built = A2uiRequestProcessor::new(setup.active(), examples, format.clone());
    let Some(processor) = settle(case, built, "creating the format")? else {
        return Ok(());
    };
    let expect = case.get("expect").unwrap_or(&Value::Null);
    known_keys(
        expect,
        &[
            "prompt_snippet_contains",
            "prompt_snippet_absent",
            "parsers_are_distinct",
            "parser_state_isolated",
        ],
        "expect",
    )?;
    snippet_checks(
        processor.prompt_snippet(),
        expect,
        "prompt_snippet_contains",
        "prompt_snippet_absent",
    )?;
    if expect.get("parsers_are_distinct") == Some(&Value::Bool(true)) {
        // What holds the buffers of an answer here is the reader a format
        // opens, and each call opens one.
        if format.reader(true).is_none() || format.reader(true).is_none() {
            return fail("the format opens no reader");
        }
    }
    if expect.get("parser_state_isolated") == Some(&Value::Bool(true)) {
        let chunks = names(args.get("probe_chunks"));
        let chunks = setup.chunks(&chunks.iter().map(String::as_str).collect::<Vec<_>>());
        let read = || -> Step<Value> {
            let mut state = processor.create_state();
            let mut stream = open_stream(&processor, &state, StreamOptions::default())?;
            let mut parts = Vec::new();
            for chunk in &chunks {
                parts.extend(
                    stream
                        .feed(chunk, &mut state)
                        .or_else(|error| fail(format!("the probe stream failed: {error}")))?,
                );
            }
            Ok(stream_parts(&setup, &parts))
        };
        let first = read()?;
        if first.as_array().is_none_or(Vec::is_empty) {
            return fail("the probe chunks yielded nothing");
        }
        expect_same(&first, &read()?, "a second parser reading the same stream")?;
    }
    Ok(())
}

fn open_stream(
    processor: &A2uiRequestProcessor,
    state: &RendererState,
    options: StreamOptions,
) -> Step<ag_ui_a2ui::agent::ResponseStream> {
    processor
        .open_stream(state, options)
        .or_else(|error| fail(format!("the stream cannot be opened: {error}")))
}

fn create_processor(case: &Value, args: &Value, lowered: &mut bool) -> Step {
    case_keys(
        case,
        &[
            "args",
            "expect",
            "expect_error",
            "then_parse",
            "then_stream",
        ],
    )?;
    known_keys(
        args,
        &[
            "format",
            "format_override",
            "catalogs",
            "examples",
            "renderer_capabilities",
            "accepts_inline_catalogs",
        ],
        "args",
    )?;
    if args.get("format").is_none() {
        return skip(
            NEEDS_FORMAT,
            "the generator's default format, which upstream makes Express",
        );
    }
    let setup = setup(case, args, false, lowered)?;
    let generator = A2uiGenerator::new(setup.catalogs.clone())
        .with_examples(examples(args, &setup)?)
        .with_format(Arc::new(direct_json(args, &setup)))
        .accepting_inline_catalogs(args.get("accepts_inline_catalogs") == Some(&Value::Bool(true)));
    let before = generator.catalogs().to_vec();
    let created = match args.get("renderer_capabilities") {
        None => generator.create_processor(None),
        Some(capabilities) => RendererCapabilities::from_json(&setup.capabilities(capabilities))
            .and_then(|capabilities| match args.get("format_override") {
                Some(_) => generator.create_processor_with(
                    Some(&capabilities),
                    Arc::new(direct_json(args, &setup)),
                ),
                None => generator.create_processor(Some(&capabilities)),
            }),
    };
    let Some(processor) = settle(case, created, "creating the processor")? else {
        return Ok(());
    };

    let expect = case.get("expect").unwrap_or(&Value::Null);
    known_keys(
        expect,
        &[
            "active_catalog_ids",
            "catalogs",
            "prompt_snippet_contains",
            "prompt_snippet_absent",
            "generator_catalogs_unchanged",
        ],
        "expect",
    )?;
    if let Some(ids) = expect.get("active_catalog_ids") {
        // In the renderer's order of preference, so the order is compared.
        let active: Vec<&str> = processor
            .active_catalogs()
            .iter()
            .map(CatalogDocument::catalog_id)
            .collect();
        expect_same(ids, &json!(active), "the active catalogs")?;
    }
    for (position, expected) in expect
        .get("catalogs")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .enumerate()
    {
        let Some(catalog) = processor.active_catalogs().get(position) else {
            return fail(format!("no active catalog at position {position}"));
        };
        expect_catalog(expected, catalog, setup.lowered)?;
    }
    snippet_checks(
        processor.prompt_snippet(),
        expect,
        "prompt_snippet_contains",
        "prompt_snippet_absent",
    )?;
    if expect.get("generator_catalogs_unchanged") == Some(&Value::Bool(true))
        && generator.catalogs() != before
    {
        return fail("creating a processor changed the generator's catalogs");
    }

    if let Some(then) = case.get("then_parse") {
        known_keys(
            then,
            &["input", "wrapped", "expect", "expect_error"],
            "then_parse",
        )?;
        let input = setup.text(
            then.get("input")
                .and_then(Value::as_str)
                .unwrap_or_default(),
        );
        let parsed = if then.get("wrapped") == Some(&Value::Bool(false)) {
            processor.parse_block_in(&input, &mut processor.create_state())
        } else {
            processor.parse_response(&input)
        };
        if let Some(parts) = settle(
            then,
            parsed.map_err(Error::from),
            "parsing through the processor",
        )? {
            expect_same(
                then.get("expect").unwrap_or(&Value::Null),
                &response_parts(&setup, &parts),
                "the parsed parts",
            )?;
        }
    }
    if let Some(then) = case.get("then_stream") {
        known_keys(
            then,
            &["steps", "expect_close", "expect_error"],
            "then_stream",
        )?;
        run_stream(&setup, &processor, then, StreamOptions::default())?;
    }
    Ok(())
}

/// Feeds the `steps` of `script` through a stream, each stating the parts its
/// chunk yields, then closes the stream.
///
/// Returns what the stream yielded in all, or `None` when the script expected
/// the stream to end in an error and it did.
fn run_stream(
    setup: &Setup,
    processor: &A2uiRequestProcessor,
    script: &Value,
    options: StreamOptions,
) -> Step<Option<Value>> {
    let steps: Vec<&Value> = script
        .get("steps")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .collect();
    let inputs: Vec<&str> = steps
        .iter()
        .map(|step| {
            step.get("input")
                .and_then(Value::as_str)
                .unwrap_or_default()
        })
        .collect();
    let chunks = setup.chunks(&inputs);
    let mut state = processor.create_state();
    let mut stream = open_stream(processor, &state, options)?;
    let mut yielded: Vec<Value> = Vec::new();
    let in_block =
        |step: &Value, stream: &ag_ui_a2ui::agent::ResponseStream, index: usize| -> Step {
            match step.get("expect_in_block").and_then(Value::as_bool) {
                Some(expected) if expected != stream.is_in_block() => {
                    fail(format!("step {index}: expected in_block {expected}"))
                }
                _ => Ok(()),
            }
        };
    for (index, (step, chunk)) in steps.iter().zip(&chunks).enumerate() {
        known_keys(
            step,
            &[
                "input",
                "expect",
                "expect_error",
                "expect_partial_results",
                "expect_in_block",
            ],
            "a step",
        )?;
        match (step.get("expect_error"), stream.feed(chunk, &mut state)) {
            (Some(expected), Err(ResponseError { error, parts })) => {
                expect_error(expected, &error, &format!("step {index}"))?;
                if let Some(partial) = step.get("expect_partial_results") {
                    expect_same(
                        partial,
                        &response_parts(setup, &parts),
                        &format!("step {index}: partial results"),
                    )?;
                }
                in_block(step, &stream, index)?;
                return Ok(None);
            }
            (Some(expected), Ok(parts)) => {
                return fail(format!(
                    "step {index}: expected {expected}, got {}",
                    stream_parts(setup, &parts)
                ));
            }
            (None, Err(rejected)) => {
                return fail(format!(
                    "step {index}: {}: {}",
                    category(&rejected.error),
                    brief(&rejected.error)
                ));
            }
            (None, Ok(parts)) => {
                let parts = stream_parts(setup, &parts);
                expect_same(
                    step.get("expect").unwrap_or(&Value::Null),
                    &parts,
                    &format!("step {index}"),
                )?;
                in_block(step, &stream, index)?;
                yielded.extend(parts.as_array().cloned().unwrap_or_default());
            }
        }
    }
    let closed = stream.close(&mut state).map_err(Error::from);
    let closed = match (script.get("expect_error"), closed) {
        (Some(expected), Err(error)) => {
            return expect_error(expected, &error, "closing the stream").map(|()| None);
        }
        (Some(expected), Ok(_)) => return fail(format!("closing the stream: expected {expected}")),
        (None, Err(error)) => {
            return fail(format!("closing the stream: {}: {error}", category(&error)));
        }
        (None, Ok(parts)) => stream_parts(setup, &parts),
    };
    if let Some(expected) = script.get("expect_close") {
        expect_same(expected, &closed, "closing the stream")?;
    }
    yielded.extend(closed.as_array().cloned().unwrap_or_default());
    Ok(Some(Value::Array(yielded)))
}

// --- catalog_resolution.yaml, catalog_provider.yaml, catalog_transformer.yaml -------------------

fn resolve(case: &Value, args: &Value, lowered: &mut bool) -> Step {
    case_keys(case, &["args", "expect", "expect_error"])?;
    known_keys(
        args,
        &[
            "catalogs",
            "renderer_capabilities",
            "accepts_inline_catalogs",
        ],
        "args",
    )?;
    let setup = setup(case, args, false, lowered)?;
    let accepts = args.get("accepts_inline_catalogs") == Some(&Value::Bool(true));
    let resolved = match args.get("renderer_capabilities") {
        None => resolve_catalogs(&setup.catalogs, None, accepts),
        Some(capabilities) => RendererCapabilities::from_json(&setup.capabilities(capabilities))
            .and_then(|capabilities| {
                resolve_catalogs(&setup.catalogs, Some(&capabilities), accepts)
            }),
    };
    let Some(active) = settle(case, resolved, "resolving the catalogs")? else {
        return Ok(());
    };
    let expect = case.get("expect").unwrap_or(&Value::Null);
    known_keys(expect, &["active_catalog_ids", "catalogs"], "expect")?;
    // Nothing in the protocol gives the sequence a meaning here.
    let found: BTreeSet<String> = active
        .iter()
        .map(|catalog| catalog.catalog_id().to_string())
        .collect();
    let wanted: BTreeSet<String> = names(expect.get("active_catalog_ids"))
        .into_iter()
        .collect();
    if found != wanted || active.len() != wanted.len() {
        return fail(format!(
            "expected active catalogs {wanted:?}, got {found:?}"
        ));
    }
    for expected in expect
        .get("catalogs")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
    {
        let id = expected
            .get("catalog_id")
            .and_then(Value::as_str)
            .unwrap_or_default();
        let Some(catalog) = active.iter().find(|catalog| catalog.catalog_id() == id) else {
            return fail(format!("catalog {id} is not active"));
        };
        expect_catalog(expected, catalog, setup.lowered)?;
    }
    Ok(())
}

/// The version a case hands a provider, as this crate names it.
fn provider_version(args: &Value, lowered: bool) -> Step<Option<A2uiVersion>> {
    let Some(stated) = args.get("protocol_version").and_then(Value::as_str) else {
        return Ok(None);
    };
    let stated = if lowered {
        lowering::swap_version(stated)
    } else {
        stated
    };
    match A2uiVersion::parse(stated) {
        Some(version) => Ok(Some(version)),
        // A case that states a version other than its document's needs a
        // second protocol family to state it in.
        None => skip(
            NEEDS_V1_0,
            "a provider version of another protocol family than the document's",
        ),
    }
}

fn provide_catalog(case: &Value, args: &Value, lowered: &mut bool) -> Step {
    case_keys(case, &["args", "expect", "expect_error"])?;
    known_keys(
        args,
        &["provider", "path", "catalog_id", "protocol_version"],
        "args",
    )?;
    if args.get("provider").and_then(Value::as_str) != Some("file_system") {
        return fail("unknown provider");
    }
    let relative = args.get("path").and_then(Value::as_str).unwrap_or_default();
    let mut path = conformance_dir().join(relative);
    // The file is read by the provider, not by the harness. A document that
    // declares v1.0 is lowered into a copy the provider reads instead; one
    // that is missing, malformed or silent about its version is read as it is.
    let on_disk = std::fs::read_to_string(&path)
        .ok()
        .and_then(|text| serde_json::from_str::<Value>(&text).ok());
    let suite = on_disk
        .as_ref()
        .is_none_or(|document| lowering::declared(document) == Declared::Suite);
    let version = provider_version(args, suite)?;
    // What the lowering rules touched: a version the case hands the provider,
    // and a document that states v1.0.
    *lowered = suite && version.is_some();
    if let Some(document) =
        on_disk.filter(|document| document.get("protocolVersion").is_some() && suite)
    {
        let copy = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join(format!(
            "lowered-{}-{}",
            case_name(case),
            relative.replace('/', "-")
        ));
        std::fs::write(&copy, lowering::lower_catalog(&document).to_string())
            .or_else(|error| fail(format!("cannot write the lowered copy: {error}")))?;
        path = copy;
        *lowered = true;
    }
    let loaded = CatalogDocument::read(
        &path,
        args.get("catalog_id").and_then(Value::as_str),
        version,
    );
    match settle(case, loaded, "loading the catalog")? {
        Some(catalog) => {
            expect_catalog(case.get("expect").unwrap_or(&Value::Null), &catalog, suite)
        }
        None => Ok(()),
    }
}

fn transform_catalog(case: &Value, args: &Value, lowered: &mut bool) -> Step {
    case_keys(case, &["args", "expect"])?;
    known_keys(
        args,
        &["catalog", "transformers", "catalog_id", "protocol_version"],
        "args",
    )?;
    let relative = args
        .get("catalog")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let document = read_fixture(relative)?;
    *lowered = lowering::declared(&document) == Declared::Suite;
    // What the case's provider says, else the fallbacks every fixture gets: a
    // document that names no catalog is given its path as its id, and one
    // that states no version is a v1.0 one, read lowered.
    let catalog_id = args
        .get("catalog_id")
        .and_then(Value::as_str)
        .or_else(|| document.get("catalogId").is_none().then_some(relative));
    let version = provider_version(args, *lowered)?.or_else(|| {
        document
            .get("protocolVersion")
            .is_none()
            .then_some(A2uiVersion::V0_9)
    });
    let catalog = CatalogDocument::load(lowering::lower_catalog(&document), catalog_id, version)
        .or_else(|error| fail(format!("the case's catalog does not load: {error}")))?;
    let mut config = CatalogConfig::new(catalog);
    for transformer in args
        .get("transformers")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
    {
        known_keys(
            transformer,
            &["component_pruning", "function_pruning"],
            "a transformer",
        )?;
        if let Some(allowed) = transformer.get("component_pruning") {
            config = config.keeping_components(names(Some(allowed)));
        }
        if let Some(allowed) = transformer.get("function_pruning") {
            config = config.keeping_functions(names(Some(allowed)));
        }
    }
    expect_catalog(
        case.get("expect").unwrap_or(&Value::Null),
        config.transformed(),
        *lowered,
    )
}

// --- renderer_capabilities.yaml -----------------------------------------------------------------

fn read_capabilities(case: &Value, args: &Value) -> Step {
    case_keys(case, &["args", "expect", "expect_error"])?;
    known_keys(args, &["version", "metadata"], "args")?;
    let stated = args
        .get("version")
        .and_then(Value::as_str)
        .unwrap_or_default();
    // Each case states its own version, so it runs as written or not at all.
    let Some(version) = A2uiVersion::parse(stated) else {
        return if stated.starts_with("v0.8") {
            skip(NEEDS_V0_8, "capabilities of a v0.8 renderer")
        } else {
            skip(NEEDS_V1_0, format!("capabilities of a {stated} renderer"))
        };
    };
    let read = RendererCapabilities::from_metadata(args.get("metadata"), version);
    let Some(capabilities) = settle(case, read, "reading the capabilities")? else {
        return Ok(());
    };
    let found = capabilities.map_or(Value::Null, |capabilities| capabilities.to_json());
    expect_same(
        case.get("expect").unwrap_or(&Value::Null),
        &found,
        "the capabilities",
    )
}

// --- renderer_state.yaml ------------------------------------------------------------------------

fn renderer_state_turns(case: &Value, lowered: &mut bool) -> Step {
    case_keys(case, &["catalogs", "turns"])?;
    let turns: Vec<&Value> = case
        .get("turns")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .collect();
    if let Some(other) = turns
        .iter()
        .filter_map(|turn| turn.get("format").and_then(Value::as_str))
        .find(|format| *format != "direct_json")
    {
        return skip(NEEDS_FORMAT, format!("the {other} format"));
    }
    let setup = setup(case, case, true, lowered)?;
    let processor = ready_processor(&Value::Null, &setup)?;
    let mut state = processor.create_state();
    for (index, turn) in turns.iter().enumerate() {
        known_keys(
            turn,
            &[
                "format",
                "payload",
                "response",
                "expect_surfaces",
                "expectError",
                "expect_error",
            ],
            "a turn",
        )?;
        let applied: Result<(), Error> = match turn.get("payload").and_then(Value::as_array) {
            Some(payload) => {
                let messages: Vec<Value> = setup
                    .messages(payload)?
                    .iter()
                    .map(|message| serde_json::to_value(message).unwrap_or(Value::Null))
                    .collect();
                state.transaction(|state| {
                    let touched = state.apply(&messages)?;
                    state.check_trees(&touched)
                })
            }
            None => {
                let response = turn
                    .get("response")
                    .and_then(Value::as_str)
                    .unwrap_or_default();
                // A turn that names its catalog is not given one.
                let text = if response.contains("\"catalogId\"") {
                    if setup.lowered {
                        lowering::lower_text(response, None)
                    } else {
                        response.to_string()
                    }
                } else {
                    setup.text(response)
                };
                processor
                    .parse_response_in(&text, &mut state)
                    .map(|_| ())
                    .map_err(Error::from)
            }
        };
        let expected_error = turn.get("expect_error").or_else(|| turn.get("expectError"));
        match (expected_error, applied) {
            (Some(expected), Err(error)) => {
                expect_error(expected, &error, &format!("turn {index}"))?;
                continue;
            }
            (Some(expected), Ok(())) => {
                return fail(format!(
                    "turn {index}: expected {expected}, and it was applied"
                ));
            }
            (None, Err(error)) => {
                return fail(format!("turn {index}: {}: {error}", category(&error)));
            }
            (None, Ok(())) => {}
        }
        let Some(expected) = turn.get("expect_surfaces").and_then(Value::as_object) else {
            continue;
        };
        let held: BTreeSet<&str> = state
            .surfaces()
            .map(|surface| surface.surface_id.as_str())
            .collect();
        let wanted: BTreeSet<&str> = expected.keys().map(String::as_str).collect();
        if held != wanted {
            return fail(format!(
                "turn {index}: expected surfaces {wanted:?}, the state holds {held:?}"
            ));
        }
        for (surface_id, expected) in expected {
            known_keys(expected, &["components", "data"], "an expected surface")?;
            let Some(surface) = state.surface(surface_id) else {
                return fail(format!("turn {index}: no surface {surface_id}"));
            };
            if let Some(components) = expected.get("components").and_then(Value::as_object) {
                let mut found = serde_json::Map::new();
                for component in &surface.components {
                    let mut properties = serde_json::to_value(component).unwrap_or(Value::Null);
                    if setup.lowered {
                        properties = lowering::lift_component(&properties);
                    }
                    if let Some(map) = properties.as_object_mut() {
                        map.remove("id");
                    }
                    found.insert(component.id.clone(), properties);
                }
                expect_same(
                    &Value::Object(components.clone()),
                    &Value::Object(found),
                    &format!("turn {index}: the components of {surface_id}"),
                )?;
            }
            if let Some(data) = expected.get("data") {
                let found = surface.data_model.to_json().unwrap_or(Value::Null);
                // A data model nothing was written to is an empty one.
                let found = if found.is_null() { json!({}) } else { found };
                expect_same(
                    data,
                    &found,
                    &format!("turn {index}: the data model of {surface_id}"),
                )?;
            }
        }
    }
    Ok(())
}

// --- streamed_answer.yaml -----------------------------------------------------------------------

fn stream_answer(case: &Value, args: &Value, lowered: &mut bool) -> Step {
    case_keys(case, &["args", "steps", "expect_close", "expect_error"])?;
    known_keys(args, &["format", "catalog", "renderer_catalogs"], "args")?;
    let setup = setup(case, args, false, lowered)?;
    let processor = ready_processor(args, &setup)?;
    // The renderer's own catalogs, when the case gives it others.
    let mut state = match args.get("renderer_catalogs") {
        None => processor.create_state(),
        Some(catalogs) => {
            let mut ignored = false;
            let renderer = self::setup(case, &json!({"catalogs": catalogs}), false, &mut ignored)?;
            RendererState::new(renderer.active())
                .or_else(|error| fail(format!("the renderer cannot be built: {error}")))?
        }
    };
    let Some(mut answer) = settle(
        case,
        StreamedAnswer::open(&processor, &state),
        "opening the answer",
    )?
    else {
        return Ok(());
    };
    let parts = |answer: &StreamedAnswer, state: &RendererState| -> Step<Value> {
        answer
            .parts()
            .iter()
            .map(|part| match part {
                AnswerPart::Text(text) => Ok(json!({"text": text})),
                // The part is the surface of that id the renderer holds.
                AnswerPart::Surface(id) if state.surface(id).is_some() => {
                    Ok(json!({"surface": id}))
                }
                AnswerPart::Surface(id) => fail(format!("the renderer does not hold surface {id}")),
                AnswerPart::Rejected(error) => Ok(json!({"error": {"category": category(error)}})),
            })
            .collect::<Step<Vec<Value>>>()
            .map(Value::Array)
    };
    let steps: Vec<&Value> = case
        .get("steps")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .collect();
    let inputs: Vec<&str> = steps
        .iter()
        .map(|step| {
            step.get("input")
                .and_then(Value::as_str)
                .unwrap_or_default()
        })
        .collect();
    for (index, (step, chunk)) in steps.iter().zip(setup.chunks(&inputs)).enumerate() {
        known_keys(
            step,
            &["input", "expect_parts", "expect_in_block"],
            "a step",
        )?;
        answer
            .feed(&chunk, &mut state)
            .or_else(|error| fail(format!("step {index}: {error}")))?;
        if let Some(expected) = step.get("expect_parts") {
            expect_same(
                expected,
                &parts(&answer, &state)?,
                &format!("step {index}: the answer's parts"),
            )?;
        }
        if let Some(expected) = step.get("expect_in_block").and_then(Value::as_bool) {
            if expected != answer.is_in_block() {
                return fail(format!("step {index}: expected in_block {expected}"));
            }
        }
    }
    answer
        .close(&mut state)
        .or_else(|error| fail(format!("closing the answer: {error}")))?;
    match case.get("expect_close") {
        Some(expected) => expect_same(
            expected,
            &parts(&answer, &state)?,
            "the answer's parts once closed",
        ),
        None => Ok(()),
    }
}

// --- direct_json/prompt_generator.yaml ----------------------------------------------------------

fn prompt_snippet(case: &Value, args: &Value, lowered: &mut bool) -> Step {
    case_keys(
        case,
        &[
            "args",
            "expect_file",
            "expect_contains",
            "expect_absent",
            "expect_deterministic",
            "expect_error",
        ],
    )?;
    known_keys(
        args,
        &["format", "catalogs", "examples", "allowed_messages"],
        "args",
    )?;
    let setup = setup(case, args, false, lowered)?;
    if setup.lowered && case.get("expect_error").is_none() {
        // The snippet embeds the schemas of the catalogs' version, and the
        // case states what it holds for v1.0, byte for byte.
        return skip(NEEDS_V1_0, "the snippet embeds the v1.0 schemas");
    }
    let examples = examples(args, &setup)?;
    let generate = || direct_json(args, &setup).prompt_snippet(&setup.active(), &examples);
    let Some(snippet) = settle(case, generate(), "generating the snippet")? else {
        return Ok(());
    };
    if let Some(golden) = case.get("expect_file").and_then(Value::as_str) {
        let golden = std::fs::read_to_string(conformance_dir().join(golden))
            .or_else(|error| fail(format!("cannot read the golden file: {error}")))?;
        if snippet != golden {
            return fail("the snippet differs from its golden file");
        }
    }
    snippet_checks(&snippet, case, "expect_contains", "expect_absent")?;
    if case.get("expect_deterministic") == Some(&Value::Bool(true))
        && generate().ok().as_ref() != Some(&snippet)
    {
        return fail("a second call returned another snippet");
    }
    Ok(())
}

// --- direct_json/response_parser.yaml -----------------------------------------------------------

fn raw_parts_json(parts: &[RawPart]) -> Value {
    Value::Array(
        parts
            .iter()
            .map(|part| match part {
                RawPart::Text(text) => json!({"text": text}),
                RawPart::Block { raw, closed: true } => json!({"a2ui_raw": raw}),
                RawPart::Block { raw, closed: false } => {
                    json!({"a2ui_raw": raw, "is_final": false})
                }
            })
            .collect(),
    )
}

/// A part is final unless it says otherwise, so `is_final: true` says nothing.
fn without_final_flags(parts: &Value) -> Value {
    let mut parts = parts.clone();
    for part in parts.as_array_mut().into_iter().flatten() {
        if part.get("is_final") == Some(&Value::Bool(true)) {
            if let Some(map) = part.as_object_mut() {
                map.remove("is_final");
            }
        }
    }
    parts
}

fn unwrap(case: &Value, args: &Value) -> Step {
    case_keys(case, &["args", "input", "expect"])?;
    known_keys(args, &["format"], "args")?;
    // Unwrapping keeps the payload as written, so nothing is lowered.
    let parts = DirectJson::new().unwrap(
        case.get("input")
            .and_then(Value::as_str)
            .unwrap_or_default(),
    );
    expect_same(
        &without_final_flags(case.get("expect").unwrap_or(&Value::Null)),
        &raw_parts_json(&parts),
        "the raw parts",
    )
}

fn has_format_content(case: &Value, args: &Value) -> Step {
    case_keys(case, &["args", "input", "expect"])?;
    known_keys(args, &["format", "complete"], "args")?;
    let found = DirectJson::new().has_content(
        case.get("input")
            .and_then(Value::as_str)
            .unwrap_or_default(),
        args.get("complete") == Some(&Value::Bool(true)),
    );
    expect_same(
        case.get("expect").unwrap_or(&Value::Null),
        &json!(found),
        "has_format_content",
    )
}

fn wrap(case: &Value, args: &Value) -> Step {
    case_keys(
        case,
        &[
            "args",
            "parts",
            "expect_output",
            "expect_contains",
            "expect_round_trip",
        ],
    )?;
    known_keys(args, &["format"], "args")?;
    let stated = case.get("parts").cloned().unwrap_or(json!([]));
    let parts: Vec<RawPart> = stated
        .as_array()
        .into_iter()
        .flatten()
        .map(|part| match (part.get("text"), part.get("a2ui_raw")) {
            (Some(Value::String(text)), None) => Ok(RawPart::Text(text.clone())),
            (None, Some(Value::String(raw))) => Ok(RawPart::Block {
                raw: raw.clone(),
                closed: part.get("is_final") != Some(&Value::Bool(false)),
            }),
            _ => fail(format!("not a raw part: {part}")),
        })
        .collect::<Step<_>>()?;
    let format = DirectJson::new();
    let wrapped = format.wrap(&parts);
    if let Some(expected) = case.get("expect_output").and_then(Value::as_str) {
        if wrapped != expected {
            return fail(format!("expected {expected:?}, got {wrapped:?}"));
        }
    }
    snippet_checks(&wrapped, case, "expect_contains", "")?;
    if case.get("expect_round_trip") == Some(&Value::Bool(true)) {
        expect_same(
            &without_final_flags(&stated),
            &raw_parts_json(&format.unwrap(&wrapped)),
            "the parts unwrapped again",
        )?;
    }
    Ok(())
}

fn parse_response(case: &Value, args: &Value, lowered: &mut bool) -> Step {
    case_keys(case, &["args", "input", "expect", "expect_error"])?;
    known_keys(args, &["format", "catalog", "wrapped"], "args")?;
    let setup = setup(case, args, true, lowered)?;
    let processor = ready_processor(args, &setup)?;
    let input = setup.text(
        case.get("input")
            .and_then(Value::as_str)
            .unwrap_or_default(),
    );
    let parsed = if args.get("wrapped") == Some(&Value::Bool(false)) {
        processor.parse_block_in(&input, &mut processor.create_state())
    } else {
        processor.parse_response(&input)
    };
    match settle(case, parsed.map_err(Error::from), "parsing the response")? {
        Some(parts) => expect_same(
            case.get("expect").unwrap_or(&Value::Null),
            &response_parts(&setup, &parts),
            "the parsed parts",
        ),
        None => Ok(()),
    }
}

// --- direct_json/compiler.yaml, decompiler.yaml -------------------------------------------------

/// Compiles one block and checks its messages on their own, the way
/// `validate_payload` checks a payload: a surface the block updates without
/// creating is assumed to exist.
fn compile_checked(setup: &Setup, block: &str) -> Result<Vec<Value>, Error> {
    let messages = DirectJson::new().compile(block, true)?;
    validate_payload(&setup.active(), &messages)?;
    Ok(messages)
}

fn compile(case: &Value, args: &Value, lowered: &mut bool) -> Step {
    case_keys(case, &["args", "input", "expect", "expect_error"])?;
    known_keys(args, &["format", "catalog"], "args")?;
    let setup = setup(case, args, true, lowered)?;
    let input = setup.text(
        case.get("input")
            .and_then(Value::as_str)
            .unwrap_or_default(),
    );
    match settle(case, compile_checked(&setup, &input), "compiling the block")? {
        Some(messages) => expect_same(
            case.get("expect").unwrap_or(&Value::Null),
            &Value::Array(setup.written(&messages)),
            "the compiled messages",
        ),
        None => Ok(()),
    }
}

fn decompile(case: &Value, args: &Value, lowered: &mut bool) -> Step {
    case_keys(
        case,
        &["args", "messages", "expect_contains", "expect_round_trip"],
    )?;
    known_keys(args, &["format", "catalog", "catalogs"], "args")?;
    let setup = setup(case, args, true, lowered)?;
    let stated = case.get("messages").cloned().unwrap_or(json!([]));
    let messages = setup.messages(stated.as_array().map(Vec::as_slice).unwrap_or_default())?;
    let written = DirectJson::new()
        .decompile(&messages)
        .or_else(|error| fail(format!("the messages cannot be written: {error}")))?;
    snippet_checks(&written, case, "expect_contains", "")?;
    if case.get("expect_round_trip") == Some(&Value::Bool(true)) {
        let recompiled = compile_checked(&setup, &written).or_else(|error| {
            fail(format!(
                "what was written does not compile: {error}\n{written}"
            ))
        })?;
        expect_same(
            &stated,
            &Value::Array(setup.written(&recompiled)),
            "the messages compiled back",
        )?;
    }
    Ok(())
}

// --- direct_json/response_streaming.yaml --------------------------------------------------------

fn parse_chunk(case: &Value, args: &Value, lowered: &mut bool) -> Step {
    case_keys(
        case,
        &[
            "args",
            "steps",
            "expect_close",
            "expect_matches_single_shot",
        ],
    )?;
    known_keys(
        args,
        &[
            "format",
            "catalog",
            "catalogs",
            "progressive_keys",
            "wrapped",
            "after_rejected_block",
        ],
        "args",
    )?;
    let setup = setup(case, args, true, lowered)?;
    let processor = ready_processor(args, &setup)?;
    let options = StreamOptions::new()
        .with_wrapped(args.get("wrapped") != Some(&Value::Bool(false)))
        .with_after_rejected_block(
            match args.get("after_rejected_block").and_then(Value::as_str) {
                Some("continue") => AfterRejectedBlock::Continue,
                _ => AfterRejectedBlock::End,
            },
        );
    let Some(yielded) = run_stream(&setup, &processor, case, options)? else {
        return Ok(());
    };
    if case.get("expect_matches_single_shot") == Some(&Value::Bool(true)) {
        let inputs: Vec<&str> = case
            .get("steps")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .map(|step| {
                step.get("input")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
            })
            .collect();
        let joined = setup.chunks(&inputs).concat();
        let whole = if options.wrapped {
            processor.parse_response(&joined)
        } else {
            processor.parse_block_in(&joined, &mut processor.create_state())
        }
        .or_else(|rejected| {
            fail(format!(
                "the chunks joined do not parse: {}",
                rejected.error
            ))
        })?;
        expect_same(
            &merge_text(&response_parts(&setup, &whole)),
            &merge_text(&yielded),
            "the stream against the answer read whole",
        )?;
    }
    Ok(())
}

// --- e2e/v0_9_1/*.yaml: the agent half of a transcript ------------------------------------------

/// Runs the agent half of a golden transcript in direct JSON: each turn's
/// chunks through a stream, and joined through one parse, both against a
/// state that lives across the turns, and both compared with the wire the
/// transcript expects. The renderer half (view trees, user input, what the
/// renderer sends back) is a renderer's.
fn transcript(case: &Value) -> Step {
    case_keys(
        case,
        &[
            "protocolVersion",
            "catalogPaths",
            "formats",
            "source",
            "turns",
        ],
    )?;
    let stated = case
        .get("protocolVersion")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let Some(version) = A2uiVersion::parse(stated) else {
        return skip(NEEDS_V1_0, format!("a {stated} transcript"));
    };
    let catalogs = names(case.get("catalogPaths"))
        .iter()
        .map(|path| {
            // The published basic catalog is the one this crate pins, as the
            // reference harnesses substitute their own copy of it.
            if path.ends_with("catalogs/basic/catalog.json") {
                CatalogDocument::basic(version)
                    .or_else(|error| fail(format!("the basic catalog does not load: {error}")))
            } else {
                fail(format!(
                    "the transcript names a catalog that is not vendored: {path}"
                ))
            }
        })
        .collect::<Step<Vec<_>>>()?;
    let as_written = Setup {
        catalogs: Vec::new(),
        lowered: false,
        inject: None,
    };
    let processor = A2uiRequestProcessor::new(catalogs, Vec::new(), Arc::new(DirectJson::new()))
        .or_else(|error| fail(format!("the processor cannot be built: {error}")))?;
    let mut streamed_into = processor.create_state();
    let mut parsed_into = processor.create_state();

    for (index, turn) in case
        .get("turns")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .enumerate()
    {
        known_keys(
            turn,
            &[
                "userText",
                "modelChunks",
                "expectWire",
                "expectViewTree",
                "expectErrors",
                "uiOps",
                "expectDataModel",
                "expectOutbound",
            ],
            "a turn",
        )?;
        let Some(chunks) = turn
            .get("modelChunks")
            .and_then(|chunks| chunks.get("direct_json"))
        else {
            return fail(format!("turn {index} has no direct_json chunks"));
        };
        let chunks = names(Some(chunks));
        let wire = turn.get("expectWire").cloned().unwrap_or(json!([]));

        // A transcript may have the model update a surface that was never
        // created, to show what the renderer answers. The agent half is not
        // what rejects it there, so the surface is assumed, as the reference
        // harnesses assume it.
        let messages: Vec<&Value> = wire
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|part| part.get("a2ui").and_then(Value::as_array))
            .flatten()
            .collect();
        let created: BTreeSet<&str> = messages
            .iter()
            .filter_map(|message| message["createSurface"]["surfaceId"].as_str())
            .collect();
        for surface_id in messages
            .iter()
            .filter_map(|message| message["updateComponents"]["surfaceId"].as_str())
            .filter(|surface_id| !created.contains(surface_id))
        {
            let assumed = json!({"version": version.as_str(), "createSurface": {
                "surfaceId": surface_id,
                "catalogId": processor.active_catalogs()[0].catalog_id(),
            }});
            for state in [&mut streamed_into, &mut parsed_into] {
                if state.surface(surface_id).is_none() {
                    state
                        .apply(std::slice::from_ref(&assumed))
                        .or_else(|error| fail(format!("turn {index}: {error}")))?;
                }
            }
        }

        let mut stream = open_stream(&processor, &streamed_into, StreamOptions::default())?;
        let mut parts = Vec::new();
        for chunk in &chunks {
            parts.extend(stream.feed(chunk, &mut streamed_into).or_else(|rejected| {
                fail(format!(
                    "turn {index}: the stream: {}",
                    brief(&rejected.error)
                ))
            })?);
        }
        parts.extend(stream.close(&mut streamed_into).or_else(|rejected| {
            fail(format!(
                "turn {index}: closing the stream: {}",
                brief(&rejected.error)
            ))
        })?);
        expect_same(
            &wire,
            &merge_text(&stream_parts(&as_written, &parts)),
            &format!("turn {index}: the streamed wire"),
        )?;

        let whole = processor
            .parse_response_in(&chunks.concat(), &mut parsed_into)
            .or_else(|rejected| {
                fail(format!(
                    "turn {index}: read whole: {}",
                    brief(&rejected.error)
                ))
            })?;
        expect_same(
            &wire,
            &merge_text(&response_parts(&as_written, &whole)),
            &format!("turn {index}: the wire read whole"),
        )?;
    }
    Ok(())
}
