//! Harness for the upstream language-agnostic A2UI conformance suite.
//!
//! The A2UI project ships its conformance tests as YAML rather than as
//! per-language test code, and asks every SDK to run them: read the YAML, feed
//! the inputs to that language's implementation, assert the outputs. The suite
//! is vendored whole under `tests/conformance/`; see the README there for the
//! upstream commit, the layout and the standing of every suite.
//!
//! # Every suite is accounted for
//!
//! [`SUITES`] names every vendored suite file and says what this crate does
//! with it. A suite is either executed, case by case, or not executed at all,
//! with the reason. Each row also states how many of its checks end in each of
//! four states — passed, expected divergence, skipped, failing — and
//! [`conformance_suite`] fails when a count is anything else. So a check cannot
//! stop passing, become a skip, or start passing without the table changing,
//! and a suite that upstream adds cannot arrive unclassified.
//!
//! A check is never counted as passing unless this crate actually produced the
//! expected outcome.
//!
//! Run with `cargo test -p ag-ui-a2ui --all-features --test conformance --
//! --nocapture` to see the report, and set `A2UI_CONFORMANCE_VERBOSE=1` to list
//! every check by name. The harness is compiled only with the
//! `schema-validation` feature, which `--all-features` turns on.

// The catalog and common-types suites validate against the published schema
// documents, so the harness needs the JSON Schema engine; `schema-validation`
// brings `toolkit` with it.
#![cfg(feature = "schema-validation")]

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use ag_ui_a2ui::binding::{Scope, absolute_path, parse_template};
use ag_ui_a2ui::catalog::Catalog;
use ag_ui_a2ui::processor::{MessageProcessor, Rejection, RejectionKind};
use ag_ui_a2ui::schema_validation::{SchemaValidator, validate_common_type};
use ag_ui_a2ui::surface::{Lifecycle, SurfaceStore};
use ag_ui_a2ui::toolkit::negotiate::{CatalogRegistry, ClientCapabilities, select_catalog_schema};
use ag_ui_a2ui::toolkit::parser::{has_a2ui_parts, parse_and_fix, parse_response};
use ag_ui_a2ui::toolkit::prompt::{GENERATION_GUIDELINES, PromptSpec, build_subagent_prompt};
use ag_ui_a2ui::toolkit::schema::{SchemaBundle, load_examples, remove_strict_validation};
use ag_ui_a2ui::toolkit::streaming::StreamParser;
use ag_ui_a2ui::validate::{ErrorCode, ValidateOptions, ValidationReport, Validator};
use ag_ui_a2ui::version::A2uiVersion;
use ag_ui_a2ui::{DataModel, DataModelUpdate, Error, ModelValue};
use serde_json::Value;

/// Upstream commit the vendored suite was taken from.
const UPSTREAM_COMMIT: &str = "3ed9601862ecc78027e20ac80ad8b1b98a5d9079";

/// Upstream commit of the suites under `retired/`, which upstream has since
/// deleted or rewritten under other names.
const RETIRED_COMMIT: &str = "44a420b67957fafc0b02d55a153fdaf72e32ffb5";

/// Why a whole suite is not executed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Why {
    /// It tests the agent layer, which the next pull request of #41 builds.
    AgentLayer,
    /// It tests a transport adapter, which #43 builds.
    Adapters,
    /// It tests A2UI v1.0, which #42 adds.
    V1_0,
    /// It tests the v0.8 wire format, which this crate does not speak.
    V0_8,
    /// It tests a renderer, and this crate does not render.
    Renderer,
    /// It asks something of an SDK that does not apply to this one.
    NotApplicable,
}

impl Why {
    /// The label the per-suite table carries.
    fn label(self) -> &'static str {
        match self {
            Why::AgentLayer => "later: agent layer (#41)",
            Why::Adapters => "later: adapters (#43)",
            Why::V1_0 => "later: A2UI v1.0 (#42)",
            Why::V0_8 => "out of scope: v0.8",
            Why::Renderer => "out of scope: renderer",
            Why::NotApplicable => "not applicable",
        }
    }
}

/// What this crate does with one vendored suite.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Plan {
    /// Its cases are executed. One of them may still be skipped, with a reason.
    Run,
    /// A suite as it stood at [`RETIRED_COMMIT`], before upstream deleted it or
    /// rewrote its cases under other names. It is kept, and executed whole,
    /// because the code it covers is still here: what passed at that pin goes
    /// on being checked until that code is replaced.
    Retired,
    /// Nothing in it is executed. The sentence says what it tests.
    Skip(Why, &'static str),
}

/// One vendored suite, and the standing this crate is held to on it.
#[derive(Debug, Clone, Copy)]
struct Suite {
    /// Path under `tests/conformance/`.
    path: &'static str,
    plan: Plan,
    /// Checks in each state: passed, expected divergence, skipped, failing.
    counts: [usize; 4],
}

const fn run(path: &'static str, counts: [usize; 4]) -> Suite {
    Suite {
        path,
        plan: Plan::Run,
        counts,
    }
}

const fn retired(path: &'static str, counts: [usize; 4]) -> Suite {
    Suite {
        path,
        plan: Plan::Retired,
        counts,
    }
}

/// A suite none of whose `cases` is executed.
const fn skip(path: &'static str, cases: usize, why: Why, what: &'static str) -> Suite {
    Suite {
        path,
        plan: Plan::Skip(why, what),
        counts: [0, 0, cases, 0],
    }
}

/// Every vendored suite. The order is the order of the report.
#[rustfmt::skip]
const SUITES: &[Suite] = &[
    // --- executed --------------------------------------------------------------------------
    run("core/validator_v0_9.yaml", [39, 3, 5, 0]),
    run("agent/legacy/parser.yaml", [22, 0, 0, 1]),
    run("agent/legacy/inference_format.yaml", [12, 0, 10, 0]),
    run("agent/legacy/streaming_parser.yaml", [33, 8, 45, 2]),
    run("core/data_context.yaml", [9, 2, 0, 0]),
    run("core/data_model.yaml", [43, 4, 1, 4]),
    run("core/expressions.yaml", [92, 0, 6, 0]),
    run("core/message_processor_v0_9.yaml", [40, 2, 4, 2]),
    run("core/catalog.yaml", [14, 2, 54, 3]),
    run("core/common_types.yaml", [8, 0, 14, 0]),
    retired("retired/core/validator.yaml", [26, 0, 25, 0]),
    retired("retired/core/catalog.yaml", [23, 0, 1, 0]),
    retired("retired/agent/inference_format.yaml", [13, 0, 6, 0]),

    // --- core tier: A2UI v1.0 ------------------------------------------------------------------
    skip("core/validator_v1_0.yaml", 49, Why::V1_0, "envelope, schema and topology validation of v1.0 messages"),
    skip("core/message_processor_v1_0.yaml", 84, Why::V1_0, "the v1.0 message processor"),
    skip("core/message_processor_scaling.yaml", 3, Why::V1_0, "timing of the v1.0 message processor"),
    skip("core/data_deletion.yaml", 7, Why::V1_0, "updateDataModel removal by null"),
    skip("core/data_model_pointers.yaml", 7, Why::V1_0, "data-model pointer rules in v1.0 messages"),
    skip("core/reserved_keys.yaml", 10, Why::V1_0, "`@`-prefixed reserved keys"),
    skip("core/multi_catalog.yaml", 8, Why::V1_0, "several catalogs on one surface"),
    skip("core/composition_constraints.yaml", 10, Why::V1_0, "allowedParents and allowedChildren through the v1.0 processor"),
    skip("core/index_function.yaml", 4, Why::V1_0, "the `@index` function in v1.0 templates"),
    skip("core/validation_result.yaml", 6, Why::V1_0, "per-component validation results"),
    skip("core/rpc_functions.yaml", 33, Why::V1_0, "callRendererFunction and its response"),
    skip("core/agent_function_routing.yaml", 43, Why::V1_0, "routing function calls to the agent with callAgentFunction"),

    // --- core tier: not this crate's -------------------------------------------------------------
    skip("core/validator_v0_8.yaml", 27, Why::V0_8, "validation of v0.8 messages"),
    skip("core/message_processor_v0_8.yaml", 9, Why::V0_8, "the v0.8 message processor"),
    skip("core/agent_to_renderer.yaml", 4, Why::NotApplicable, "a message schema generated from an SDK's own types against the published one; this crate ships the published schema"),
    skip("core/functions.yaml", 116, Why::Renderer, "evaluating the basic catalog functions, which run on the renderer"),
    skip("core/actions.yaml", 18, Why::Renderer, "dispatching a user action from a rendered component"),
    skip("core/error_responses.yaml", 24, Why::Renderer, "the error messages a renderer sends back to the agent"),
    skip("core/node_resolution.yaml", 31, Why::Renderer, "materialising the reactive component node tree"),
    skip("core/accessibility.yaml", 7, Why::Renderer, "accessibility attributes of rendered components"),
    skip("core/adapter_conformance.yaml", 22, Why::Renderer, "rendered view hierarchies in the UI framework adapters"),

    // --- agent tier ----------------------------------------------------------------------------
    skip("agent/request_processor.yaml", 29, Why::AgentLayer, "building an inference format and a per-request processor"),
    skip("agent/catalog_resolution.yaml", 23, Why::AgentLayer, "negotiating renderer capabilities against registered catalogs"),
    skip("agent/catalog_provider.yaml", 10, Why::AgentLayer, "loading catalog documents from the filesystem"),
    skip("agent/catalog_transformer.yaml", 15, Why::AgentLayer, "pruning a catalog before it reaches a prompt or a validator"),
    skip("agent/renderer_capabilities.yaml", 24, Why::AgentLayer, "reading renderer capabilities from transport metadata"),
    skip("agent/renderer_state.yaml", 9, Why::AgentLayer, "renderer state across turns"),
    skip("agent/skill.yaml", 12, Why::AgentLayer, "skill documents generated from a format and its catalogs"),
    skip("agent/streamed_answer.yaml", 6, Why::AgentLayer, "reading a streamed answer into a message processor"),
    skip("agent/direct_json/prompt_generator.yaml", 15, Why::AgentLayer, "the direct JSON format: system prompt snippet"),
    skip("agent/direct_json/response_parser.yaml", 36, Why::AgentLayer, "the direct JSON format: unwrapping and parsing a response"),
    skip("agent/direct_json/response_streaming.yaml", 139, Why::AgentLayer, "the direct JSON format: parsing a response chunk by chunk"),
    skip("agent/direct_json/compiler.yaml", 38, Why::AgentLayer, "the direct JSON format: compiling a block into messages"),
    skip("agent/direct_json/decompiler.yaml", 11, Why::AgentLayer, "the direct JSON format: writing messages back as notation"),
    skip("agent/direct_json/scaling.yaml", 7, Why::AgentLayer, "the direct JSON format: parse time against payload size"),
    skip("agent/express/prompt_generator.yaml", 23, Why::AgentLayer, "the Express format: system prompt snippet"),
    skip("agent/express/response_parser.yaml", 38, Why::AgentLayer, "the Express format: unwrapping and parsing a response"),
    skip("agent/express/response_streaming.yaml", 41, Why::AgentLayer, "the Express format: parsing a response chunk by chunk"),
    skip("agent/express/compiler.yaml", 135, Why::AgentLayer, "the Express format: compiling a block into messages"),
    skip("agent/express/decompiler.yaml", 49, Why::AgentLayer, "the Express format: writing messages back as notation"),
    skip("agent/express/scaling.yaml", 1, Why::AgentLayer, "the Express format: compile time against payload size"),
    skip("agent/atom/compiler.yaml", 7, Why::AgentLayer, "the Atom format"),
    skip("agent/elemental/compiler.yaml", 8, Why::AgentLayer, "the Elemental format"),
    skip("agent/builder/builder.yaml", 22, Why::AgentLayer, "the typesafe builder API"),
    skip("agent/macros/macros.yaml", 70, Why::AgentLayer, "builder macros"),
    skip("agent/multi_catalog_formats.yaml", 35, Why::V1_0, "components and function calls resolved across several active catalogs"),
    skip("agent/agent_functions.yaml", 64, Why::V1_0, "agent functions and the agent's side of renderer messages"),
    skip("agent/renderer_function_calls.yaml", 30, Why::V1_0, "the agent calling renderer functions"),

    // --- end to end: the agent half parses a transcript, a renderer half draws it ---------------
    skip("e2e/v0_9_1/login_form_action.yaml", 1, Why::AgentLayer, "a v0.9.1 transcript through the agent layer's response parser"),
    skip("e2e/v0_9_1/missing_surface_error.yaml", 1, Why::AgentLayer, "a v0.9.1 transcript through the agent layer's response parser"),
    skip("e2e/v0_9_1/newsletter_signup.yaml", 1, Why::AgentLayer, "a v0.9.1 transcript through the agent layer's response parser"),
    skip("e2e/v0_9_1/restaurant_list_booking.yaml", 1, Why::AgentLayer, "a v0.9.1 transcript through the agent layer's response parser"),
    skip("e2e/login_form_action.yaml", 1, Why::V1_0, "a v1.0 transcript"),
    skip("e2e/missing_surface_error.yaml", 1, Why::V1_0, "a v1.0 transcript"),
    skip("e2e/newsletter_signup.yaml", 1, Why::V1_0, "a v1.0 transcript"),
    skip("e2e/restaurant_list_booking.yaml", 1, Why::V1_0, "a v1.0 transcript"),

    // --- upstream's adapter tier is its UI framework adapters, that is, renderers ---------------
    skip("adapter/adapter_conformance_v0_9.yaml", 16, Why::Renderer, "rendered view hierarchies in the UI framework adapters"),
    skip("adapter/markdown_subset.yaml", 7, Why::Renderer, "rendering the Markdown subset"),

    // --- extensions: transport and framework integrations ---------------------------------------
    skip("extensions/a2a/a2a_integration.yaml", 11, Why::Adapters, "A2A events and parts"),
    skip("extensions/adk/adk_extensions.yaml", 8, Why::NotApplicable, "integration with Google's ADK, which this workspace does not target"),
];

fn conformance_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/conformance")
}

/// Reads a key a suite may spell either way.
///
/// Upstream renamed its case vocabulary from `snake_case` to `camelCase`
/// between the two pins this harness reads. Call sites name the key the old
/// way; the suites under `retired/` use that spelling and the current ones use
/// the new.
fn field<'a>(value: &'a Value, snake: &str) -> Option<&'a Value> {
    value.get(snake).or_else(|| {
        let mut camel = String::with_capacity(snake.len());
        let mut upper = false;
        for ch in snake.chars() {
            match ch {
                '_' => upper = true,
                _ if upper => {
                    camel.extend(ch.to_uppercase());
                    upper = false;
                }
                _ => camel.push(ch),
            }
        }
        value.get(camel.as_str())
    })
}

/// The cases of a suite: a list at the top, or under `tests` in the two
/// suites that carry a preamble.
fn cases_of(suite: &str) -> Vec<Value> {
    let path = conformance_dir().join(suite);
    let text = std::fs::read_to_string(&path)
        .unwrap_or_else(|e| panic!("cannot read vendored suite {}: {e}", path.display()));
    let document: Value = serde_norway::from_str(&readable(&text))
        .unwrap_or_else(|e| panic!("cannot parse vendored suite {suite}: {e}"));
    match document {
        Value::Array(cases) => cases,
        Value::Object(mut map) => match map.remove("tests") {
            Some(Value::Array(cases)) => cases,
            _ => panic!("{suite} is neither a list of cases nor a map with `tests`"),
        },
        _ => panic!("{suite} is not a list of cases"),
    }
}

