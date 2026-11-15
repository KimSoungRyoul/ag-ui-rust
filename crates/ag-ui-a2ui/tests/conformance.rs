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
//! # The agent tier
//!
//! The suites under `agent/` are run by the module in `conformance_agent/`.
//! Upstream writes them against A2UI v1.0; a case is lowered to v0.9 on the
//! way in and lifted on the way out, by the rules of
//! `conformance_agent/lowering.rs` and nowhere else, and a case v0.9 cannot
//! state is skipped. The report says, per suite, how many cases ran lowered,
//! how many as written and how many were skipped under each label.
//!
//! Run with `cargo test -p ag-ui-a2ui --all-features --test conformance --
//! --nocapture` to see the report, and set `A2UI_CONFORMANCE_VERBOSE=1` to list
//! every check by name.

// The standing of every suite is one table, so the harness needs every layer
// the table speaks of.
#![cfg(all(feature = "toolkit", feature = "agent"))]

#[path = "conformance_agent/mod.rs"]
mod agent_tier;

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use ag_ui_a2ui::catalog::Catalog;
use ag_ui_a2ui::surface::SurfaceStore;
use ag_ui_a2ui::toolkit::negotiate::{CatalogRegistry, ClientCapabilities, select_catalog_schema};
use ag_ui_a2ui::toolkit::parser::{has_a2ui_parts, parse_and_fix, parse_response};
use ag_ui_a2ui::toolkit::prompt::{GENERATION_GUIDELINES, PromptSpec, build_subagent_prompt};
use ag_ui_a2ui::toolkit::schema::{SchemaBundle, load_examples, remove_strict_validation};
use ag_ui_a2ui::toolkit::streaming::StreamParser;
use ag_ui_a2ui::validate::{ErrorCode, ValidateOptions, ValidationReport, Validator};
use ag_ui_a2ui::version::A2uiVersion;
use serde_json::Value;

/// Upstream commit the vendored suite was taken from.
const UPSTREAM_COMMIT: &str = "3ed9601862ecc78027e20ac80ad8b1b98a5d9079";

/// Upstream commit of the suites under `retired/`, which upstream has since
/// deleted or rewritten under other names.
const RETIRED_COMMIT: &str = "44a420b67957fafc0b02d55a153fdaf72e32ffb5";

/// Why a whole suite is not executed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Why {
    /// It tests an output format other than direct JSON, or something built
    /// on one. #74 adds them.
    Formats,
    /// It times an operation instead of checking its result. #75 makes the
    /// direct JSON stream linear and runs the timing cases in a job of their
    /// own.
    Timing,
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
    /// It tests the pre-1.0 core this crate implements, and this harness has
    /// no runner for its action yet. Issue #72 adds the runners.
    Unharnessed,
}

impl Why {
    /// The label the per-suite table carries.
    fn label(self) -> &'static str {
        match self {
            Why::Formats => agent_tier::NEEDS_FORMAT,
            Why::Timing => "later: timing (#75)",
            Why::Adapters => "later: adapters (#43)",
            Why::V1_0 => "later: A2UI v1.0 (#42)",
            Why::V0_8 => "out of scope: v0.8",
            Why::Renderer => "out of scope: renderer",
            Why::NotApplicable => "not applicable",
            Why::Unharnessed => "no runner yet (#72)",
        }
    }
}

/// What this crate does with one vendored suite.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Plan {
    /// Its cases are executed. One of them may still be skipped, with a reason.
    Run,
    /// An agent-tier suite, executed by the runners in `conformance_agent/`.
    /// A case written against v1.0 runs lowered to v0.9; one that v0.9 cannot
    /// state is skipped, with the reason.
    Agent,
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