/// A suite's text with half a surrogate pair made readable.
///
/// One streaming case expects a string that ends in a lone high surrogate, and
/// writes it as the YAML escape `"\ud83d"`. That is a string in the languages
/// the suite was written for. It is not one in Rust, and a YAML document that
/// contains it does not parse here at all. The escape is read as U+FFFD so the
/// rest of the suite loads; the case that carries it is pinned as a divergence
/// by name ([`streaming_divergence`]) rather than compared.
///
/// Only a double-quoted scalar is an escape. The same characters inside a
/// single-quoted one are the two halves of a pair split across chunks, which
/// is the input the case is about, and are left alone.
fn readable(text: &str) -> String {
    let lone_high_surrogate = |line: &str| -> Option<usize> {
        let quoted = line.contains(": \"") && line.trim_end().ends_with('"');
        let at = line.find("\\ud").or_else(|| line.find("\\uD"))?;
        let escape = line.get(at..at + 6)?;
        let unit = u16::from_str_radix(&escape[2..], 16).ok()?;
        let paired = line
            .get(at + 6..at + 12)
            .and_then(|next| u16::from_str_radix(next.strip_prefix("\\u")?, 16).ok())
            .is_some_and(|next| (0xDC00..=0xDFFF).contains(&next));
        (quoted && (0xD800..=0xDBFF).contains(&unit) && !paired).then_some(at)
    };
    text.lines()
        .map(|line| match lone_high_surrogate(line) {
            Some(at) => format!("{}\\uFFFD{}", &line[..at], &line[at + 6..]),
            None => line.to_string(),
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn case_name(case: &Value) -> &str {
    case.get("name")
        .or_else(|| case.get("id"))
        .and_then(Value::as_str)
        .unwrap_or("<unnamed>")
}

/// Every suite file in the vendored tree, relative to it.
fn suites_on_disk() -> BTreeSet<String> {
    fn walk(dir: &Path, root: &Path, out: &mut BTreeSet<String>) {
        let entries =
            std::fs::read_dir(dir).unwrap_or_else(|e| panic!("cannot list {}: {e}", dir.display()));
        for entry in entries {
            let path = entry.expect("directory entry").path();
            let relative = path
                .strip_prefix(root)
                .expect("under the vendored tree")
                .to_string_lossy()
                .replace('\\', "/");
            if path.is_dir() {
                // Fixtures and goldens are data the suites load, not suites.
                let data = relative.ends_with("test_data")
                    || relative.ends_with("golden")
                    || relative == "specification";
                if !data {
                    walk(&path, root, out);
                }
            } else if relative.ends_with(".yaml") && relative != "features.yaml" {
                out.insert(relative);
            }
        }
    }
    let root = conformance_dir();
    let mut out = BTreeSet::new();
    walk(&root, &root, &mut out);
    out
}

/// The outcome of one check within a case.
#[derive(Debug)]
enum Outcome {
    Passed,
    ExpectedDivergence(String),
    Skipped(String),
    Failed(String),
}

#[derive(Debug, Default)]
struct Tally {
    passed: usize,
    divergent: usize,
    skipped: usize,
    /// Checks that fail and are listed in [`KNOWN_FAILURES`].
    failing: usize,
    divergence_reasons: BTreeMap<String, usize>,
    skip_reasons: BTreeMap<String, usize>,
    failing_reasons: BTreeMap<String, usize>,
    /// Failures nothing lists, and listed failures that did not happen.
    unexpected: Vec<String>,
}

impl Tally {
    fn counts(&self) -> [usize; 4] {
        [self.passed, self.divergent, self.skipped, self.failing]
    }

    fn record(&mut self, check: &str, outcome: Outcome) {
        if verbose() {
            println!("    {outcome:?} {check}");
        }
        let known = KNOWN_FAILURES
            .iter()
            .find(|(listed, _)| *listed == check)
            .map(|(_, reason)| *reason);
        match (outcome, known) {
            (Outcome::Failed(_), Some(reason)) => {
                self.failing += 1;
                *self.failing_reasons.entry(reason.to_string()).or_default() += 1;
            }
            (Outcome::Failed(detail), None) => self.unexpected.push(format!("{check}: {detail}")),
            (_, Some(_)) => self.unexpected.push(format!(
                "{check}: listed in KNOWN_FAILURES but it no longer fails; remove it from the list"
            )),
            (Outcome::Passed, None) => self.passed += 1,
            (Outcome::ExpectedDivergence(reason), None) => {
                self.divergent += 1;
                *self.divergence_reasons.entry(reason).or_default() += 1;
            }
            (Outcome::Skipped(reason), None) => {
                self.skipped += 1;
                *self.skip_reasons.entry(reason).or_default() += 1;
            }
        }
    }
}

/// Checks that fail today, by the id the report prints, each with the reason.
///
/// A check listed here must fail: one that starts passing fails the suite
/// until it is taken off the list, so the list cannot outlive its entries.
const KNOWN_FAILURES: &[(&str, &str)] = &[
    // Three cases upstream added after the old pin. All three are about the
    // toolkit's parsers, which the agent layer replaces.
    (
        "agent/legacy/parser.yaml::test_fix_payload_unescaped_latex_backslashes_diagnostic_hint",
        "an invalid escape is rejected with the JSON parser's message, without the hint \
         'Unescaped backslash found'",
    ),
    (
        "agent/legacy/streaming_parser.yaml::test_no_placeholders_without_placeholder_component_v09",
        "with validation off, a `Row` placeholder is emitted for a catalog that defines no `Row`",
    ),
    (
        "agent/legacy/streaming_parser.yaml::test_cut_escape_sequences_v09",
        "a second `<a2ui-json>` inside a block that is still open is read as JSON, and the \
         messages after it are not dispatched",
    ),
    // Four rules of the reference data model this crate does not have. The
    // official renderer core that `examples/review-desk/interop` runs against
    // pads on the first of them, as this crate does.
    (
        "core/data_model.yaml::test_data_model_delete_out_of_range_list_index_is_noop[step 1]",
        "removing an index past the end of a list pads the list out to it with undefined slots",
    ),
    (
        "core/data_model.yaml::test_data_model_delete_absent_deep_path_is_noop[step 0]",
        "removing a path that does not exist creates the containers on the way to it",
    ),
    (
        "core/data_model.yaml::test_data_model_rejects_forbidden_path_segments[step 0]",
        "`__proto__`, `constructor` and `prototype` are ordinary keys",
    ),
    (
        "core/data_model.yaml::test_data_model_rejects_excessive_list_index[step 2]",
        "a write may grow a list by 65,536 slots (DEFAULT_ARRAY_GROWTH_LIMIT), whatever the \
         index; the reference refuses an index above 10,000",
    ),
    // Two refusals only a JSON Schema engine makes. The processor checks what
    // the semantic validator checks; `SchemaValidator` is the engine, and it
    // takes whole schema documents.
    (
        "core/message_processor_v0_9.yaml::test_create_surface_strict_theme_validation_failure",
        "a theme is not checked against the catalog's theme schema",
    ),
    (
        "core/message_processor_v0_9.yaml::test_v09_rejects_at_prefixed_component_envelope_keys",
        "a property the component does not declare, here `@metadata`, is accepted",
    ),
    // The catalog this crate builds in is a summary written in Rust.
    (
        "core/catalog.yaml::test_v09_basic_catalog_from_json",
        "Catalog::basic() is not the published v0.9 catalog read back: it has its own id, \
         declares `checks` itself where the document takes it from the Checkable mixin, and \
         has no List.listStyle",
    ),
    (
        "core/catalog.yaml::test_v09_basic_catalog_to_json",
        "Catalog::basic() was not read from a document and has none to give",
    ),
    (
        "core/catalog.yaml::test_v09_published_vegachart_catalog_is_self_contained",
        "no SchemaValidator can be built for a catalog that declares no `$defs/theme`, which \
         the envelope schema refers to",
    ),
];

/// Set `A2UI_CONFORMANCE_VERBOSE=1` to list every executed check by name.
fn verbose() -> bool {
    std::env::var_os("A2UI_CONFORMANCE_VERBOSE").is_some()
}

/// Checks where this crate does something else on purpose, by the id the
/// report prints: what this crate gives there, as the runner writes it, and
/// why.
///
/// Both halves are held. The case has to go on expecting something else, and
/// this crate has to go on giving exactly what is written here: a divergence
/// that changes, in either direction, fails the suite.
const DIVERGENCES: &[(&str, &str, &str)] = &[
    (
        "core/data_context.yaml::test_resolve_path_context_with_trailing_slash",
        r#""/bar//foo""#,
        TRAILING_SLASH,
    ),
    (
        "core/data_context.yaml::test_resolve_path_empty_in_trailing_slash_context",
        r#""/user/""#,
        TRAILING_SLASH,
    ),
    // `/foo/` is the key `""` under `foo`, so `/foo` holds an object.
    (
        "core/data_model.yaml::test_data_model_normalizes_trailing_slash[step 2]",
        r#"{"":"bar"}"#,
        TRAILING_SLASH,
    ),
    // `//a///b//` writes under the key `""`, so `/a/b` holds nothing.
    (
        "core/data_model.yaml::test_data_model_normalizes_empty_path_segments[step 0]",
        "notified []",
        TRAILING_SLASH,
    ),
    // `/items/-1`.
    (
        "core/data_model.yaml::test_data_model_get_out_of_range_index[step 1]",
        "pointer error: expected a canonical array index",
        NOT_AN_INDEX,
    ),
    // `/items/01`.
    (
        "core/data_model.yaml::test_data_model_rejects_leading_zero_list_index[step 0]",
        "pointer error: expected a canonical array index",
        NOT_AN_INDEX,
    ),
    (
        "core/message_processor_v0_9.yaml::test_resolve_path_relative_trailing_slash",
        r#""/bar//foo""#,
        TRAILING_SLASH,
    ),
    (
        "core/catalog.yaml::test_v09_published_minimal_catalog_is_self_contained",
        NOT_BUNDLED,
        REGISTRY,
    ),
    (
        "core/catalog.yaml::test_v091_published_basic_catalog_is_self_contained",
        NOT_BUNDLED,
        REGISTRY,
    ),
    // The same difference as in `core/validator_v0_9.yaml`: the component is
    // found and reported, and the message is not refused for it.
    (
        "core/message_processor_v0_9.yaml::test_topology_orphaned_component_error",
        r#"accepted, with unreachable ["orphan_comp"] on s1"#,
        "an unreachable component is a warning here, an error upstream",
    ),
];

const TRAILING_SLASH: &str =
    "an empty path segment is the empty key here, as RFC 6901 reads it; the reference drops it";

const REGISTRY: &str = "this crate bundles no schema: a catalog's references resolve through the \
                        validator's registry of local documents";

const NOT_AN_INDEX: &str = "reading a list at a segment that is not an index is an error here, as the \
     specification says of a path; the reference reads no value";

/// Where a case run as a whole stopped, and what this crate did there.
#[derive(Debug)]
struct Miss {
    /// The step it stopped at, in a case that has steps.
    step: Option<usize>,
    /// What this crate did, written the way [`DIVERGENCES`] pins it.
    actual: String,
    /// What the case wanted instead.
    expected: String,
}

impl Miss {
    fn new(actual: impl Into<String>, expected: impl Into<String>) -> Self {
        Self {
            step: None,
            actual: actual.into(),
            expected: expected.into(),
        }
    }

    fn at(mut self, step: usize) -> Self {
        self.step = Some(step);
        self
    }
}

/// Records a case that is one check: passed, or stopped at one place.
///
/// A case with steps is recorded under the step it stopped at,
/// `suite::case[step 2]`, so that a listed failure or divergence is the one at
/// that step and not an earlier one that appeared later.
fn settle(check: &str, result: Result<(), Miss>, tally: &mut Tally) {
    match result {
        Ok(()) => {
            let pinned = DIVERGENCES.iter().any(|(listed, ..)| {
                listed
                    .strip_prefix(check)
                    .is_some_and(|rest| rest.is_empty() || rest.starts_with("[step "))
            });
            let outcome = if pinned {
                Outcome::Failed(
                    "listed in DIVERGENCES, and it now does what the case expects; remove it from \
                     the list"
                        .into(),
                )
            } else {
                Outcome::Passed
            };
            tally.record(check, outcome);
        }
        Err(miss) => {
            let id = match miss.step {
                Some(step) => format!("{check}[step {step}]"),
                None => check.to_string(),
            };
            let outcome = match DIVERGENCES.iter().find(|(listed, ..)| *listed == id) {
                Some((_, pinned, reason)) if *pinned == miss.actual => {
                    Outcome::ExpectedDivergence((*reason).to_string())
                }
                Some((_, pinned, _)) => Outcome::Failed(format!(
                    "the pinned divergence changed: this crate gave {pinned}, and now gives {}",
                    miss.actual
                )),
                None => Outcome::Failed(format!("expected {}, got {}", miss.expected, miss.actual)),
            };
            tally.record(&id, outcome);
        }
    }
}

/// The first key of `value` that `known` does not list.
///
/// A runner names every key it reads. A key it does not know is an input or
/// an expectation it would otherwise ignore, and a case with one fails rather
/// than passing on the part that was understood.
fn unknown_key(value: &Value, known: &[&str], what: &str) -> Result<(), Miss> {
    let unknown = value
        .as_object()
        .and_then(|object| object.keys().find(|key| !known.contains(&key.as_str())));
    match unknown {
        Some(key) => Err(Miss::new(
            format!("the runner does not read {what} key {key:?}"),
            "every key of the case to be read",
        )),
        None => Ok(()),
    }
}

/// The keys every case carries, whatever its action.
const CASE_KEYS: [&str; 6] = [
    "name",
    "description",
    "features",
    "action",
    "catalog",
    "protocolVersion",
];

fn tally_suite(suite: &Suite) -> (usize, Tally) {
    let cases = cases_of(suite.path);
    let mut tally = Tally::default();
    for case in &cases {
        let check = format!("{}::{}", suite.path, case_name(case));
        match suite.plan {
            Plan::Skip(why, what) => {
                tally.record(&check, Outcome::Skipped(format!("{}: {what}", why.label())));
            }
            Plan::Retired => run_case(&check, case, Format::Retired, &mut tally),
            Plan::Run => run_case(&check, case, Format::Current, &mut tally),
        }
    }
    (cases.len(), tally)
}

/// Which case vocabulary a suite is written in.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Format {
    /// The layout at [`UPSTREAM_COMMIT`]: stateful `steps` of `messages`.
    Current,
    /// The layout at [`RETIRED_COMMIT`]: a `payload` per case or step.
    Retired,
}

/// The row of the per-suite table for one suite.
fn table_row(suite: &Suite, cases: usize, counts: [usize; 4]) -> String {
    let [passed, divergent, skipped, failing] = counts;
    let note = match suite.plan {
        Plan::Run => "executed".to_string(),
        Plan::Retired => "executed; retired upstream".to_string(),
        Plan::Skip(why, what) => format!("{}: {what}", why.label()),
    };
    format!(
        "| `{}` | {cases} | {passed} | {divergent} | {skipped} | {failing} | {note} |",
        suite.path
    )
}

#[test]
fn conformance_suite() {
    println!("\n=== A2UI conformance (upstream {UPSTREAM_COMMIT}, retired {RETIRED_COMMIT}) ===");

    // A suite the table does not name would be read by nothing, and a row
    // with no file behind it would assert nothing.
    let listed: BTreeSet<String> = SUITES.iter().map(|suite| suite.path.to_string()).collect();
    assert_eq!(listed.len(), SUITES.len(), "a suite is listed twice");
    assert_eq!(
        suites_on_disk(),
        listed,
        "the vendored tree and SUITES name different suites (left: on disk, right: listed)"
    );

    let mut total = Tally::default();
    let mut mismatches = Vec::new();
    let mut rows = Vec::new();

    for suite in SUITES {
        let (cases, tally) = tally_suite(suite);
        let counts = tally.counts();
        println!(
            "{:<44} cases {:>3}   checks: {} passed, {} expected divergence, {} skipped, {} failing",
            suite.path, cases, counts[0], counts[1], counts[2], counts[3]
        );
        if !matches!(suite.plan, Plan::Skip(..)) {
            for (reason, count) in &tally.skip_reasons {
                println!("    skipped x{count:<3} {reason}");
            }
            for (reason, count) in &tally.divergence_reasons {
                println!("    expected divergence x{count:<3} {reason}");
            }
            for (reason, count) in &tally.failing_reasons {
                println!("    failing x{count:<3} {reason}");
            }
        }
        for failure in &tally.unexpected {
            println!("    UNEXPECTED  {failure}");
        }
        if counts != suite.counts {
            mismatches.push(format!(
                "{}: expected {:?}, got {counts:?} (passed, expected divergence, skipped, failing)",
                suite.path, suite.counts
            ));
        }
        rows.push(table_row(suite, cases, counts));

        total.passed += tally.passed;
        total.divergent += tally.divergent;
        total.skipped += tally.skipped;
        total.failing += tally.failing;
        total.unexpected.extend(tally.unexpected);
    }

    println!(
        "\nTOTAL: {} passed, {} expected divergence, {} skipped, {} failing\n",
        total.passed, total.divergent, total.skipped, total.failing
    );
    println!("| Suite | Cases | Passed | Expected divergence | Skipped | Failing | Standing |");
    println!("|---|---:|---:|---:|---:|---:|---|");
    for row in &rows {
        println!("{row}");
    }

    assert!(
        total.unexpected.is_empty(),
        "{} conformance check(s) did not end as listed:\n{}",
        total.unexpected.len(),
        total.unexpected.join("\n")
    );
    // The ratchet, per suite and in both directions. A check can be lost by
    // starting to fail, which the assertion above catches — or by quietly
    // becoming a skip, which nothing would: the report still reads "0 failing"
    // while the suite stops testing anything. Exact counts catch both, and a
    // check that starts passing has to be claimed here too.
    assert!(
        mismatches.is_empty(),
        "the standing of {} suite(s) changed; update SUITES and the README table if that is \
         intended:\n{}",
        mismatches.len(),
        mismatches.join("\n")
    );

    // The README carries the same table, so that it can be read without
    // running anything. It is the report above, row for row.
    let readme = std::fs::read_to_string(conformance_dir().join("README.md")).expect("README");
    let stale: Vec<&String> = rows
        .iter()
        .filter(|row| !readme.contains(row.as_str()))
        .collect();
    assert!(
        stale.is_empty(),
        "tests/conformance/README.md does not carry these rows of the report:\n{}",
        stale
            .iter()
            .map(|row| row.as_str())
            .collect::<Vec<_>>()
            .join("\n")
    );
    let total_line = format!(
        "{} passed, {} expected divergence, {} skipped, {} failing",
        total.passed, total.divergent, total.skipped, total.failing
    );
    assert!(
        readme.contains(&total_line),
        "tests/conformance/README.md does not state the total: {total_line}"
    );
}

/// Actions of the core tier in the current layout. Each case is about one
/// protocol version, and is run only at a version this crate speaks.
const CORE_ACTIONS: [&str; 13] = [
    "validate",
    "resolve_path",
    "data_model",
    "parse_expression_template",
    "process_messages",
    "get_renderer_data_model",
    "get_renderer_capabilities",
    "from_json",
    "to_json",
    "round_trip",
    "validation_schema",
    "derive",
    "validate_common_type",
];

/// Actions on a model's output, which is in one wire format or another in
/// either layout.
const STREAM_ACTIONS: [&str; 4] = ["process_chunk", "parse_full", "fix_payload", "has_parts"];

fn run_case(check: &str, case: &Value, format: Format, tally: &mut Tally) {
    let name = case_name(case);
    let action = case.get("action").and_then(Value::as_str).unwrap_or("");

    // These legacy toolkit-policy cases require implicit fallback or cross-ID
    // merging. The reviewed v0.9.1 contract deliberately rejects those choices;
    // named replacement regressions live in toolkit::negotiate and author.rs.
    if matches!(
        name,
        "test_select_catalog_default"
            | "test_select_catalog_inline"
            | "test_select_catalog_multiple_inline"
            | "test_select_catalog_no_match_with_inline"
    ) {
        tally.record(
            check,
            Outcome::Skipped(
                "superseded toolkit policy: no implicit catalog fallback or cross-ID inline merge"
                    .into(),
            ),
        );
        return;
    }

    // Wire-format actions are gated on the version a case targets. Schema
    // surgery and prompt assembly work on the shape of documents rather than
    // on the protocol, and gate themselves where a version matters.
    // Asked of every version alike, and of none of them here.
    if action == "common_types_schema" {
        tally.record(
            check,
            Outcome::Skipped(
                "not applicable: compares a schema generated from an SDK's own types with the \
                 published one; this crate ships the published document"
                    .to_string(),
            ),
        );
        return;
    }

    let wire_format = match format {
        Format::Current => CORE_ACTIONS.contains(&action) || STREAM_ACTIONS.contains(&action),
        Format::Retired => STREAM_ACTIONS.contains(&action),
    };
    if wire_format {
        if let CaseVersion::Unsupported(reason) = case_version(case) {
            tally.record(check, Outcome::Skipped(reason));
            return;
        }
    }

    match (action, format) {
        ("validate", Format::Retired) => run_validate_case(check, case, tally),
        ("validate", Format::Current) => run_validate_steps(check, case, tally),
        ("process_chunk", _) => tally.record(check, run_process_chunk(name, case)),
        ("parse_full", _) => tally.record(check, run_parse_full(case)),
        ("fix_payload", _) => tally.record(check, run_fix_payload(case)),
        ("has_parts", _) => tally.record(check, run_has_parts(case)),
        ("prune", _) => tally.record(check, run_prune(case)),
        ("render", _) => tally.record(check, run_render(case)),
        ("load", _) => tally.record(check, run_load(case)),
        ("remove_strict_validation", _) => tally.record(check, run_remove_strict(case)),
        ("verify_cuttable_keys", _) => tally.record(check, run_verify_cuttable_keys(case)),
        ("select_catalog", _) => tally.record(check, run_select_catalog(case)),
        ("load_catalog", _) => tally.record(check, run_load_catalog(case)),
        ("generate_prompt", _) => tally.record(check, run_generate_prompt(case)),
        ("resolve_path", Format::Current) => settle(check, run_resolve_path(case), tally),
        (
            "from_json" | "to_json" | "round_trip" | "validation_schema" | "derive",
            Format::Current,
        ) => {
            run_catalog_case(check, action, case, tally);
        }
        ("validate_common_type", Format::Current) => {
            settle(check, run_validate_common_type(case), tally);
        }
        ("process_messages", Format::Current) => {
            settle(check, run_process_messages(case), tally);
        }
        ("get_renderer_data_model", Format::Current) => {
            settle(check, run_renderer_data_model(case), tally);
        }
        ("get_renderer_capabilities", Format::Current) => tally.record(
            check,
            Outcome::Skipped(
                "advertises a renderer's capabilities: this crate reads them \
                 (toolkit::negotiate) and has nothing that writes them"
                    .to_string(),
            ),
        ),
        ("parse_expression_template", Format::Current) => {
            settle(check, run_parse_expression_template(case), tally);
        }
        ("data_model", Format::Current) => match data_model_skip(case) {
            Some(reason) => tally.record(check, Outcome::Skipped(reason.to_string())),
            None => settle(check, run_data_model(case), tally),
        },
        (other, _) => tally.record(check, Outcome::Skipped(unsupported_action(other))),
    }
}

/// Why an action is out of scope for an agent-side, non-rendering crate.
fn unsupported_action(action: &str) -> String {
    let reason = match action {
        "accessibility_check" => "renderer-side accessibility tree; this crate does not render",
        _ => "action not implemented",
    };
    format!("action '{action}': {reason}")
}

/// What a case says about the protocol version it is written for, and whether
/// this crate speaks it.
enum CaseVersion {
    /// A version of the v0.9 family, or none stated.
    Supported,
    /// A version this crate does not speak, with the reason to skip the case.
    Unsupported(String),
}

/// Reads the version a case targets: its own `protocolVersion`, or the one its
/// `catalog` block states under either spelling.
fn case_version(case: &Value) -> CaseVersion {
    let stated = case
        .get("protocolVersion")
        .or_else(|| {
            let catalog = case.get("catalog")?;
            catalog
                .get("protocolVersion")
                .or_else(|| catalog.get("version"))
        })
        .map(render_version);
    let Some(stated) = stated else {
        return CaseVersion::Supported;
    };
    // The suites write a version with and without the `v`: "0.9" and "v0.9".
    let wire = if stated.starts_with('v') {
        stated.clone()
    } else {
        format!("v{stated}")
    };
    if A2uiVersion::parse(&wire).is_some() {
        CaseVersion::Supported
    } else if wire.starts_with("v0.8") {
        CaseVersion::Unsupported(
            "protocol v0.8: this crate targets the v0.9 wire format only".to_string(),
        )
    } else {
        CaseVersion::Unsupported(format!("protocol {stated}: A2UI v1.0 support is #42"))
    }
}

// --- catalog schema operations ---------------------------------------------

/// Builds the three schema documents a case configures.
///
/// Schema surgery is version-independent — it works on the shape of the
/// documents, not on the wire format — so unlike `validate` and `process_chunk`
/// these actions run for v0.8 cases too.
fn schema_bundle(case: &Value) -> Result<SchemaBundle, String> {
    let config = case.get("catalog");
    let load = |key: &str| -> Result<Value, String> {
        Ok(load_catalog_schema(config.and_then(|c| field(c, key)))?.unwrap_or(Value::Null))
    };
    Ok(SchemaBundle {
        s2c: load("s2c_schema")?,
        common_types: load("common_types_schema")?,
        catalog: load("catalog_schema")?,
        custom_cuttable_keys: config
            .and_then(|c| field(c, "custom_cuttable_keys"))
            .and_then(Value::as_array)
            .map(|keys| {
                keys.iter()
                    .filter_map(Value::as_str)
                    .map(str::to_string)
                    .collect()
            }),
    })
}

fn string_list(value: Option<&Value>) -> Vec<String> {
    value
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .filter_map(Value::as_str)
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default()
}

fn run_prune(case: &Value) -> Outcome {
    let bundle = match schema_bundle(case) {
        Ok(bundle) => bundle,
        Err(reason) => return Outcome::Skipped(reason),
    };
    let args = case.get("args");
    let pruned = bundle.prune(
        &string_list(args.and_then(|a| field(a, "allowed_components"))),
        &string_list(args.and_then(|a| field(a, "allowed_messages"))),
    );

    let Some(expect) = case.get("expect").and_then(Value::as_object) else {
        return Outcome::Skipped("case has no 'expect'".to_string());
    };
    for (key, want) in expect {
        let actual = match key.as_str() {
            "catalog_schema" => &pruned.catalog,
            "s2c_schema" => &pruned.s2c,
            "common_types_schema" => &pruned.common_types,
            other => return Outcome::Skipped(format!("unexpected expectation key '{other}'")),
        };
        if actual != want {
            return Outcome::Failed(format!("{key}: expected {want}, got {actual}"));
        }
    }
    Outcome::Passed
}

fn run_render(case: &Value) -> Outcome {
    let bundle = match schema_bundle(case) {
        Ok(bundle) => bundle,
        Err(reason) => return Outcome::Skipped(reason),
    };
    let Some(expected) = field(case, "expect_output").and_then(Value::as_str) else {
        return Outcome::Skipped("case has no 'expect_output'".to_string());
    };
    let actual = bundle.render_llm_instructions();
    if actual.trim() == expected.trim() {
        Outcome::Passed
    } else {
        Outcome::Failed(format!("expected {expected:?}, got {actual:?}"))
    }
}

fn run_load(case: &Value) -> Outcome {
    let args = case.get("args");
    let validate = args
        .and_then(|a| a.get("validate"))
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let path = args.and_then(|a| a.get("path")).and_then(Value::as_str);
    // Upstream validates each example against the catalog schema; this crate
    // only checks that it parses, so schema-driven failures stay out of scope.
    if validate && field(case, "expect_error").is_some() {
        let wants_schema_failure = path.is_some_and(|p| p.contains("schema_error"));
        if wants_schema_failure {
            return Outcome::Skipped(
                "expects JSON Schema validation of an example (no JSON Schema validator here)"
                    .to_string(),
            );
        }
    }

    let resolved = path.map(|p| {
        if p.starts_with('/') {
            PathBuf::from(p)
        } else {
            conformance_dir().join(p)
        }
    });
    let result = load_examples(resolved.as_deref(), validate);

    if let Some(expect_error) = field(case, "expect_error") {
        let wanted = expected_message(expect_error);
        return match result {
            Ok(output) => Outcome::Failed(format!(
                "expected an error matching {wanted:?}, got {output:?}"
            )),
            Err(error) => {
                let text = error.to_string();
                if wanted.is_empty() || text.contains(&wanted) {
                    Outcome::Passed
                } else {
                    Outcome::Failed(format!("expected {wanted:?} in error, got {text:?}"))
                }
            }
        };
    }

    let Some(expected) = field(case, "expect_output").and_then(Value::as_str) else {
        return Outcome::Skipped("case has no 'expect_output'".to_string());
    };
    match result {
        Ok(actual) if actual.trim() == expected.trim() => Outcome::Passed,
        Ok(actual) => Outcome::Failed(format!("expected {expected:?}, got {actual:?}")),
        Err(error) => Outcome::Failed(format!("unexpected error: {error}")),
    }
}

fn run_remove_strict(case: &Value) -> Outcome {
    let Some(mut schema) = case.get("args").and_then(|a| a.get("schema")).cloned() else {
        return Outcome::Skipped("case has no 'args.schema'".to_string());
    };
    let Some(expected) = case.get("expect").and_then(|e| e.get("schema")) else {
        return Outcome::Skipped("case has no 'expect.schema'".to_string());
    };
    remove_strict_validation(&mut schema);
    if &schema == expected {
        Outcome::Passed
    } else {
        Outcome::Failed(format!("expected {expected}, got {schema}"))
    }
}

fn run_verify_cuttable_keys(case: &Value) -> Outcome {
    let bundle = match schema_bundle(case) {
        Ok(bundle) => bundle,
        Err(reason) => return Outcome::Skipped(reason),
    };
    let expected: BTreeSet<String> = string_list(
        case.get("expect")
            .and_then(|e| field(e, "custom_cuttable_keys")),
    )
    .into_iter()
    .collect();
    let actual: BTreeSet<String> = bundle.cuttable_keys().into_iter().collect();
    if actual == expected {
        Outcome::Passed
    } else {
        Outcome::Failed(format!("expected {expected:?}, got {actual:?}"))
    }
}

// --- catalog negotiation and prompt generation -----------------------------

fn client_capabilities(value: Option<&Value>) -> ClientCapabilities {
    let Some(value) = value else {
        return ClientCapabilities::default();
    };
    ClientCapabilities {
        supported_catalog_ids: string_list(value.get("supportedCatalogIds")),
        inline_catalogs: value
            .get("inlineCatalogs")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default(),
        ..ClientCapabilities::default()
    }
}

fn run_select_catalog(case: &Value) -> Outcome {
    let args = case.get("args");
    let supported = args
        .and_then(|a| field(a, "supported_catalogs"))
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let capabilities = client_capabilities(args.and_then(|a| field(a, "client_capabilities")));
    let accepts_inline = args
        .and_then(|a| field(a, "accepts_inline_catalogs"))
        .and_then(Value::as_bool)
        .unwrap_or(false);

    let result = select_catalog_schema(&supported, &capabilities, accepts_inline);

    if let Some(expect_error) = field(case, "expect_error") {
        let wanted = expected_message(expect_error);
        return match result {
            Ok(schema) => Outcome::Failed(format!(
                "expected an error matching {wanted:?}, got {schema}"
            )),
            Err(error) => {
                let text = error.to_string();
                if wanted.is_empty() || text.contains(&wanted) {
                    Outcome::Passed
                } else {
                    Outcome::Failed(format!("expected {wanted:?} in error, got {text:?}"))
                }
            }
        };
    }

    let selected = match result {
        Ok(selected) => selected,
        Err(error) => return Outcome::Failed(format!("unexpected error: {error}")),
    };
    if let Some(expected) = field(case, "expect_selected").and_then(Value::as_str) {
        let actual = selected.get("catalogId").and_then(Value::as_str);
        if actual != Some(expected) {
            return Outcome::Failed(format!("expected catalogId {expected:?}, got {actual:?}"));
        }
    }
    if let Some(expected) = field(case, "expect_catalog_schema") {
        if &selected != expected {
            return Outcome::Failed(format!("expected {expected}, got {selected}"));
        }
    }
    Outcome::Passed
}

fn run_load_catalog(case: &Value) -> Outcome {
    let Some(configs) = field(case, "catalog_configs").and_then(Value::as_array) else {
        return Outcome::Skipped("case has no 'catalog_configs'".to_string());
    };
    let mut registry = CatalogRegistry::new();
    for config in configs {
        let name = config
            .get("name")
            .and_then(Value::as_str)
            .unwrap_or_default();
        let Some(path) = config.get("path").and_then(Value::as_str) else {
            return Outcome::Skipped("catalog config has no path".to_string());
        };
        if let Err(error) = registry.load(name, fixture(path)) {
            return Outcome::Failed(format!("cannot load {path}: {error}"));
        }
    }
    for modifier in string_list(case.get("modifiers")) {
        match modifier.as_str() {
            "remove_strict_validation" => registry.relax_strict_validation(),
            other => return Outcome::Skipped(format!("unknown modifier '{other}'")),
        }
    }

    let Some(expect) = case.get("expect").filter(|expect| expect.is_object()) else {
        return Outcome::Skipped("case has no 'expect'".to_string());
    };
    if let Some(expected) = field(expect, "catalog_schema") {
        let Some(entry) = registry.entries().first() else {
            return Outcome::Failed("registry is empty".to_string());
        };
        if &entry.schema != expected {
            return Outcome::Failed(format!("expected {expected}, got {}", entry.schema));
        }
    }
    if let Some(expected) = field(expect, "supported_catalog_ids") {
        let actual = Value::from(registry.supported_catalog_ids());
        if &actual != expected {
            return Outcome::Failed(format!("expected {expected}, got {actual}"));
        }
    }
    Outcome::Passed
}

fn run_generate_prompt(case: &Value) -> Outcome {
    let args = case.get("args");
    let version = args
        .and_then(|a| a.get("version"))
        .map(render_version)
        .unwrap_or_default();
    let include_schema = args
        .and_then(|a| field(a, "include_schema"))
        .and_then(Value::as_bool)
        .unwrap_or(false);
    // The prompt scaffolding is version-independent, but the schema block
    // embeds the protocol's own schema documents, and this crate ships v0.9.
    if include_schema && !version.starts_with("0.9") && !version.starts_with("v0.9") {
        return Outcome::Skipped(format!(
            "prompt embeds the v{version} schema bundle; this crate targets v0.9"
        ));
    }

    let role = args
        .and_then(|a| field(a, "role_description"))
        .and_then(Value::as_str)
        .unwrap_or("");
    let catalog = Catalog::empty("conformance");
    let mut spec = PromptSpec::new(role, "", &catalog);
    spec.include_response_format = false;

    let workflow = args
        .and_then(|a| field(a, "workflow_description"))
        .and_then(Value::as_str);
    let ui = args
        .and_then(|a| field(a, "ui_description"))
        .and_then(Value::as_str);
    spec.ui_description = ui;

    // Upstream appends a custom workflow description to the standard rules.
    let rules;
    if let Some(workflow) = workflow {
        rules = format!("{}\n{workflow}", GENERATION_GUIDELINES);
        spec.workflow_rules = Some(&rules);
    }

    let bundle;
    if include_schema {
        let mut capabilities =
            client_capabilities(args.and_then(|a| field(a, "client_ui_capabilities")));
        let accepts_inline = args
            .and_then(|a| field(a, "accepts_inline_catalogs"))
            .and_then(Value::as_bool)
            .unwrap_or(false);
        // The suite does not carry the protocol's own schema documents, so the
        // harness supplies the vendored v0.9 ones; the assertions are on the
        // block structure, not on their contents.
        let base = match load_catalog_schema(Some(&Value::String(
            "test_data/catalogs/simplified_catalog_v09.json".to_string(),
        ))) {
            Ok(Some(schema)) => schema,
            _ => return Outcome::Failed("vendored v0.9 catalog is missing".to_string()),
        };
        // Prompt-only vectors supply no renderer metadata. Explicitly configure
        // the fixture's known catalog; do not exercise negotiation fallback.
        if args
            .and_then(|a| field(a, "client_ui_capabilities"))
            .is_none()
        {
            capabilities = ClientCapabilities::supporting([base["catalogId"].as_str().unwrap()]);
        }
        let catalog_schema = match select_catalog_schema(&[base], &capabilities, accepts_inline) {
            Ok(schema) => schema,
            Err(error) => return Outcome::Failed(format!("catalog negotiation failed: {error}")),
        };
        let allowed = string_list(args.and_then(|a| field(a, "allowed_components")));
        bundle = SchemaBundle {
            s2c: load_vendored("test_data/simplified_s2c_v09.json"),
            common_types: load_vendored("test_data/simplified_common_types_v09.json"),
            catalog: catalog_schema,
            custom_cuttable_keys: None,
        }
        .prune(&allowed, &[]);
        spec.schemas = Some(&bundle);
    }

    let examples;
    if args
        .and_then(|a| field(a, "include_examples"))
        .and_then(Value::as_bool)
        .unwrap_or(false)
    {
        let path = args
            .and_then(|a| field(a, "examples_path"))
            .and_then(Value::as_str)
            .map(|p| conformance_dir().join(p));
        examples = match load_examples(path.as_deref(), false) {
            Ok(examples) => examples,
            Err(error) => return Outcome::Failed(format!("cannot load examples: {error}")),
        };
        spec.examples = Some(&examples);
    }

    let prompt = build_subagent_prompt(&spec);
    for wanted in string_list(field(case, "expect_contains")).iter() {
        if !prompt.contains(wanted) {
            return Outcome::Failed(format!("prompt does not contain {wanted:?}"));
        }
    }
    Outcome::Passed
}

// --- resolve_path ----------------------------------------------------------

/// `resolve_path`: the absolute path a binding path resolves to in a scope.
fn run_resolve_path(case: &Value) -> Result<(), Miss> {
    unknown_key(
        case,
        &[&CASE_KEYS[..], &["args", "expect"]].concat(),
        "case",
    )?;
    let args = case.get("args").unwrap_or(&Value::Null);
    unknown_key(args, &["path", "contextPath"], "args")?;
    let path = args.get("path").and_then(Value::as_str).unwrap_or_default();
    // No context path is the root scope.
    let scope = args
        .get("contextPath")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let expected = case.get("expect").cloned().unwrap_or(Value::Null);
    let actual = Value::String(absolute_path(scope, path));
    if actual == expected {
        Ok(())
    } else {
        Err(Miss::new(actual.to_string(), expected.to_string()))
    }
}

// --- data_model ------------------------------------------------------------

/// Why a `data_model` case is not run at all.
fn data_model_skip(case: &Value) -> Option<&'static str> {
    let disposes = case
        .get("steps")
        .and_then(Value::as_array)
        .is_some_and(|steps| {
            steps
                .iter()
                .any(|step| step.get("op").and_then(Value::as_str) == Some("dispose"))
        });
    disposes.then_some(
        "detaches a model's observers: registering observers is a renderer's, and this crate \
         has none to detach",
    )
}

/// What a failed write or read gave, written the way [`DIVERGENCES`] pins it.
fn pointer_outcome(error: &Error) -> String {
    match error {
        // The reason, not the sentence: the sentence quotes the path.
        Error::Pointer { reason, .. } => format!("pointer error: {reason}"),
        other => format!("error: {other}"),
    }
}

/// What a read found, written the way [`DIVERGENCES`] pins it.
fn read_outcome(found: Option<&ModelValue>) -> String {
    match found.map(ModelValue::to_json) {
        None => "no value".to_string(),
        Some(Ok(value)) => value.to_string(),
        Some(Err(error)) => format!("a value with an undefined slot ({error})"),
    }
}

/// The reason this crate gives for the failure a phrase of the reference's
/// `DataError` names. `None` for a phrase with no counterpart here.
fn data_error_reason(phrase: &str) -> Option<&'static str> {
    match phrase {
        "Cannot set path" => Some("path walks through a scalar value"),
        "non-numeric segment" => Some("expected a canonical array index"),
        _ => None,
    }
}