const fn agent(path: &'static str, counts: [usize; 4]) -> Suite {
    Suite {
        path,
        plan: Plan::Agent,
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
    retired("retired/core/validator.yaml", [26, 0, 25, 0]),
    retired("retired/core/catalog.yaml", [23, 0, 1, 0]),
    retired("retired/agent/inference_format.yaml", [13, 0, 6, 0]),

    // --- core tier, pre-1.0: in scope, no runner in this harness yet (#72) ---------------------
    skip("core/message_processor_v0_9.yaml", 48, Why::Unharnessed, "the surface state machine, driven as a renderer's MessageProcessor: catalog registry, rendererError payloads"),
    skip("core/data_model.yaml", 52, Why::Unharnessed, "data-model reads, writes and deletion, with a renderer's observer notifications"),
    skip("core/data_context.yaml", 11, Why::Unharnessed, "resolving a relative path against a scope; the crate has no public entry point that returns the absolute path"),
    skip("core/expressions.yaml", 98, Why::Unharnessed, "the formatString template parser; the crate evaluates a template and exposes no parse"),
    skip("core/catalog.yaml", 73, Why::Unharnessed, "the reference catalog API: fromJson, toJson, round trip, validationSchema, derive"),
    skip("core/common_types.yaml", 22, Why::Unharnessed, "values against the published common types; 3 cases compare a schema generated from an SDK's own types"),

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
    agent("agent/request_processor.yaml", [20, 0, 9, 0]),
    agent("agent/catalog_resolution.yaml", [23, 0, 0, 0]),
    agent("agent/catalog_provider.yaml", [9, 0, 1, 0]),
    agent("agent/catalog_transformer.yaml", [15, 0, 0, 0]),
    agent("agent/renderer_capabilities.yaml", [5, 0, 19, 0]),
    agent("agent/renderer_state.yaml", [5, 0, 4, 0]),
    agent("agent/streamed_answer.yaml", [6, 0, 0, 0]),
    agent("agent/direct_json/prompt_generator.yaml", [1, 0, 14, 0]),
    agent("agent/direct_json/response_parser.yaml", [35, 0, 1, 0]),
    agent("agent/direct_json/response_streaming.yaml", [91, 3, 45, 0]),
    agent("agent/direct_json/compiler.yaml", [18, 0, 20, 0]),
    agent("agent/direct_json/decompiler.yaml", [9, 0, 2, 0]),
    skip("agent/direct_json/scaling.yaml", 7, Why::Timing, "the direct JSON format: parse time against payload size"),
    skip("agent/skill.yaml", 12, Why::Formats, "skill documents generated from a format and its catalogs; the goldens are Express"),
    skip("agent/express/prompt_generator.yaml", 23, Why::Formats, "the Express format: system prompt snippet"),
    skip("agent/express/response_parser.yaml", 38, Why::Formats, "the Express format: unwrapping and parsing a response"),
    skip("agent/express/response_streaming.yaml", 41, Why::Formats, "the Express format: parsing a response chunk by chunk"),
    skip("agent/express/compiler.yaml", 135, Why::Formats, "the Express format: compiling a block into messages"),
    skip("agent/express/decompiler.yaml", 49, Why::Formats, "the Express format: writing messages back as notation"),
    skip("agent/express/scaling.yaml", 1, Why::Formats, "the Express format: compile time against payload size"),
    skip("agent/atom/compiler.yaml", 7, Why::Formats, "the Atom format"),
    skip("agent/elemental/compiler.yaml", 8, Why::Formats, "the Elemental format"),
    skip("agent/builder/builder.yaml", 22, Why::Formats, "the typesafe builder API"),
    skip("agent/macros/macros.yaml", 70, Why::Formats, "builder macros"),
    skip("agent/multi_catalog_formats.yaml", 35, Why::V1_0, "components and function calls resolved across several active catalogs"),
    skip("agent/agent_functions.yaml", 64, Why::V1_0, "agent functions and the agent's side of renderer messages"),
    skip("agent/renderer_function_calls.yaml", 30, Why::V1_0, "the agent calling renderer functions"),

    // --- end to end: the agent half parses a transcript, a renderer half draws it ---------------
    agent("e2e/v0_9_1/login_form_action.yaml", [1, 0, 1, 0]),
    agent("e2e/v0_9_1/missing_surface_error.yaml", [1, 0, 1, 0]),
    agent("e2e/v0_9_1/newsletter_signup.yaml", [1, 0, 1, 0]),
    agent("e2e/v0_9_1/restaurant_list_booking.yaml", [1, 0, 1, 0]),
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
    /// Agent-tier cases written against v1.0 and executed lowered to v0.9.
    lowered: usize,
    /// Agent-tier cases executed as they are written.
    as_written: usize,
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
        // An agent-tier case this crate answers differently on purpose. The
        // case is executed and has to go on ending the way the entry says.
        let outcome = match AGENT_DIVERGENCES
            .iter()
            .find(|(listed, _, _)| *listed == check)
        {
            None => outcome,
            Some((_, reason, ends_with)) => match outcome {
                Outcome::Failed(detail) if detail.contains(ends_with) => {
                    Outcome::ExpectedDivergence((*reason).to_string())
                }
                Outcome::Failed(detail) => Outcome::Failed(format!(
                    "listed in AGENT_DIVERGENCES as ending in {ends_with:?}, but it ended: {detail}"
                )),
                other => Outcome::Failed(format!(
                    "listed in AGENT_DIVERGENCES, but it no longer diverges ({other:?}); remove \
                     it from the list"
                )),
            },
        };
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
];

/// Set `A2UI_CONFORMANCE_VERBOSE=1` to list every executed check by name.
fn verbose() -> bool {
    std::env::var_os("A2UI_CONFORMANCE_VERBOSE").is_some()
}

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
            Plan::Agent => {
                for (format, ran) in agent_tier::run_case(case) {
                    match (&ran.outcome, ran.lowered) {
                        (Outcome::Skipped(_), _) => {}
                        (_, true) => tally.lowered += 1,
                        (_, false) => tally.as_written += 1,
                    }
                    // A transcript is one check per format it declares.
                    let check = match format {
                        Some(format) => format!("{check}[{format}]"),
                        None => check.clone(),
                    };
                    tally.record(&check, ran.outcome);
                }
            }
        }
    }
    (cases.len(), tally)
}