/// Whether `error` is the failure a step's `expect_error` names. The error is
/// what the step wanted instead.
fn is_expected_data_error(error: &Error, expect_error: &Value) -> Result<(), String> {
    let category = expect_error.get("category").and_then(Value::as_str);
    if category != Some("DataError") {
        return Err(format!("a {category:?}, which this runner does not map"));
    }
    let Error::Pointer { reason, .. } = error else {
        return Err("a pointer error".to_string());
    };
    match expect_error.get("message").and_then(Value::as_str) {
        None => Ok(()),
        Some(phrase) => match data_error_reason(phrase) {
            Some(wanted) if wanted == reason => Ok(()),
            Some(wanted) => Err(format!("a pointer error saying {wanted:?}")),
            None => Err(format!(
                "a pointer error for {phrase:?}, which this crate has no reason for"
            )),
        },
    }
}

/// `data_model`: reads, writes and deletes against one [`DataModel`], with the
/// paths in `watch` asked, after each write, whether their value changed.
///
/// A watched path is counted as notified when
/// [`ModelChange::changed`](ag_ui_a2ui::ModelChange::changed) says so, once
/// for each time `watch` lists it. That is the reference's rule for firing an
/// observer, without the observer.
fn run_data_model(case: &Value) -> Result<(), Miss> {
    unknown_key(
        case,
        &[&CASE_KEYS[..], &["initial", "watch", "steps"]].concat(),
        "case",
    )?;
    let initial = case
        .get("initial")
        .cloned()
        .unwrap_or_else(|| serde_json::json!({}));
    let mut model = DataModel::from(initial);
    let watch = string_list(case.get("watch"));
    // The value the last `set` was handed, for the step that changes it after.
    let mut handed = Value::Null;
    let steps = case.get("steps").and_then(Value::as_array);
    for (index, step) in steps.into_iter().flatten().enumerate() {
        data_model_step(&mut model, &watch, &mut handed, step).map_err(|miss| miss.at(index))?;
    }
    Ok(())
}

fn data_model_step(
    model: &mut DataModel,
    watch: &[String],
    handed: &mut Value,
    step: &Value,
) -> Result<(), Miss> {
    unknown_key(
        step,
        &[
            "op",
            "path",
            "value",
            "expect",
            "expect_absent",
            "expect_type",
            "expect_error",
            "expect_notified",
            "expect_values",
        ],
        "step",
    )?;
    let op = step.get("op").and_then(Value::as_str).unwrap_or_default();
    let path = step.get("path").and_then(Value::as_str).unwrap_or_default();
    let expect_error = step.get("expect_error");

    // The watched paths the step changed: none of them, for a read.
    let mut notified: Vec<&str> = Vec::new();
    match op {
        "get" => match (model.lookup(path), expect_error) {
            (Err(error), Some(wanted)) => is_expected_data_error(&error, wanted)
                .map_err(|wanted| Miss::new(pointer_outcome(&error), wanted))?,
            (Err(error), None) => return Err(Miss::new(pointer_outcome(&error), "a read")),
            (Ok(found), Some(_)) => return Err(Miss::new(read_outcome(found), "an error")),
            (Ok(found), None) => {
                let actual = read_outcome(found);
                if let Some(wanted) = step.get("expect") {
                    if found.and_then(|value| value.to_json().ok()).as_ref() != Some(wanted) {
                        return Err(Miss::new(actual, wanted.to_string()));
                    }
                }
                if step.get("expect_absent") == Some(&Value::Bool(true)) && found.is_some() {
                    return Err(Miss::new(actual, "no value"));
                }
                if let Some(kind) = step.get("expect_type").and_then(Value::as_str) {
                    let is_kind = match kind {
                        "list" => matches!(found, Some(ModelValue::Array(_))),
                        "object" => matches!(found, Some(ModelValue::Object(_))),
                        _ => {
                            return Err(Miss::new(
                                format!("the runner does not know the type {kind:?}"),
                                "a list or an object",
                            ));
                        }
                    };
                    if !is_kind {
                        return Err(Miss::new(actual, format!("a {kind}")));
                    }
                }
            }
        },
        "set" | "delete" => {
            let update = if op == "set" {
                *handed = step.get("value").cloned().unwrap_or(Value::Null);
                DataModelUpdate::Set(handed.clone())
            } else {
                DataModelUpdate::Remove
            };
            match (model.apply_reporting(path, &update), expect_error) {
                (Err(error), Some(wanted)) => is_expected_data_error(&error, wanted)
                    .map_err(|wanted| Miss::new(pointer_outcome(&error), wanted))?,
                (Err(error), None) => {
                    return Err(Miss::new(
                        pointer_outcome(&error),
                        "the write to be accepted",
                    ));
                }
                (Ok(_), Some(_)) => return Err(Miss::new("accepted", "an error")),
                (Ok(change), None) => {
                    for watched in watch {
                        let changed = change.changed(watched).map_err(|error| {
                            Miss::new(pointer_outcome(&error), "a watched path")
                        })?;
                        if changed {
                            notified.push(watched);
                        }
                    }
                }
            }
        }
        // The caller changes the value it handed to `set`, afterwards. The
        // model took its own copy, so nothing is written and nothing changes;
        // the reads that follow are what show it.
        "mutate_input" => match handed.pointer_mut(path) {
            Some(slot) => *slot = step.get("value").cloned().unwrap_or(Value::Null),
            None => {
                return Err(Miss::new(
                    format!("the value handed to the last set has nothing at {path:?}"),
                    "a value to change",
                ));
            }
        },
        other => {
            return Err(Miss::new(
                format!("the runner does not know the op {other:?}"),
                "get, set, delete or mutate_input",
            ));
        }
    }

    if let Some(wanted) = step.get("expect_notified") {
        // One entry for each observer, in no stated order.
        let mut wanted = string_list(Some(wanted));
        wanted.sort();
        notified.sort_unstable();
        if notified != wanted {
            return Err(Miss::new(
                format!("notified {notified:?}"),
                format!("notified {wanted:?}"),
            ));
        }
    }
    if let Some(Value::Object(values)) = step.get("expect_values") {
        for (watched, wanted) in values {
            let found = model
                .lookup(watched)
                .map_err(|error| Miss::new(pointer_outcome(&error), "a watched path"))?;
            if found.and_then(|value| value.to_json().ok()).as_ref() != Some(wanted) {
                return Err(Miss::new(
                    format!("{watched} holds {}", read_outcome(found)),
                    format!("{watched} to hold {wanted}"),
                ));
            }
        }
    }
    Ok(())
}

// --- parse_expression_template ----------------------------------------------

/// Whether two JSON values are the same value, with a number compared as a
/// number: the suite writes `100000` where a literal `1e5` is a float.
fn same_json(left: &Value, right: &Value) -> bool {
    match (left, right) {
        (Value::Number(left), Value::Number(right)) => left.as_f64() == right.as_f64(),
        (Value::Array(left), Value::Array(right)) => {
            left.len() == right.len() && left.iter().zip(right).all(|(l, r)| same_json(l, r))
        }
        (Value::Object(left), Value::Object(right)) => {
            left.len() == right.len()
                && left
                    .iter()
                    .all(|(key, l)| right.get(key).is_some_and(|r| same_json(l, r)))
        }
        _ => left == right,
    }
}

/// The parts of a template as the suite compares them: adjacent literal
/// strings joined, and empty ones dropped. A case fixes what a template means,
/// not where an implementation happens to split its literal text.
fn joined_literals(parts: &[Value]) -> Vec<Value> {
    let mut out: Vec<Value> = Vec::new();
    for part in parts {
        match (out.last_mut(), part) {
            (_, Value::String(text)) if text.is_empty() => {}
            (Some(Value::String(last)), Value::String(text)) => last.push_str(text),
            _ => out.push(part.clone()),
        }
    }
    out
}

/// What this crate's reason for a template that does not parse starts or ends
/// with, for a phrase of the reference parser's message.
fn template_fault(phrase: &str) -> Option<fn(&str) -> bool> {
    Some(match phrase {
        "Unexpected characters" => |reason| reason.starts_with("unexpected characters after"),
        "Unclosed interpolation" => |reason| reason.starts_with("unterminated interpolation"),
        "after function arguments" => {
            |reason| reason.starts_with("expected ')' after the arguments")
        }
        "after argument name" => {
            |reason| reason.starts_with("expected ':' after the argument name")
        }
        "Invalid number literal" => |reason| reason.starts_with("invalid number literal"),
        "out of range" => |reason| reason.ends_with("is out of range"),
        "Max recursion depth" => |reason| reason.starts_with("expressions nest deeper"),
        _ => return None,
    })
}

/// `parse_expression_template`: a `formatString` template into its parts.
fn run_parse_expression_template(case: &Value) -> Result<(), Miss> {
    unknown_key(
        case,
        &[&CASE_KEYS[..], &["input", "expect", "expect_error"]].concat(),
        "case",
    )?;
    let input = case
        .get("input")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let parsed = parse_template(input).map(|parts| {
        let parts: Vec<Value> = parts.iter().map(|part| part.to_json()).collect();
        Value::Array(joined_literals(&parts))
    });

    let Some(expect_error) = case.get("expect_error") else {
        let wanted = case.get("expect").and_then(Value::as_array);
        let wanted = Value::Array(joined_literals(wanted.map_or(&[], Vec::as_slice)));
        return match parsed {
            Ok(parts) if same_json(&parts, &wanted) => Ok(()),
            Ok(parts) => Err(Miss::new(parts.to_string(), wanted.to_string())),
            Err(error) => Err(Miss::new(format!("error: {error}"), wanted.to_string())),
        };
    };
    unknown_key(expect_error, &["category", "message"], "expect_error")?;
    // The reference raises one error class for a template that does not
    // parse, and the suite names it by either category.
    let category = expect_error.get("category").and_then(Value::as_str);
    if !matches!(category, Some("ParseError" | "ExpressionError")) {
        return Err(Miss::new(
            format!("the runner does not map the category {category:?}"),
            "ParseError",
        ));
    }
    let phrase = expect_error
        .get("message")
        .and_then(Value::as_str)
        .unwrap_or_default();
    match parsed {
        Ok(parts) => Err(Miss::new(parts.to_string(), format!("an error: {phrase}"))),
        Err(Error::Binding { reason, .. }) => match template_fault(phrase) {
            _ if phrase.is_empty() => Ok(()),
            Some(is_fault) if is_fault(&reason) => Ok(()),
            Some(_) => Err(Miss::new(
                format!("binding error: {reason}"),
                format!("the error for {phrase:?}"),
            )),
            None => Err(Miss::new(
                format!("binding error: {reason}"),
                format!("an error for {phrase:?}, a phrase this runner does not map"),
            )),
        },
        Err(other) => Err(Miss::new(format!("error: {other}"), "a binding error")),
    }
}

// --- process_messages -------------------------------------------------------

/// The messages of a case or a step: a list, or a list wrapped as
/// `{"messages": [...]}`.
fn message_list(value: Option<&Value>) -> Vec<Value> {
    match value {
        Some(Value::Array(items)) => items.clone(),
        Some(Value::Object(wrapper)) => match wrapper.get("messages") {
            Some(Value::Array(items)) => items.clone(),
            _ => vec![Value::Object(wrapper.clone())],
        },
        Some(other) => vec![other.clone()],
        None => Vec::new(),
    }
}

/// How a `process_messages` case without `strictMode` is held: the reference
/// processor then checks the envelope and the surface's life, and neither the
/// catalog's schema nor the shape of the tree. Its fixtures rely on that: a
/// `Button` with only a `label`, a component type no catalog defines.
fn lenient_options() -> ValidateOptions {
    ValidateOptions {
        require_root: false,
        allow_dangling_children: true,
        check_component_types: false,
        check_required_props: false,
        check_prop_types: false,
        check_bindings: false,
        ..ValidateOptions::full_surface()
    }
}