/// Agent-tier checks this crate answers differently from the reference on
/// purpose: the check, why, and what its failure has to say, so that an entry
/// cannot go on covering a case that started failing for another reason.
const AGENT_DIVERGENCES: &[(&str, &str, &str)] = &[
    (
        "agent/direct_json/response_streaming.yaml::test_stream_orphan_component_fails_v09",
        "a component the root cannot reach is a warning (ValidationReport::unreachable), not an \
         error; the block is emitted",
        "expected {\"category\":\"ValidationError\"}, got [{\"a2ui\"",
    ),
    (
        "agent/direct_json/response_streaming.yaml::test_stream_relative_path_kept_v09",
        "a relative binding path outside a list template is rejected; the reference does not \
         look at bindings",
        "is not inside a list template",
    ),
    (
        "agent/direct_json/response_streaming.yaml::test_stream_data_model_grows_v09",
        "a list template whose path holds a map, not an array, is rejected once the data model \
         arrives; the reference does not look at the data model",
        "must point at an array",
    ),
];

/// The row of the agent-tier table for one suite: how many of its cases ran
/// lowered from v1.0, how many as written, and how many were skipped under
/// each label. It is what #42 reads to know what to stop lowering.
fn agent_row(suite: &Suite, cases: usize, tally: &Tally) -> String {
    let skipped = |label: &str| -> usize {
        tally
            .skip_reasons
            .iter()
            .filter(|(reason, _)| reason.starts_with(label))
            .map(|(_, count)| count)
            .sum()
    };
    format!(
        "| `{}` | {cases} | {} | {} | {} | {} | {} |",
        suite.path,
        tally.lowered,
        tally.as_written,
        skipped(agent_tier::NEEDS_V1_0),
        skipped(agent_tier::NEEDS_V0_8),
        skipped(agent_tier::NEEDS_FORMAT),
    )
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
        Plan::Run | Plan::Agent => "executed".to_string(),
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
    let mut agent_rows = Vec::new();

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
        if suite.plan == Plan::Agent {
            agent_rows.push(agent_row(suite, cases, &tally));
            total.lowered += tally.lowered;
            total.as_written += tally.as_written;
        }

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
    println!(
        "\nAGENT TIER: {} cases ran lowered from v1.0, {} as written\n",
        total.lowered, total.as_written
    );
    println!(
        "| Suite | Cases | Lowered from v1.0 | As written | Skipped: v1.0 | Skipped: v0.8 | Skipped: another format |"
    );
    println!("|---|---:|---:|---:|---:|---:|---:|");
    for row in &agent_rows {
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
        .chain(&agent_rows)
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
    let agent_line = format!(
        "{} checks ran lowered from v1.0 and {} as written",
        total.lowered, total.as_written
    );
    assert!(
        readme.contains(&agent_line),
        "tests/conformance/README.md does not state the agent-tier total: {agent_line}"
    );
}

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
    let wire_format = matches!(
        (action, format),
        ("validate", Format::Current)
            | (
                "process_chunk" | "parse_full" | "fix_payload" | "has_parts",
                _
            )
    );
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