/// The processor a case describes: its catalogs registered, and held strictly
/// when the case says `strictMode: true`.
///
/// A catalog named by `catalogPaths` is a published one, and the suite's
/// messages create their surfaces under ids of their own (`test-catalog`). As
/// the reference harnesses do, the published catalog is registered under each
/// id the messages name, except in a case that expects a `CatalogError`, where
/// an unregistered id is the point.
fn case_processor(case: &Value) -> Result<MessageProcessor, Miss> {
    let setup = |what: String| Miss::new(what, "a processor for the case");
    let options = if case.get("strictMode") == Some(&Value::Bool(true)) {
        conformance_options()
    } else {
        lenient_options()
    };
    let mut processor = MessageProcessor::with_options(options);
    for document in case
        .get("catalogs")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
    {
        let catalog = Catalog::from_schema(document)
            .map_err(|error| setup(format!("an inline catalog was rejected: {error}")))?;
        processor
            .register(catalog)
            .map_err(|error| setup(error.to_string()))?;
    }
    let mut published = Vec::new();
    for path in string_list(case.get("catalogPaths")) {
        let text = std::fs::read_to_string(fixture(&path))
            .map_err(|_| setup(format!("{path} is not vendored under tests/conformance/")))?;
        let document: Value = serde_json::from_str(&text)
            .map_err(|error| setup(format!("{path} is not JSON: {error}")))?;
        let catalog = Catalog::from_schema(&document)
            .map_err(|error| setup(format!("{path} was rejected: {error}")))?;
        processor
            .register(catalog.clone())
            .map_err(|error| setup(error.to_string()))?;
        published.push(catalog);
    }
    let expects_catalog_error = case
        .pointer("/expectError/category")
        .and_then(Value::as_str)
        == Some("CatalogError");
    if let (Some(catalog), false) = (published.first(), expects_catalog_error) {
        let steps = case.get("steps").and_then(Value::as_array);
        let batches = std::iter::once(case.get("messages"))
            .chain(steps.into_iter().flatten().map(|step| step.get("messages")));
        for message in batches.flat_map(message_list) {
            let named = message
                .pointer("/createSurface/catalogId")
                .and_then(Value::as_str);
            if let Some(id) = named.filter(|id| processor.catalog(id).is_none()) {
                let mut alias = catalog.clone();
                alias.catalog_id = id.to_string();
                processor
                    .register(alias)
                    .map_err(|error| setup(error.to_string()))?;
            }
        }
    }
    Ok(processor)
}

/// What a refusal was, written the way [`DIVERGENCES`] pins it.
fn refusal_outcome(rejection: &Rejection) -> String {
    let detail = match rejection.errors().first() {
        Some(error) => format!("{} at {}", error.code, error.path),
        None => rejection.error.to_string(),
    };
    format!("refused as {}: {detail}", rejection.kind)
}

/// What an accepted batch left that a case could have wanted refused: the
/// components no root reaches, which this crate reports and does not refuse.
fn accepted_outcome(processor: &MessageProcessor) -> String {
    let unreachable: Vec<String> = processor
        .surfaces()
        .surfaces()
        .filter_map(|surface| {
            let report = processor.report(&surface.surface_id)?;
            (!report.unreachable.is_empty())
                .then(|| format!("{:?} on {}", report.unreachable, surface.surface_id))
        })
        .collect();
    if unreachable.is_empty() {
        "accepted".to_string()
    } else {
        format!("accepted, with unreachable {}", unreachable.join(", "))
    }
}

/// Whether a refusal is the one a phrase of the reference processor's message
/// names. `None` for a phrase this runner has no reading of.
fn refusal_says(phrase: &str, rejection: &Rejection) -> Option<bool> {
    let errors = rejection.errors();
    let any = |code: ErrorCode| errors.iter().any(|error| error.code == code);
    let first_says = |needle: &str| errors.first().is_some_and(|e| e.message.contains(needle));
    let integrity = rejection.kind == RejectionKind::Integrity;
    Some(
        if phrase.starts_with("Surface ") && phrase.ends_with(" already exists") {
            integrity && first_says("already exists")
        } else if phrase.starts_with("Surface not found for message: ") {
            integrity && (first_says("does not exist") || first_says("was deleted"))
        } else if let Some(id) = phrase.strip_prefix("Catalog not found: ") {
            rejection.kind == RejectionKind::Catalog
                && rejection.error.to_string().contains(&format!("{id:?}"))
        } else if phrase == "catalogId" {
            errors
                .iter()
                .any(|e| e.code == ErrorCode::MissingField && e.path.ends_with(".catalogId"))
        } else if phrase == "missing an 'id'" {
            any(ErrorCode::MissingId)
        } else if phrase.starts_with("Cannot create component ")
            && phrase.ends_with("without a type")
        {
            any(ErrorCode::MissingComponentType)
        } else if phrase == "Missing root component" {
            any(ErrorCode::NoRoot)
        } else if phrase == "Circular reference detected" {
            any(ErrorCode::ChildCycle)
        } else if phrase == "Dangling reference" {
            any(ErrorCode::UnresolvedChild)
        } else if let Some(kind) = phrase.strip_prefix("Validation failed for component ") {
            // `'Chart'`: an error about a property of a component of that type.
            errors.iter().any(|e| {
                matches!(
                    e.code,
                    ErrorCode::TypeMismatch
                        | ErrorCode::MissingRequiredProp
                        | ErrorCode::InvalidValue
                ) && e.message.starts_with(kind)
            })
        } else if phrase == "Message contains multiple conflicting update actions" {
            errors.iter().any(|e| {
                e.code == ErrorCode::InvalidValue
                    && e.message.starts_with("Exactly one A2UI payload key")
            })
        } else if phrase.starts_with("action '") && phrase.contains("is not supported in protocol")
        {
            errors.iter().any(|e| {
                e.code == ErrorCode::MissingField
                    && e.message.starts_with("A message must carry one")
            })
        } else if phrase == "surfaceId must be a string" {
            errors
                .iter()
                .any(|e| e.code == ErrorCode::TypeMismatch && e.path.ends_with(".surfaceId"))
        } else {
            return None;
        },
    )
}

/// Holds a refusal to a case's `expectError`: its category, its surface, the
/// fault its message names, and the `error` message a renderer would send.
fn check_refusal(rejection: &Rejection, wanted: &Value) -> Result<(), Miss> {
    unknown_key(
        wanted,
        &["category", "surfaceId", "message", "rendererError"],
        "expectError",
    )?;
    let actual = refusal_outcome(rejection);
    if let Some(category) = wanted.get("category").and_then(Value::as_str) {
        if !rejection.kind.is(category) {
            return Err(Miss::new(actual, format!("a {category}")));
        }
    }
    if let Some(surface) = wanted.get("surfaceId").and_then(Value::as_str) {
        if rejection.surface_id.as_deref() != Some(surface) {
            return Err(Miss::new(
                format!("{actual}, on surface {:?}", rejection.surface_id),
                format!("a refusal on surface {surface:?}"),
            ));
        }
    }
    if let Some(phrase) = wanted.get("message").and_then(Value::as_str) {
        match refusal_says(phrase, rejection) {
            Some(true) => {}
            Some(false) => return Err(Miss::new(actual, format!("the refusal for {phrase:?}"))),
            None => {
                return Err(Miss::new(
                    actual,
                    format!("a refusal for {phrase:?}, a phrase this runner does not map"),
                ));
            }
        }
    }
    if let Some(reply) = wanted.get("rendererError") {
        unknown_key(reply, &["version", "error"], "rendererError")?;
        let error = reply.get("error").unwrap_or(&Value::Null);
        unknown_key(
            error,
            &["code", "surfaceId", "message"],
            "rendererError.error",
        )?;
        let sent = serde_json::to_value(rejection.renderer_error())
            .map_err(|error| Miss::new(error.to_string(), "an error message"))?;
        for (pointer, wanted) in [
            ("/version", reply.get("version")),
            ("/error/code", error.get("code")),
            ("/error/surfaceId", error.get("surfaceId")),
        ] {
            if wanted.is_some() && sent.pointer(pointer) != wanted {
                return Err(Miss::new(
                    format!("a renderer error {sent}"),
                    format!("a renderer error {reply}"),
                ));
            }
        }
        // The sentence is this crate's own. The fault it is about is the one
        // `message` names, which was held above.
        let says_something = sent
            .pointer("/error/message")
            .and_then(Value::as_str)
            .is_some_and(|text| !text.is_empty());
        if error.get("message") != wanted.get("message") || !says_something {
            return Err(Miss::new(
                format!("a renderer error {sent}"),
                format!("a renderer error {reply}"),
            ));
        }
    }
    Ok(())
}

/// Holds the state a processor is in to a case's `expect.surfaces`.
fn check_surfaces(processor: &MessageProcessor, expect: Option<&Value>) -> Result<(), Miss> {
    let Some(expect) = expect else {
        return Ok(());
    };
    unknown_key(expect, &["surfaces"], "expect")?;
    let surfaces = expect.get("surfaces").and_then(Value::as_object);
    for (id, wanted) in surfaces.into_iter().flatten() {
        unknown_key(
            wanted,
            &[
                "exists",
                "sendDataModel",
                "theme",
                "components",
                "dataModel",
            ],
            "expect.surfaces",
        )?;
        let store = processor.surfaces();
        let live = store.lifecycle(id) == Lifecycle::Live;
        if wanted.get("exists") == Some(&Value::Bool(false)) {
            if live {
                return Err(Miss::new(
                    format!("surface {id} is live"),
                    format!("no surface {id}"),
                ));
            }
            continue;
        }
        let Some(surface) = store.get(id).filter(|_| live) else {
            return Err(Miss::new(
                format!("no surface {id}"),
                format!("surface {id}"),
            ));
        };
        if let Some(theme) = wanted.get("theme") {
            if surface.theme.as_ref() != Some(theme) {
                return Err(Miss::new(
                    format!("{id} has theme {:?}", surface.theme),
                    format!("{id} to have theme {theme}"),
                ));
            }
        }
        if let Some(send) = wanted.get("sendDataModel").and_then(Value::as_bool) {
            // Not asked for is not sent.
            if surface.send_data_model.unwrap_or(false) != send {
                return Err(Miss::new(
                    format!("{id} has sendDataModel {:?}", surface.send_data_model),
                    format!("{id} to have sendDataModel {send}"),
                ));
            }
        }
        let data = surface.data_model.to_json().unwrap_or(Value::Null);
        if let Some(model) = wanted.get("dataModel") {
            // The whole model, so that a key that should be gone is seen.
            if !same_json(&data, model) {
                return Err(Miss::new(
                    format!("{id} holds {data}"),
                    format!("{id} to hold {model}"),
                ));
            }
        }
        let Some(components) = wanted.get("components") else {
            continue;
        };
        // A list is every component of the surface; a map is some of them.
        let listed: Vec<(String, &Value)> = match components {
            Value::Array(items) => {
                if items.len() != surface.components.len() {
                    return Err(Miss::new(
                        format!("{id} has {} component(s)", surface.components.len()),
                        format!("{id} to have {}", items.len()),
                    ));
                }
                items
                    .iter()
                    .map(|item| (item["id"].as_str().unwrap_or_default().to_string(), item))
                    .collect()
            }
            Value::Object(items) => items.iter().map(|(id, item)| (id.clone(), item)).collect(),
            _ => Vec::new(),
        };
        let scope = Scope::root(&data);
        for (component_id, wanted) in listed {
            let Some(component) = surface.components.iter().find(|c| c.id == component_id) else {
                return Err(Miss::new(
                    format!("{id} has no component {component_id}"),
                    format!("component {component_id} on {id}"),
                ));
            };
            for (key, wanted) in wanted.as_object().into_iter().flatten() {
                let actual = match key.as_str() {
                    "id" => Some(Value::String(component.id.clone())),
                    "component" => Some(Value::String(component.component.clone())),
                    // A property as a renderer would show it: bound against
                    // the surface's data model, at the root scope.
                    _ => component
                        .prop(key)
                        .map(|raw| scope.resolve_dynamic(raw).unwrap_or_else(|_| raw.clone())),
                };
                if !actual
                    .as_ref()
                    .is_some_and(|actual| same_json(actual, wanted))
                {
                    return Err(Miss::new(
                        format!(
                            "{component_id}.{key} is {}",
                            actual.map_or_else(|| "absent".to_string(), |value| value.to_string())
                        ),
                        format!("{component_id}.{key} to be {wanted}"),
                    ));
                }
            }
        }
    }
    Ok(())
}

/// One batch of a `process_messages` case: taken in as a transaction, and held
/// to what the case expects of it.
fn process_step(
    processor: &mut MessageProcessor,
    messages: Option<&Value>,
    expect: Option<&Value>,
    expect_error: Option<&Value>,
) -> Result<(), Miss> {
    match (processor.process_all(&message_list(messages)), expect_error) {
        (Ok(()), None) => check_surfaces(processor, expect),
        (Ok(()), Some(wanted)) => Err(Miss::new(
            accepted_outcome(processor),
            format!("a refusal: {wanted}"),
        )),
        (Err(rejection), None) => Err(Miss::new(
            refusal_outcome(&rejection),
            "the messages to be accepted",
        )),
        (Err(rejection), Some(wanted)) => check_refusal(&rejection, wanted),
    }
}

/// `process_messages`: one [`MessageProcessor`] for the case, and the case's
/// messages, or each step's, taken in as a batch.
fn run_process_messages(case: &Value) -> Result<(), Miss> {
    unknown_key(
        case,
        &[
            &CASE_KEYS[..],
            &[
                "catalogPaths",
                "catalogs",
                "strictMode",
                "messages",
                "steps",
                "expect",
                "expectError",
            ],
        ]
        .concat(),
        "case",
    )?;
    let mut processor = case_processor(case)?;
    let Some(steps) = case.get("steps").and_then(Value::as_array) else {
        return process_step(
            &mut processor,
            case.get("messages"),
            case.get("expect"),
            case.get("expectError"),
        );
    };
    for (index, step) in steps.iter().enumerate() {
        unknown_key(step, &["messages", "expect", "expectError"], "step")
            .and_then(|()| {
                process_step(
                    &mut processor,
                    step.get("messages"),
                    step.get("expect"),
                    step.get("expectError"),
                )
            })
            .map_err(|miss| miss.at(index))?;
    }
    Ok(())
}

/// `get_renderer_data_model`: the `a2uiClientDataModel` metadata a renderer
/// holding the surfaces the messages leave would attach.
fn run_renderer_data_model(case: &Value) -> Result<(), Miss> {
    unknown_key(
        case,
        &[
            &CASE_KEYS[..],
            &["catalogPaths", "catalogs", "messages", "args", "expect"],
        ]
        .concat(),
        "case",
    )?;
    let args = case.get("args").unwrap_or(&Value::Null);
    unknown_key(args, &["version"], "args")?;
    let mut processor = case_processor(case)?;
    processor
        .process_all(&message_list(case.get("messages")))
        .map_err(|rejection| {
            Miss::new(refusal_outcome(&rejection), "the messages to be accepted")
        })?;
    let version = args
        .get("version")
        .and_then(Value::as_str)
        .and_then(A2uiVersion::parse)
        .ok_or_else(|| Miss::new("args.version is not a version this crate speaks", "one"))?;
    let wanted = case.get("expect").cloned().unwrap_or(Value::Null);
    let actual = processor
        .surfaces()
        .client_data_model(version)
        .and_then(|metadata| Ok(serde_json::to_value(metadata)?))
        .map_err(|error| Miss::new(format!("error: {error}"), wanted.to_string()))?;
    if same_json(&actual, &wanted) {
        Ok(())
    } else {
        Err(Miss::new(actual.to_string(), wanted.to_string()))
    }
}

// --- catalog: from_json, to_json, round_trip, validation_schema, derive -----

/// A document a core case names by repository path.
///
/// The tree mirrors upstream's repository root, with `conformance/` itself at
/// the top of it.
fn repository_document(path: &str) -> Result<Value, String> {
    let relative = path.strip_prefix("conformance/").unwrap_or(path);
    let text = std::fs::read_to_string(fixture(relative))
        .map_err(|_| format!("{path} is not vendored under tests/conformance/"))?;
    serde_json::from_str(&text).map_err(|error| format!("{path} is not JSON: {error}"))
}

/// The catalog document a case is about: inline, or named by path. `None` for
/// a case about the SDK's own catalog.
fn catalog_document(case: &Value) -> Result<Option<Value>, String> {
    match (
        case.get("catalog"),
        case.get("catalogPath").and_then(Value::as_str),
    ) {
        (Some(inline), _) => Ok(Some(inline.clone())),
        (None, Some(path)) => repository_document(path).map(Some),
        (None, None) => Ok(None),
    }
}

/// One case of `core/catalog.yaml`. The version is the case's, or the one its
/// document states: a case about a v1.0 catalog often says so only there.
fn run_catalog_case(check: &str, action: &str, case: &Value, tally: &mut Tally) {
    let document = match catalog_document(case) {
        Ok(document) => document,
        Err(reason) => {
            tally.record(check, Outcome::Failed(reason));
            return;
        }
    };
    let stated = document
        .as_ref()
        .and_then(|document| document.get("protocolVersion"))
        .map(|version| serde_json::json!({ "protocolVersion": version }));
    if let Some(CaseVersion::Unsupported(reason)) = stated.as_ref().map(case_version) {
        tally.record(check, Outcome::Skipped(reason));
        return;
    }
    match action {
        "from_json" => settle(check, run_from_json(case, document), tally),
        "round_trip" => settle(check, run_round_trip(case, document), tally),
        "to_json" => settle(check, run_to_json(case), tally),
        "validation_schema" => tally.record(
            check,
            Outcome::Skipped(
                "validationSchema is the reference's bundled, flattened schema: this crate \
                 validates against the documents as published and bundles nothing"
                    .to_string(),
            ),
        ),
        _ => tally.record(
            check,
            Outcome::Skipped(
                "Catalog has no derive; 7 of the 8 derive cases are on a v1.0 catalog (#42)"
                    .to_string(),
            ),
        ),
    }
}

/// The version a catalog case runs under: the one it or its document states,
/// or v0.9, which is the reference's reading of a document that states none.
fn catalog_version(case: &Value, document: Option<&Value>) -> String {
    case.get("protocolVersion")
        .or_else(|| document.and_then(|document| document.get("protocolVersion")))
        .map_or_else(|| A2uiVersion::DEFAULT.as_str().to_string(), render_version)
}

/// A validator for messages on a surface of this catalog, built from the
/// documents the crate ships and this one, and from nothing else.
fn catalog_validator(document: &Value) -> Result<SchemaValidator, String> {
    let mut bundle = SchemaBundle::basic().map_err(|error| error.to_string())?;
    bundle.catalog = document.clone();
    SchemaValidator::new(&bundle, &BTreeMap::new()).map_err(|error| error.to_string())
}

/// What makes a catalog read from the published document differ from the one
/// this crate builds in: its id, its component and function names, each
/// component's properties and required properties, each function's return
/// type.
fn differences_from_builtin(loaded: &Catalog) -> Vec<String> {
    let builtin = Catalog::basic();
    let mut out = Vec::new();
    if loaded.catalog_id != builtin.catalog_id {
        out.push(format!(
            "id {:?}, built in as {:?}",
            loaded.catalog_id, builtin.catalog_id
        ));
    }
    let names = |catalog: &Catalog| catalog.components.keys().cloned().collect::<Vec<_>>();
    if names(loaded) != names(&builtin) {
        out.push(format!("components {:?}", names(loaded)));
    }
    for (name, published) in &loaded.components {
        let Some(own) = builtin.component(name) else {
            continue;
        };
        let props = |def: &ag_ui_a2ui::catalog::ComponentDef| {
            def.props.keys().cloned().collect::<BTreeSet<_>>()
        };
        let (published_props, own_props) = (props(published), props(own));
        if published_props != own_props {
            out.push(format!(
                "{name} properties: published only {:?}, built in only {:?}",
                published_props.difference(&own_props).collect::<Vec<_>>(),
                own_props.difference(&published_props).collect::<Vec<_>>()
            ));
        }
        let required = |def: &ag_ui_a2ui::catalog::ComponentDef| {
            def.required.iter().cloned().collect::<BTreeSet<_>>()
        };
        if required(published) != required(own) {
            out.push(format!(
                "{name} requires {:?}, built in {:?}",
                required(published),
                required(own)
            ));
        }
    }
    let functions = |catalog: &Catalog| {
        catalog
            .functions
            .iter()
            .map(|(name, def)| (name.clone(), def.return_type.clone()))
            .collect::<Vec<_>>()
    };
    if functions(loaded) != functions(&builtin) {
        out.push(format!("functions {:?}", functions(loaded)));
    }
    out
}

/// The entries a case lists against the ones a catalog defines: by name when
/// the case gives a list, and by name and schema when it gives a map. Either
/// way the names are all of them.
fn check_entries(
    kind: &str,
    wanted: &Value,
    defined: Vec<&String>,
    document: &Value,
) -> Result<(), Miss> {
    let names: Vec<&str> = match wanted {
        Value::Array(names) => names.iter().filter_map(Value::as_str).collect(),
        Value::Object(entries) => entries.keys().map(String::as_str).collect(),
        _ => Vec::new(),
    };
    let listed: BTreeSet<&str> = names.into_iter().collect();
    let defined: BTreeSet<&str> = defined.into_iter().map(String::as_str).collect();
    if listed != defined {
        return Err(Miss::new(
            format!("{kind} {defined:?}"),
            format!("{kind} {listed:?}"),
        ));
    }
    for (name, schema) in wanted.as_object().into_iter().flatten() {
        let held = document.get(kind).and_then(|entries| entries.get(name));
        if !held.is_some_and(|held| same_json(held, schema)) {
            return Err(Miss::new(
                format!("{kind}.{name} is {}", held.unwrap_or(&Value::Null)),
                format!("{kind}.{name} to be {schema}"),
            ));
        }
    }
    Ok(())
}

/// The fault a phrase of the reference's `CatalogError` names, as this crate's
/// reason for it starts.
fn catalog_fault(phrase: &str) -> Option<&'static str> {
    match phrase {
        "catalog_id must be provided or exist in catalog schema" => {
            Some("catalog document is missing 'catalogId'")
        }
        _ => None,
    }
}

/// `from_json`: [`Catalog::from_schema`], and what the case expects of the
/// catalog it gives.
fn run_from_json(case: &Value, document: Option<Value>) -> Result<(), Miss> {
    unknown_key(
        case,
        &[
            &CASE_KEYS[..],
            &["catalogId", "catalogPath", "expect", "expectError"],
        ]
        .concat(),
        "case",
    )?;
    let document = document.ok_or_else(|| Miss::new("the case names no catalog", "one"))?;
    let loaded = Catalog::from_schema(&document);

    if let Some(wanted) = case.get("expectError") {
        unknown_key(wanted, &["category", "message"], "expectError")?;
        let phrase = wanted
            .get("message")
            .and_then(Value::as_str)
            .unwrap_or_default();
        let fault = catalog_fault(phrase);
        return match loaded {
            Ok(_) => Err(Miss::new("loaded", format!("a refusal: {wanted}"))),
            Err(Error::Catalog(reason))
                if wanted.get("category").and_then(Value::as_str) == Some("CatalogError")
                    && (phrase.is_empty()
                        || fault.is_some_and(|fault| reason.starts_with(fault))) =>
            {
                Ok(())
            }
            Err(error) => Err(Miss::new(format!("error: {error}"), wanted.to_string())),
        };
    }
    let mut catalog =
        loaded.map_err(|error| Miss::new(format!("error: {error}"), "the catalog to load"))?;
    // The case's own `catalogId` is the id to load the catalog under.
    if let Some(id) = case.get("catalogId").and_then(Value::as_str) {
        catalog.catalog_id = id.to_string();
    }
    let expect = case.get("expect").unwrap_or(&Value::Null);
    unknown_key(
        expect,
        &[
            "catalogId",
            "protocolVersion",
            "components",
            "functions",
            "matchesBuiltinCatalog",
            "selfContained",
            "validComponents",
            "invalidComponents",
        ],
        "expect",
    )?;
    if let Some(id) = expect.get("catalogId").and_then(Value::as_str) {
        if catalog.catalog_id != id {
            return Err(Miss::new(
                format!("catalogId {:?}", catalog.catalog_id),
                format!("catalogId {id:?}"),
            ));
        }
    }
    // A catalog has no version of its own here. The expectation is held to
    // the version the case runs under.
    let version = catalog_version(case, Some(&document));
    if let Some(wanted) = expect.get("protocolVersion").map(render_version) {
        if wanted != version {
            return Err(Miss::new(
                format!("version {version}"),
                format!("version {wanted}"),
            ));
        }
    }
    if let Some(wanted) = expect.get("components") {
        check_entries(
            "components",
            wanted,
            catalog.components.keys().collect(),
            &document,
        )?;
    }
    if let Some(wanted) = expect.get("functions") {
        check_entries(
            "functions",
            wanted,
            catalog.functions.keys().collect(),
            &document,
        )?;
    }
    if expect.get("matchesBuiltinCatalog") == Some(&Value::Bool(true)) {
        let differences = differences_from_builtin(&catalog);
        if !differences.is_empty() {
            return Err(Miss::new(
                format!("differs from Catalog::basic(): {}", differences.join("; ")),
                "the catalog this crate builds in",
            ));
        }
    }

    let samples = |key: &str| {
        expect
            .get(key)
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default()
    };
    let (valid, invalid) = (samples("validComponents"), samples("invalidComponents"));
    let self_contained = expect.get("selfContained") == Some(&Value::Bool(true));
    if valid.is_empty() && invalid.is_empty() && !self_contained {
        return Ok(());
    }
    let validator = catalog_validator(&document)
        .map_err(|error| Miss::new(format!("no validator compiles: {error}"), "one"))?;
    for (component, wanted_valid) in valid
        .iter()
        .map(|component| (component, true))
        .chain(invalid.iter().map(|component| (component, false)))
    {
        let message = serde_json::json!({
            "version": version,
            "updateComponents": {"surfaceId": "s", "components": [component]},
        });
        let verdict = validator.validate_message(&message);
        if verdict.is_ok() != wanted_valid {
            return Err(Miss::new(
                match verdict {
                    Ok(()) => format!("{component} is accepted"),
                    Err(error) => format!("{component} is refused: {error}"),
                },
                if wanted_valid {
                    "it accepted"
                } else {
                    "it refused"
                },
            ));
        }
    }
    if self_contained {
        // Every other expectation of the case held. This one is about a
        // schema this crate does not make.
        return Err(Miss::new(
            NOT_BUNDLED,
            "a resolved catalog schema whose every $ref is inside it",
        ));
    }
    Ok(())
}

/// What this crate does where a case wants a self-contained resolved schema.
const NOT_BUNDLED: &str = "no bundled schema; a validator compiles from this document and the \
                           ones the crate ships, with retrieval off";

/// `round_trip`: a catalog read from a document gives that document back, and
/// reading what it gave back changes nothing.
fn run_round_trip(case: &Value, document: Option<Value>) -> Result<(), Miss> {
    unknown_key(
        case,
        &[&CASE_KEYS[..], &["catalogId", "catalogPath"]].concat(),
        "case",
    )?;
    let document = document.ok_or_else(|| Miss::new("the case names no catalog", "one"))?;
    let mut read = document.clone();
    for pass in ["first", "second"] {
        let catalog = Catalog::from_schema(&read)
            .map_err(|error| Miss::new(format!("error: {error}"), "the catalog to load"))?;
        let Some(written) = catalog.document() else {
            return Err(Miss::new("no document", "the document back"));
        };
        if *written != document {
            return Err(Miss::new(
                format!("another document on the {pass} pass"),
                "the document it was read from",
            ));
        }
        read = written.clone();
    }
    Ok(())
}

/// `to_json`: the document of the catalog an SDK builds in, against the
/// published one.
fn run_to_json(case: &Value) -> Result<(), Miss> {
    unknown_key(
        case,
        &[&CASE_KEYS[..], &["builtinCatalog", "expectFile"]].concat(),
        "case",
    )?;
    let builtin = case.get("builtinCatalog").and_then(Value::as_str);
    if builtin != Some("basic") {
        return Err(Miss::new(
            format!("the runner does not know the built-in catalog {builtin:?}"),
            "basic",
        ));
    }
    let path = case
        .get("expectFile")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let published = repository_document(path).map_err(|reason| Miss::new(reason, "a file"))?;
    match Catalog::basic().document() {
        Some(document) if *document == published => Ok(()),
        Some(_) => Err(Miss::new("another document", path)),
        None => Err(Miss::new("Catalog::basic() has no document", path)),
    }
}

// --- validate_common_type ---------------------------------------------------

/// `validate_common_type`: each step's value against one definition of the
/// published common types.
fn run_validate_common_type(case: &Value) -> Result<(), Miss> {
    unknown_key(
        case,
        &[&CASE_KEYS[..], &["definition", "steps"]].concat(),
        "case",
    )?;
    let definition = case
        .get("definition")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let steps = case.get("steps").and_then(Value::as_array);
    for (index, step) in steps.into_iter().flatten().enumerate() {
        let held = || -> Result<(), Miss> {
            unknown_key(step, &["value", "expectError"], "step")?;
            let value = step.get("value").cloned().unwrap_or(Value::Null);
            let verdict = validate_common_type(definition, &value);
            let Some(wanted) = step.get("expectError") else {
                // An accepted value is the value: nothing is rewritten on the
                // way through a schema, so there is no serialized form to
                // compare it with.
                return verdict.map_err(|error| {
                    Miss::new(format!("{value} is refused: {error}"), "it accepted")
                });
            };
            unknown_key(wanted, &["category"], "expectError")?;
            let category = wanted.get("category").and_then(Value::as_str);
            match verdict {
                Err(Error::Validation { .. }) if category == Some("ValidationError") => Ok(()),
                Err(error) => Err(Miss::new(
                    format!("{value} is refused: {error}"),
                    format!("a {category:?}"),
                )),
                Ok(()) => Err(Miss::new(
                    format!("{value} is accepted"),
                    format!("a {category:?}"),
                )),
            }
        };
        held().map_err(|miss| miss.at(index))?;
    }
    Ok(())
}

/// The reason three `common_types_schema` cases are skipped is that this
/// crate ships the published common types and generates none. This is that
/// claim, checked: the document the crate ships is the published one of both
/// versions, as JSON.
#[test]
fn the_shipped_common_types_are_the_published_ones() {
    let shipped = Path::new(env!("CARGO_MANIFEST_DIR")).join("schemas/v0_9_1/common_types.json");
    let shipped: Value =
        serde_json::from_str(&std::fs::read_to_string(shipped).expect("the shipped document"))
            .expect("JSON");
    for case in cases_of("core/common_types.yaml") {
        if case.get("action").and_then(Value::as_str) != Some("common_types_schema")
            || matches!(case_version(&case), CaseVersion::Unsupported(_))
        {
            continue;
        }
        let path = case["expectFile"].as_str().expect("expectFile");
        let published = repository_document(path).unwrap_or_else(|reason| panic!("{reason}"));
        assert_eq!(shipped, published, "{path}");
    }
}

fn load_vendored(relative: &str) -> Value {
    load_catalog_schema(Some(&Value::String(relative.to_string())))
        .ok()
        .flatten()
        .unwrap_or(Value::Null)
}

// --- validate, current layout ---------------------------------------------

/// The scope the `validate` cases are compared at.
///
/// Upstream runs every message through a JSON Schema engine as well as its
/// structural validator. This crate's semantic validator checks natively the
/// part of that a generating model gets wrong — the envelope, component types,
/// required properties, property types and enums — so those are on. Data
/// bindings are not resolved, because upstream's validator does not look at the
/// data model; pointer syntax is checked, since upstream checks it too.
fn conformance_options() -> ValidateOptions {
    ValidateOptions {
        check_bindings: false,
        check_binding_syntax: true,
        ..ValidateOptions::full_surface()
    }
}

/// The catalog a current-layout case validates against: an inline `catalog`
/// document, or the specification document `catalogPaths` names.
fn case_catalog(case: &Value) -> Result<Catalog, Outcome> {
    let document = match (case.get("catalog"), case.get("catalogPaths")) {
        (Some(inline), _) if inline.get("components").is_some() => inline.clone(),
        (_, Some(Value::Array(paths))) => {
            let [Value::String(path)] = paths.as_slice() else {
                return Err(Outcome::Skipped(format!(
                    "validates against {} catalogs at once",
                    paths.len()
                )));
            };
            let text = std::fs::read_to_string(fixture(path)).map_err(|_| {
                Outcome::Skipped(format!(
                    "catalog {path} is not vendored under tests/conformance/"
                ))
            })?;
            serde_json::from_str(&text)
                .map_err(|e| Outcome::Failed(format!("catalog {path} is not JSON: {e}")))?
        }
        _ => return Ok(Catalog::empty("conformance")),
    };
    Catalog::from_schema(&document)
        .map_err(|error| Outcome::Failed(format!("catalog rejected: {error}")))
}

/// A `validate` case in the current layout: one validator, and one surface
/// state, across the case's steps. A later step is checked against the
/// surfaces earlier steps left, which is what
/// [`Validator::validate_json_updates`] is for.
///
/// A step that fails changes nothing: the state the next step sees is the one
/// the last accepted step left.
fn run_validate_steps(check: &str, case: &Value, tally: &mut Tally) {
    let catalog = match case_catalog(case) {
        Ok(catalog) => catalog,
        Err(outcome) => {
            tally.record(check, outcome);
            return;
        }
    };
    let validator = Validator::with_options(&catalog, conformance_options());
    let steps: Vec<&Value> = match case.get("steps").and_then(Value::as_array) {
        Some(steps) => steps.iter().collect(),
        None => vec![case],
    };
    let mut state = SurfaceStore::new();
    for (index, step) in steps.iter().enumerate() {
        let id = format!("{check}[step {index}]");
        let messages: Vec<Value> = match step.get("messages").or_else(|| step.get("payload")) {
            Some(Value::Array(items)) => items.clone(),
            Some(Value::Object(wrapper)) if wrapper.contains_key("messages") => {
                wrapper["messages"].as_array().cloned().unwrap_or_default()
            }
            Some(other) => vec![other.clone()],
            None => {
                tally.record(&id, Outcome::Skipped("step has no messages".to_string()));
                continue;
            }
        };
        let expect_error = field(step, "expect_error").or_else(|| field(case, "expect_error"));
        let (outcome, next) = validate_step(&id, &validator, &state, &messages, expect_error);
        if let Some(next) = next {
            state = next;
        }
        tally.record(&id, outcome);
    }
}

fn validate_step(
    id: &str,
    validator: &Validator<'_>,
    state: &SurfaceStore,
    messages: &[Value],
    expect_error: Option<&Value>,
) -> (Outcome, Option<SurfaceStore>) {
    if let Some(reason) = SCHEMA_ONLY
        .iter()
        .find(|(listed, _)| *listed == id)
        .map(|(_, reason)| *reason)
    {
        return (
            Outcome::Skipped(format!("expects a JSON Schema failure: {reason}")),
            None,
        );
    }
    let Some(expect_error) = expect_error else {
        // A step that only creates surfaces leaves them pending: upstream
        // accepts a `createSurface` with nothing drawn into it yet, where this
        // crate's whole-surface contract wants a root. The lifecycle is
        // applied; the tree is judged by the step that fills it.
        if !messages.is_empty()
            && messages
                .iter()
                .all(|message| message.get("createSurface").is_some())
        {
            let report = validator.validate_json_updates(state, messages).0;
            let pending_only = report
                .errors
                .iter()
                .all(|error| error.code == ErrorCode::EmptyComponents);
            if !pending_only {
                return (
                    Outcome::Failed(format!(
                        "expected a clean create, got {:?}",
                        located_codes(&report)
                    )),
                    None,
                );
            }
            let mut next = state.clone();
            for message in messages {
                let applied = serde_json::from_value(message.clone())
                    .map_err(|error| error.to_string())
                    .and_then(|typed| next.apply(&typed).map_err(|error| error.to_string()));
                if let Err(error) = applied {
                    return (Outcome::Failed(error), None);
                }
            }
            return (Outcome::Passed, Some(next));
        }
        let (report, next) = validator.validate_json_updates(state, messages);
        return if report.is_valid() && report.unreachable.is_empty() {
            (Outcome::Passed, Some(next))
        } else {
            (
                Outcome::Failed(format!(
                    "expected a clean step, got errors {:?} and unreachable {:?}",
                    located_codes(&report),
                    report.unreachable
                )),
                None,
            )
        };
    };

    let (report, _) = validator.validate_json_updates(state, messages);
    if let Some((_, code, location)) = CATEGORY_ONLY.iter().find(|(listed, ..)| *listed == id) {
        let found = report
            .errors
            .iter()
            .any(|error| error.code == *code && error.path.ends_with(location));
        let outcome = if found {
            Outcome::Passed
        } else {
            Outcome::Failed(format!(
                "expected {code} at …{location}, got {:?}",
                located_codes(&report)
            ))
        };
        return (outcome, None);
    }
    (judge_error(expect_error, &report), None)
}

/// Steps whose expected failure only a JSON Schema engine finds, by the id the
/// report prints. The semantic validator this harness runs does not evaluate
/// them; `SchemaValidator`, behind the `schema-validation` feature, does.
///
/// Listed by name because their fixtures are also invalid in ways the semantic
/// validator does report (a `Button` with no `child`), so a report that merely
/// has an error in it would pass them for the wrong reason.
const SCHEMA_ONLY: &[(&str, &str)] = &[
    (
        "core/validator_v0_9.yaml::test_v09_unknown_component_property[step 0]",
        "a property the component does not declare",
    ),
    (
        "core/validator_v0_9.yaml::test_v09_unknown_nested_function[step 0]",
        "a function the catalog does not declare",
    ),
    (
        "core/validator_v0_9.yaml::test_v09_function_argument_schema_mismatch_error[step 0]",
        "a function argument of the wrong type",
    ),
    (
        "core/validator_v0_9.yaml::test_v09_theme_schema_validation_error[step 0]",
        "a theme value the catalog's theme schema rejects",
    ),
    (
        "core/validator_v0_9.yaml::test_validator_theme_schema[step 1]",
        "a theme value the catalog's theme schema rejects",
    ),
];

/// What a bare `expectError: {category: ValidationError}` is about, by the id
/// the report prints.
///
/// A category names no failure, and any error at all would satisfy it. Each of
/// these was read against its case's description and names the failure this
/// crate reports for the defect the case describes: its code, and the end of
/// its locator. A category-only step that is not listed is skipped, not
/// guessed at.
const CATEGORY_ONLY: &[(&str, ErrorCode, &str)] = &[
    // "multiple invalid component schemas": a Button and a Text with none of
    // their required properties.
    (
        "core/validator_v0_9.yaml::test_v09_multiple_component_validation_errors[step 0]",
        ErrorCode::MissingRequiredProp,
        ".components[1].text",
    ),
    // A child template with a componentId and no path.
    (
        "core/validator_v0_9.yaml::test_v09_child_list_template_missing_path_error[step 0]",
        ErrorCode::InvalidValue,
        ".components[0].children",
    ),
];

/// Whether a report carries the failure a current-layout `expectError` names.
///
/// An expectation is a sentence of the reference validator, a list of coded
/// `details`, or a category with a hint in the `rendererError` it would send.
/// Each is matched on what this crate reports for the same defect: its code,
/// and where the suite gives one, its location.
fn judge_error(expect_error: &Value, report: &ValidationReport) -> Outcome {
    let got = || located_codes(report);

    if let Some(details) = expect_error.get("details").and_then(Value::as_array) {
        let mut divergent = false;
        for detail in details {
            let code = detail
                .get("code")
                .and_then(Value::as_str)
                .unwrap_or_default();
            let path = detail
                .get("path")
                .and_then(Value::as_str)
                .unwrap_or_default();
            // `messages.0.version` locates a field of the envelope and is this
            // crate's own locator once its indices are bracketed. `/children`
            // locates a property within the failing component.
            let at = |error: &&ag_ui_a2ui::ValidationError| match path.strip_prefix('/') {
                Some(pointer) => error
                    .path
                    .ends_with(&format!(".{}", bracket_indices(&pointer.replace('/', ".")))),
                None => error.path == bracket_indices(path),
            };
            let here: Vec<ErrorCode> = report.errors.iter().filter(at).map(|e| e.code).collect();
            let exact: &[ErrorCode] = match code {
                "missing_field" => &[ErrorCode::MissingField, ErrorCode::MissingRequiredProp],
                "invalid_value" => &[ErrorCode::InvalidValue, ErrorCode::UnknownComponent],
                "type_mismatch" => &[ErrorCode::TypeMismatch],
                other => {
                    return Outcome::Skipped(format!(
                        "expects the JSON Schema error code {other:?}, which this crate does not \
                         report"
                    ));
                }
            };
            if here.iter().any(|found| exact.contains(found)) {
                continue;
            }
            // A structural property of the wrong JSON type is `type_mismatch`
            // here; the reference's schema engine calls it `invalid_value`.
            if code == "invalid_value" && here.contains(&ErrorCode::TypeMismatch) {
                divergent = true;
                continue;
            }
            return Outcome::Failed(format!("expected {code} at {path}, got {:?}", got()));
        }
        return if divergent {
            Outcome::ExpectedDivergence(
                "a value of the wrong JSON type is type_mismatch here, invalid_value upstream"
                    .into(),
            )
        } else {
            Outcome::Passed
        };
    }

    let message = match expect_error {
        Value::String(sentence) => sentence.as_str(),
        other => other
            .get("message")
            .or_else(|| other.pointer("/rendererError/error/message"))
            .and_then(Value::as_str)
            .unwrap_or_default(),
    };
    let has = |code: ErrorCode| report.errors.iter().any(|error| error.code == code);
    let expect_code = |code: ErrorCode| {
        if has(code) {
            Outcome::Passed
        } else {
            Outcome::Failed(format!("expected {code} for {message:?}, got {:?}", got()))
        }
    };
    let phrase = |needle: &str| message.contains(needle);

    if message.is_empty() {
        // Nothing says which failure is meant, and an unrelated error would
        // satisfy a bare category. Not counted either way.
        Outcome::Skipped("expects an error with no message to match on (category only)".into())
    } else if phrase("Duplicate component ID") {
        expect_code(ErrorCode::DuplicateId)
    } else if phrase("Missing root component") {
        expect_code(ErrorCode::NoRoot)
    } else if phrase("references non-existent component") || phrase("Dangling reference") {
        expect_code(ErrorCode::UnresolvedChild)
    } else if phrase("Self-reference detected")
        || phrase("Circular reference detected")
        || phrase("Circular component reference")
    {
        expect_code(ErrorCode::ChildCycle)
    } else if phrase("ecursion limit") || phrase("functionCall depth") {
        expect_code(ErrorCode::MaxDepthExceeded)
    } else if phrase("Invalid path syntax") {
        // A binding path is reported where the binding is; the path of an
        // `updateDataModel` is refused with the message that carries it.
        let refused_update = report.errors.iter().any(|error| {
            error.code == ErrorCode::InvalidValue
                && error.message.starts_with("invalid data-model pointer")
        });
        if refused_update {
            Outcome::Passed
        } else {
            expect_code(ErrorCode::UnresolvedBinding)
        }
    } else if phrase("is not of type") {
        expect_code(ErrorCode::TypeMismatch)
    } else if phrase("is not reachable from") {
        // The one place the severities differ: the specification tells
        // renderers to buffer a component until its parent arrives, so this
        // crate reports an unreachable component as a warning.
        if !report.unreachable.is_empty() {
            Outcome::ExpectedDivergence(
                "an unreachable component is a warning here, an error upstream".into(),
            )
        } else {
            Outcome::Failed(format!(
                "expected an unreachable component, got {:?}",
                got()
            ))
        }
    } else if report
        .errors
        .iter()
        .any(|error| error.to_string().contains(message))
    {
        // No phrase of the reference validator: the suite names the thing the
        // error must be about, and an error here names it too.
        Outcome::Passed
    } else if report
        .unreachable
        .iter()
        .any(|id| message.contains(id.as_str()))
    {
        Outcome::ExpectedDivergence(
            "an unreachable component is a warning here, an error upstream".into(),
        )
    } else {
        Outcome::Failed(format!(
            "expected an error mentioning {message:?}, got {:?}",
            got()
        ))
    }
}

// --- validate, retired layout ---------------------------------------------

fn run_validate_case(case_id: &str, case: &Value, tally: &mut Tally) {
    let catalog_config = case.get("catalog");
    let version = catalog_config
        .and_then(|c| c.get("version"))
        .map(render_version)
        .unwrap_or_default();

    if !version.starts_with("0.9") && !version.starts_with("v0.9") {
        tally.record(
            case_id,
            Outcome::Skipped(format!(
                "protocol {version}: this crate targets the v0.9 wire format only"
            )),
        );
        return;
    }

    let catalog = match load_catalog_schema(catalog_config.and_then(|c| field(c, "catalog_schema")))
    {
        Ok(Some(schema)) => match Catalog::from_schema(&schema) {
            Ok(catalog) => catalog,
            Err(error) => {
                tally.record(
                    case_id,
                    Outcome::Failed(format!("catalog rejected: {error}")),
                );
                return;
            }
        },
        Ok(None) => Catalog::empty("conformance"),
        Err(reason) => {
            tally.record(case_id, Outcome::Skipped(reason));
            return;
        }
    };

    match case.get("steps").and_then(Value::as_array) {
        Some(steps) => {
            for (index, step) in steps.iter().enumerate() {
                let step_id = format!("{case_id}[step {index}]");
                let outcome = run_validate_step(
                    &catalog,
                    step.get("payload"),
                    field(step, "expect_error").or_else(|| field(case, "expect_error")),
                );
                tally.record(&step_id, outcome);
            }
        }
        None => {
            let outcome =
                run_validate_step(&catalog, case.get("payload"), field(case, "expect_error"));
            tally.record(case_id, outcome);
        }
    }
}

/// Where a fixture a case names by relative path is vendored.
///
/// Paths are relative to upstream's `conformance/` directory, or to the
/// repository root for a specification document; both are mirrored under
/// `tests/conformance/`. A fixture only the retired suites still name is kept
/// with them, so `retired/` is searched second.
fn fixture(relative: &str) -> PathBuf {
    let current = conformance_dir().join(relative);
    if current.exists() {
        return current;
    }
    let retired = conformance_dir().join("retired").join(relative);
    if retired.exists() { retired } else { current }
}

/// Resolves a case's `catalog_schema`, which is either inline or a path
/// relative to the vendored `conformance/` directory.
fn load_catalog_schema(schema: Option<&Value>) -> Result<Option<Value>, String> {
    match schema {
        None | Some(Value::Null) => Ok(None),
        Some(Value::Object(_)) => Ok(schema.cloned()),
        Some(Value::String(relative)) => {
            let path = fixture(relative);
            let text = std::fs::read_to_string(&path).map_err(|_| {
                format!("catalog file {relative} is not vendored under tests/conformance/")
            })?;
            serde_json::from_str(&text)
                .map(Some)
                .map_err(|e| format!("catalog file {relative} is not JSON: {e}"))
        }
        Some(other) => Err(format!("unsupported catalog_schema form: {other}")),
    }
}

fn render_version(value: &Value) -> String {
    match value {
        Value::String(s) => s.clone(),
        other => other.to_string().trim_matches('"').to_string(),
    }
}

fn run_validate_step(
    catalog: &Catalog,
    payload: Option<&Value>,
    expect_error: Option<&Value>,
) -> Outcome {
    let Some(payload) = payload else {
        return Outcome::Skipped("step has no payload".to_string());
    };
    let expectation = classify_expectation(expect_error);
    if let Expectation::Unsupported(reason) = &expectation {
        return Outcome::Skipped(reason.clone());
    }

    let messages: Vec<Value> = match payload {
        Value::Array(items) => items.clone(),
        other => vec![other.clone()],
    };

    // Upstream's structural validator checks references, roots, cycles and
    // nesting depth; required-property checking is JSON Schema's job there, and
    // data bindings are not checked at all. Matching that scope keeps the
    // comparison honest rather than failing cases on checks upstream never ran.
    // The root and dangling-reference contract is chosen from the payload by
    // `validate_json_messages`, exactly as upstream chooses it.
    let options = ValidateOptions {
        check_component_types: false,
        check_required_props: false,
        check_bindings: false,
        // Pointer syntax is checked: upstream's structural validator checks it
        // too, and it needs no data model.
        check_binding_syntax: true,
        // The envelope and the declared property types are what upstream hands
        // to a JSON Schema engine. This crate checks both natively, so the cases
        // that assert those failures run here rather than being skipped.
        check_envelope: true,
        check_prop_types: true,
        ..ValidateOptions::full_surface()
    };
    let report = if matches!(expectation, Expectation::Valid)
        && messages.iter().all(|m| m.get("createSurface").is_some())
    {
        // A create-only step is a pending stream, not a finalized surface.
        let mut state = ag_ui_a2ui::surface::SurfaceStore::new();
        for message in &messages {
            let op = match serde_json::from_value(message.clone()) {
                Ok(op) => op,
                Err(e) => return Outcome::Failed(e.to_string()),
            };
            if let Err(e) = state.apply(&op) {
                return Outcome::Failed(e.to_string());
            }
        }
        ValidationReport::default()
    } else if matches!(expectation, Expectation::Valid)
        && messages.iter().all(|m| m.get("updateComponents").is_some())
    {
        // Legacy vectors provide component fragments without renderer state.
        // Exercise their structural contract directly; lifecycle has its own
        // whole-stream regressions in protocol_091.rs.
        let components: Vec<_> = messages
            .iter()
            .filter_map(|m| {
                m.pointer("/updateComponents/components")
                    .and_then(Value::as_array)
            })
            .flatten()
            .cloned()
            .collect();
        let options = ValidateOptions {
            require_root: false,
            allow_dangling_children: true,
            ..options
        };
        Validator::with_options(catalog, options).validate_json(&components, None)
    } else {
        Validator::with_options(catalog, options).validate_json_messages(&messages)
    };

    match expectation {
        Expectation::Unsupported(reason) => Outcome::Skipped(reason),
        Expectation::Details(details) => {
            let missing: Vec<String> = details
                .iter()
                .filter(|want| {
                    !report
                        .errors
                        .iter()
                        .any(|error| error.code == want.code && error.path == want.path)
                })
                .map(|want| format!("{} at {}", want.code, want.path))
                .collect();
            if missing.is_empty() {
                Outcome::Passed
            } else {
                Outcome::Failed(format!(
                    "expected {missing:?}, got {:?}",
                    located_codes(&report)
                ))
            }
        }
        Expectation::Valid => {
            if report.is_valid() && report.unreachable.is_empty() {
                Outcome::Passed
            } else {
                Outcome::Failed(format!(
                    "expected a clean payload, got errors {:?} and unreachable {:?}",
                    codes(&report),
                    report.unreachable
                ))
            }
        }
        Expectation::Code(code) => {
            if report.errors.iter().any(|e| e.code == code) {
                Outcome::Passed
            } else {
                Outcome::Failed(format!(
                    "expected error code {code}, got {:?}",
                    codes(&report)
                ))
            }
        }
        Expectation::Unreachable => {
            if !report.unreachable.is_empty() {
                Outcome::Passed
            } else {
                Outcome::Failed(format!(
                    "expected an unreachable component, got errors {:?}",
                    codes(&report)
                ))
            }
        }
    }
}

fn codes(report: &ValidationReport) -> Vec<&'static str> {
    report.errors.iter().map(|e| e.code.as_str()).collect()
}

fn located_codes(report: &ValidationReport) -> Vec<String> {
    report
        .errors
        .iter()
        .map(|e| format!("{} at {}", e.code, e.path))
        .collect()
}

/// What a conformance expectation means for this crate.
enum Expectation {
    /// The payload should validate cleanly.
    Valid,
    /// The payload should produce this error code.
    Code(ErrorCode),
    /// The payload should produce each of these coded failures, at these paths.
    ///
    /// Not an equality check: this crate may report more than upstream's engine
    /// did for the same payload, and reporting more is not a conformance
    /// failure. Every failure upstream pins has to be there, code and locator
    /// both.
    Details(Vec<ExpectedDetail>),
    /// The payload should leave a component unreachable from the root.
    ///
    /// Upstream raises this as an error. This crate reports it as a warning,
    /// because the specification tells renderers to buffer components until
    /// their parent arrives — so the condition must still be *detected*, which
    /// is what this checks.
    Unreachable,
    /// The expectation depends on behaviour this crate does not implement.
    Unsupported(String),
}

/// Maps an upstream expectation onto this crate's outcomes.
///
/// The mapping is by the upstream error text, which is stable across their SDKs
/// because the conformance suite matches on it.
fn classify_expectation(expect_error: Option<&Value>) -> Expectation {
    let Some(expect_error) = expect_error else {
        return Expectation::Valid;
    };

    // A `details` list pins exact error codes and paths from envelope
    // validation. This crate reports the envelope contract natively and uses the
    // same code spellings, so each detail is matched on both.
    if let Some(details) = expect_error.get("details").and_then(Value::as_array) {
        let mut wanted = Vec::new();
        for detail in details {
            let code = detail
                .get("code")
                .and_then(Value::as_str)
                .unwrap_or_default();
            let Some(code) = detail_code(code) else {
                return Expectation::Unsupported(format!(
                    "expects the JSON Schema error code {code:?}, which this crate does not report"
                ));
            };
            let path = detail
                .get("path")
                .and_then(Value::as_str)
                .unwrap_or_default();
            wanted.push(ExpectedDetail {
                code,
                path: bracket_indices(path),
            });
        }
        return Expectation::Details(wanted);
    }

    let message = match expect_error {
        Value::String(s) => s.clone(),
        other => other
            .get("message")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string(),
    };

    if message.contains("Duplicate component ID") {
        Expectation::Code(ErrorCode::DuplicateId)
    } else if message.contains("Missing root component") {
        Expectation::Code(ErrorCode::NoRoot)
    } else if message.contains("references non-existent component") {
        Expectation::Code(ErrorCode::UnresolvedChild)
    } else if message.contains("Self-reference detected")
        || message.contains("Circular reference detected")
    {
        Expectation::Code(ErrorCode::ChildCycle)
    } else if message.contains("is not reachable from") {
        Expectation::Unreachable
    } else if message.contains("ecursion limit") {
        Expectation::Code(ErrorCode::MaxDepthExceeded)
    } else if message.contains("Invalid path syntax") {
        Expectation::Code(ErrorCode::UnresolvedBinding)
    } else if message.contains("is not of type") {
        // JSON Schema's wording for a value whose type the catalog rejects.
        Expectation::Code(ErrorCode::TypeMismatch)
    } else if message.is_empty() {
        Expectation::Unsupported(
            "expects an error with no message to match on (category only)".to_string(),
        )
    } else {
        Expectation::Unsupported(format!(
            "expects a JSON Schema validation failure: {:?}",
            truncate(&message)
        ))
    }
}

/// One entry of an upstream `expect_error.details` list.
struct ExpectedDetail {
    code: ErrorCode,
    path: String,
}

/// Maps an upstream JSON Schema error code onto this crate's.
///
/// The three below are the ones the envelope produces. Anything else is a
/// constraint this crate does not evaluate, and the case is skipped rather than
/// stretched to fit.
fn detail_code(code: &str) -> Option<ErrorCode> {
    match code {
        "missing_field" => Some(ErrorCode::MissingField),
        "invalid_value" => Some(ErrorCode::InvalidValue),
        "type_mismatch" => Some(ErrorCode::TypeMismatch),
        _ => None,
    }
}

/// Rewrites an upstream locator into this crate's dialect.
///
/// Upstream separates every segment with a dot, list indices included
/// (`messages.0.version`); this crate brackets indices (`messages[0].version`).
/// Same locator, different spelling.
fn bracket_indices(path: &str) -> String {
    let mut out = String::new();
    for segment in path.split('.') {
        if segment.parse::<usize>().is_ok() {
            out.push('[');
            out.push_str(segment);
            out.push(']');
        } else {
            if !out.is_empty() {
                out.push('.');
            }
            out.push_str(segment);
        }
    }
    out
}

fn truncate(text: &str) -> String {
    if text.chars().count() <= 60 {
        return text.to_string();
    }
    text.chars().take(57).collect::<String>() + "..."
}

// --- streaming parser ------------------------------------------------------

fn run_process_chunk(name: &str, case: &Value) -> Outcome {
    let catalog_config = case.get("catalog");
    let catalog = match load_catalog_schema(catalog_config.and_then(|c| field(c, "catalog_schema")))
    {
        Ok(Some(schema)) => match Catalog::from_schema(&schema) {
            Ok(catalog) => catalog,
            Err(error) => return Outcome::Failed(format!("catalog rejected: {error}")),
        },
        Ok(None) => Catalog::empty("conformance"),
        Err(reason) => return Outcome::Skipped(reason),
    };

    let mut parser = StreamParser::new(catalog);
    if let Some(keys) = catalog_config
        .and_then(|c| field(c, "custom_cuttable_keys"))
        .and_then(Value::as_array)
    {
        parser = parser.with_cuttable_keys(keys.iter().filter_map(Value::as_str));
    }
    if field(case, "disable_validation").and_then(Value::as_bool) == Some(true) {
        parser = parser.without_validation();
    }

    let Some(steps) = case.get("steps").and_then(Value::as_array) else {
        return Outcome::Skipped("case has no steps".to_string());
    };
    let divergence = streaming_divergence(name);
    let mut saw_safe_alternative = false;

    for (index, step) in steps.iter().enumerate() {
        let input = step
            .get("input")
            .and_then(Value::as_str)
            .unwrap_or_default();
        let result = parser.process_chunk(input);

        if let Some(expect_error) = field(step, "expect_error") {
            let wanted = expected_message(expect_error);
            return match result {
                Ok(parts) => Outcome::Failed(format!(
                    "step {index}: expected an error matching {wanted:?}, got {parts:?}"
                )),
                Err(error) => {
                    let text = error.to_string();
                    if wanted.is_empty() || text.contains(&wanted) {
                        Outcome::Passed
                    } else {
                        Outcome::Failed(format!(
                            "step {index}: expected {wanted:?} in error, got {text:?}"
                        ))
                    }
                }
            };
        }

        let parts = match result {
            Ok(parts) => parts,
            Err(error) => {
                return Outcome::Failed(format!("step {index}: unexpected error: {error}"));
            }
        };
        let upstream_expected = step
            .get("expect")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        let expected = match safe_streaming_expectation(name, index, steps, &upstream_expected) {
            Ok(Some(expected)) => {
                saw_safe_alternative = true;
                expected
            }
            Ok(None) => upstream_expected,
            Err(error) => return Outcome::Failed(format!("step {index}: {error}")),
        };
        if parts.len() != expected.len() {
            return Outcome::Failed(format!(
                "step {index}: expected {} part(s), got {}: {}",
                expected.len(),
                parts.len(),
                describe_parts(&parts)
            ));
        }
        for (part, want) in parts.iter().zip(&expected) {
            let want_text = want.get("text").and_then(Value::as_str).unwrap_or("");
            if part.text != want_text {
                return Outcome::Failed(format!(
                    "step {index}: expected text {want_text:?}, got {:?}",
                    part.text
                ));
            }
            let want_a2ui = want.get("a2ui").and_then(Value::as_array);
            match (&part.a2ui, want_a2ui) {
                (Some(actual), Some(expected)) if actual == expected => {}
                (None, None) => {}
                (actual, expected) => {
                    return Outcome::Failed(format!(
                        "step {index}: expected a2ui {}, got {}",
                        serde_json::to_string(&expected).unwrap_or_default(),
                        serde_json::to_string(&actual).unwrap_or_default()
                    ));
                }
            }
        }
    }
    match divergence {
        Some(reason) if saw_safe_alternative => Outcome::ExpectedDivergence(reason.into()),
        Some(_) => Outcome::Failed("pinned reference difference was not exercised".into()),
        None => Outcome::Passed,
    }
}

/// Pins differences in partial-message timing and catalog reference semantics.
fn streaming_divergence(name: &str) -> Option<&'static str> {
    match name {
        "test_delta_streaming_correctness_v09" => Some("partial message has no required version"),
        "test_incremental_data_model_streaming_v09"
        | "test_sniff_partial_invalid_datamodel_fails_gracefully_v09"
        | "test_sniff_partial_datamodel_with_cut_key_v09"
        | "test_sniff_partial_datamodel_cumulative_unmodified_keys_v09"
        | "test_sniff_partial_datamodel_prunes_empty_keys_v09"
        | "test_sniff_partial_datamodel_prunes_empty_trailing_dict_v09" => {
            Some("partial root update has no settled path")
        }
        "test_cut_unicode_multibyte_emoji_v09" => Some(
            "half a surrogate pair is not a string in Rust; nothing is emitted until it is whole",
        ),
        _ => None,
    }
}

fn safe_streaming_expectation(
    name: &str,
    index: usize,
    steps: &[Value],
    upstream: &[Value],
) -> Result<Option<Vec<Value>>, String> {
    if name == "test_cut_unicode_multibyte_emoji_v09" && index == 1 {
        // Upstream emits the text cut after the first half of the pair. The
        // suite loader reads that half as U+FFFD; see `readable`.
        let text = upstream
            .first()
            .and_then(|part| part.pointer("/a2ui/0/updateComponents/components/0/text"));
        if upstream.len() != 1 || text != Some(&serde_json::json!("Rocket \u{FFFD}")) {
            return Err("pinned upstream surrogate expectation changed".into());
        }
        return Ok(Some(Vec::new()));
    }
    let suppress = match name {
        "test_delta_streaming_correctness_v09" => matches!(index, 5 | 6),
        "test_incremental_data_model_streaming_v09" => matches!(index, 3 | 4),
        "test_sniff_partial_invalid_datamodel_fails_gracefully_v09" => index == 0,
        "test_sniff_partial_datamodel_with_cut_key_v09" => matches!(index, 0 | 1),
        "test_sniff_partial_datamodel_cumulative_unmodified_keys_v09" => {
            matches!(index, 1 | 2)
        }
        "test_sniff_partial_datamodel_prunes_empty_keys_v09"
        | "test_sniff_partial_datamodel_prunes_empty_trailing_dict_v09" => index == 0,
        _ => false,
    };
    if suppress {
        let key = if name == "test_delta_streaming_correctness_v09" {
            "updateComponents"
        } else {
            "updateDataModel"
        };
        if upstream.len() != 1
            || upstream[0]["a2ui"]
                .as_array()
                .is_none_or(|ops| ops.len() != 1)
            || upstream[0]["a2ui"][0].get(key).is_none()
        {
            return Err("pinned upstream partial expectation changed".into());
        }
        return Ok(Some(Vec::new()));
    }
    if name == "test_sniff_partial_datamodel_with_cut_key_v09" && index == 3 {
        if upstream.len() != 1 || upstream[0]["a2ui"][0].get("updateDataModel").is_none() {
            return Err("pinned upstream completion expectation changed".into());
        }
        let mut source = String::new();
        for step in steps.iter().take(index + 1) {
            source.push_str(
                step.get("input")
                    .and_then(Value::as_str)
                    .unwrap_or_default(),
            );
        }
        let inner = source
            .strip_prefix("<a2ui-json>")
            .and_then(|text| text.strip_suffix("</a2ui-json>"))
            .ok_or("pinned upstream block delimiters changed")?;
        if serde_json::from_str::<Value>(inner).is_ok() {
            return Err("pinned upstream final block became valid JSON".into());
        }
        return Ok(Some(Vec::new()));
    }
    Ok(None)
}

fn describe_parts(parts: &[ag_ui_a2ui::toolkit::parser::ResponsePart]) -> String {
    parts
        .iter()
        .map(|part| {
            format!(
                "{{text: {:?}, a2ui: {}}}",
                part.text,
                serde_json::to_string(&part.a2ui).unwrap_or_default()
            )
        })
        .collect::<Vec<_>>()
        .join(", ")
}

// --- parser ---------------------------------------------------------------

fn run_parse_full(case: &Value) -> Outcome {
    let input = case
        .get("input")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let result = parse_response(input);

    if let Some(expect_error) = field(case, "expect_error") {
        let wanted = expected_message(expect_error);
        return match result {
            Ok(_) => Outcome::Failed(format!("expected an error matching {wanted:?}, got parts")),
            Err(error) => {
                let text = error.to_string();
                if wanted.is_empty() || text.contains(&wanted) {
                    Outcome::Passed
                } else {
                    Outcome::Failed(format!("expected {wanted:?} in error, got {text:?}"))
                }
            }
        };
    }

    let Some(expected) = case.get("expect").and_then(Value::as_array) else {
        return Outcome::Skipped("case has neither 'expect' nor 'expect_error'".to_string());
    };
    let parts = match result {
        Ok(parts) => parts,
        Err(error) => return Outcome::Failed(format!("unexpected parse error: {error}")),
    };
    if parts.len() != expected.len() {
        return Outcome::Failed(format!(
            "expected {} part(s), got {}",
            expected.len(),
            parts.len()
        ));
    }
    for (part, want) in parts.iter().zip(expected) {
        let want_text = want.get("text").and_then(Value::as_str).unwrap_or("");
        if part.text != want_text {
            return Outcome::Failed(format!("expected text {want_text:?}, got {:?}", part.text));
        }
        let want_a2ui = want.get("a2ui").and_then(Value::as_array);
        match (&part.a2ui, want_a2ui) {
            (Some(actual), Some(expected)) if actual == expected => {}
            (None, None) => {}
            (actual, expected) => {
                return Outcome::Failed(format!("expected a2ui {expected:?}, got {actual:?}"));
            }
        }
    }
    Outcome::Passed
}

fn run_fix_payload(case: &Value) -> Outcome {
    let input = case
        .get("input")
        .and_then(Value::as_str)
        .unwrap_or_default();
    if let Some(expect_error) = field(case, "expect_error") {
        let wanted = expected_message(expect_error);
        return match parse_and_fix(input) {
            Ok(fixed) => Outcome::Failed(format!(
                "expected an error matching {wanted:?}, got {fixed:?}"
            )),
            Err(error) if error.to_string().contains(&wanted) => Outcome::Passed,
            Err(error) => Outcome::Failed(format!("expected {wanted:?} in error, got {error}")),
        };
    }
    let Some(expected) = case.get("expect").and_then(Value::as_array) else {
        return Outcome::Skipped("case has no array 'expect'".to_string());
    };
    match parse_and_fix(input) {
        Ok(actual) if &actual == expected => Outcome::Passed,
        Ok(actual) => Outcome::Failed(format!("expected {expected:?}, got {actual:?}")),
        Err(error) => Outcome::Failed(format!("unexpected error: {error}")),
    }
}

fn run_has_parts(case: &Value) -> Outcome {
    let input = case
        .get("input")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let Some(expected) = case.get("expect").and_then(Value::as_bool) else {
        return Outcome::Skipped("case has no boolean 'expect'".to_string());
    };
    let actual = has_a2ui_parts(input);
    if actual == expected {
        Outcome::Passed
    } else {
        Outcome::Failed(format!("expected {expected}, got {actual}"))
    }
}

/// The message text an `expect_error` wants matched.
///
/// Upstream treats these as regexes; every one used by the suites this harness
/// executes is a plain substring, so a substring check is exact here and avoids
/// pulling in a regex dependency. A pattern with regex metacharacters would be
/// reported as a mismatch rather than passing by accident.
fn expected_message(expect_error: &Value) -> String {
    match expect_error {
        Value::String(s) => s.clone(),
        other => other
            .get("message")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string(),
    }
}

#[test]
fn every_vendored_suite_parses() {
    for suite in SUITES {
        let cases = cases_of(suite.path);
        assert!(!cases.is_empty(), "{} is empty", suite.path);
        for case in &cases {
            assert_ne!(
                case_name(case),
                "<unnamed>",
                "{} has an unnamed case",
                suite.path
            );
            // The suites this harness executes dispatch on `action`.
            if !matches!(suite.plan, Plan::Skip(..)) {
                assert!(
                    case.get("action").is_some(),
                    "{} case {:?} has no action",
                    suite.path,
                    case_name(case)
                );
            }
        }
    }
}

/// The key spellings of the two pins are the same key.
#[test]
fn a_case_key_is_read_in_either_spelling() {
    let retired = serde_json::json!({"expect_error": 1, "s2c_schema": 2});
    let current = serde_json::json!({"expectError": 1, "s2cSchema": 2, "catalogId": 3});
    for case in [&retired, &current] {
        assert_eq!(field(case, "expect_error"), Some(&serde_json::json!(1)));
        assert_eq!(field(case, "s2c_schema"), Some(&serde_json::json!(2)));
        assert_eq!(field(case, "client_ui_capabilities"), None);
    }
}
