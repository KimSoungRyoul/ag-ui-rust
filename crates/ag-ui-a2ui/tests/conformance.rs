//! Harness for the upstream language-agnostic A2UI conformance suite.
//!
//! The A2UI project ships its conformance tests as YAML rather than as
//! per-language test code, and asks every SDK to run them: read the YAML, feed
//! the inputs to that language's implementation, assert the outputs. The suites
//! are vendored under `tests/conformance/`; see the README there for the
//! upstream commit and what each file covers.
//!
//! # Skips are counted, not hidden
//!
//! Large parts of the suite exercise behaviour this crate does not implement —
//! JSON Schema validation, the v0.8 wire format, the streaming parser, renderer
//! accessibility. Those cases are **skipped with a reason and counted**, and the
//! counts are printed by [`conformance_suite`]. Nothing is silently ignored, and
//! a case is never counted as passing unless this crate actually produced the
//! expected outcome.
//!
//! Run with `cargo test -p ag-ui-a2ui --no-default-features --features toolkit
//! -- --nocapture` to see the report.

#![cfg(feature = "toolkit")]

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use ag_ui_a2ui::catalog::Catalog;
use ag_ui_a2ui::toolkit::parser::{has_a2ui_parts, parse_and_fix, parse_response};
use ag_ui_a2ui::validate::{ErrorCode, ValidateOptions, ValidationReport, Validator};
use serde_json::Value;

/// Upstream commit the vendored YAML was taken from.
const UPSTREAM_COMMIT: &str = "44a420b67957fafc0b02d55a153fdaf72e32ffb5";

/// Every vendored suite, whether or not this crate can execute any of it.
const SUITES: [&str; 6] = [
    "core/validator.yaml",
    "core/catalog.yaml",
    "core/accessibility.yaml",
    "agent/parser.yaml",
    "agent/inference_format.yaml",
    "agent/streaming_parser.yaml",
];

fn conformance_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/conformance")
}

/// The outcome of one assertion within a case.
#[derive(Debug)]
enum Outcome {
    Passed,
    Skipped(String),
    Failed(String),
}

#[derive(Debug, Default)]
struct Tally {
    passed: usize,
    failed: usize,
    skipped: usize,
    skip_reasons: BTreeMap<String, usize>,
    failures: Vec<String>,
}

/// Set `A2UI_CONFORMANCE_VERBOSE=1` to list every executed check by name.
fn verbose() -> bool {
    std::env::var_os("A2UI_CONFORMANCE_VERBOSE").is_some()
}

impl Tally {
    fn record(&mut self, case: &str, outcome: Outcome) {
        if verbose() {
            println!("    {outcome:?} {case}");
        }
        match outcome {
            Outcome::Passed => self.passed += 1,
            Outcome::Skipped(reason) => {
                self.skipped += 1;
                *self.skip_reasons.entry(reason).or_default() += 1;
            }
            Outcome::Failed(detail) => {
                self.failed += 1;
                self.failures.push(format!("{case}: {detail}"));
            }
        }
    }
}

#[test]
fn conformance_suite() {
    let mut total = Tally::default();
    println!("\n=== A2UI conformance (upstream {UPSTREAM_COMMIT}) ===");

    for suite in SUITES {
        let path = conformance_dir().join(suite);
        let text = std::fs::read_to_string(&path)
            .unwrap_or_else(|e| panic!("cannot read vendored suite {}: {e}", path.display()));
        let cases: Vec<Value> = serde_norway::from_str(&text)
            .unwrap_or_else(|e| panic!("cannot parse vendored suite {suite}: {e}"));

        let mut tally = Tally::default();
        for case in &cases {
            run_case(suite, case, &mut tally);
        }

        println!(
            "{:<32} cases {:>3}   checks: {} passed, {} skipped, {} failed",
            suite,
            cases.len(),
            tally.passed,
            tally.skipped,
            tally.failed
        );
        for (reason, count) in &tally.skip_reasons {
            println!("    skipped x{count:<3} {reason}");
        }
        for failure in &tally.failures {
            println!("    FAILED  {failure}");
        }

        total.passed += tally.passed;
        total.failed += tally.failed;
        total.skipped += tally.skipped;
        for (reason, count) in tally.skip_reasons {
            *total.skip_reasons.entry(reason).or_default() += count;
        }
        total.failures.extend(tally.failures);
    }

    println!(
        "\nTOTAL: {} passed, {} skipped, {} failed\n",
        total.passed, total.skipped, total.failed
    );

    assert!(
        total.failed == 0,
        "{} conformance check(s) failed:\n{}",
        total.failed,
        total.failures.join("\n")
    );
    // Guards against a refactor that quietly turns every case into a skip.
    assert!(
        total.passed >= 30,
        "expected at least 30 executed conformance checks, got {}",
        total.passed
    );
}

fn run_case(suite: &str, case: &Value, tally: &mut Tally) {
    let name = case
        .get("name")
        .and_then(Value::as_str)
        .unwrap_or("<unnamed>");
    let action = case.get("action").and_then(Value::as_str).unwrap_or("");
    let case_id = format!("{suite}::{name}");

    match action {
        "validate" => run_validate_case(&case_id, case, tally),
        "parse_full" => tally.record(&case_id, run_parse_full(case)),
        "fix_payload" => tally.record(&case_id, run_fix_payload(case)),
        "has_parts" => tally.record(&case_id, run_has_parts(case)),
        other => tally.record(&case_id, Outcome::Skipped(unsupported_action(other))),
    }
}

/// Why an action is out of scope for an agent-side, non-rendering crate.
fn unsupported_action(action: &str) -> String {
    let reason = match action {
        "process_chunk" => "streaming parser (incremental chunk buffering) not implemented",
        "prune" | "render" | "load" | "remove_strict_validation" | "verify_cuttable_keys" => {
            "catalog JSON Schema pruning/rendering/loading not implemented"
        }
        "accessibility_check" => "renderer-side accessibility tree; this crate does not render",
        "select_catalog" | "load_catalog" | "generate_prompt" => {
            "catalog negotiation and upstream prompt wording not implemented"
        }
        _ => "action not implemented",
    };
    format!("action '{action}': {reason}")
}

// --- validate -------------------------------------------------------------

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

    let catalog = match load_catalog_schema(catalog_config.and_then(|c| c.get("catalog_schema"))) {
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
                    step.get("expect_error")
                        .or_else(|| case.get("expect_error")),
                );
                tally.record(&step_id, outcome);
            }
        }
        None => {
            let outcome =
                run_validate_step(&catalog, case.get("payload"), case.get("expect_error"));
            tally.record(case_id, outcome);
        }
    }
}

/// Resolves a case's `catalog_schema`, which is either inline or a path
/// relative to the vendored `conformance/` directory.
fn load_catalog_schema(schema: Option<&Value>) -> Result<Option<Value>, String> {
    match schema {
        None | Some(Value::Null) => Ok(None),
        Some(Value::Object(_)) => Ok(schema.cloned()),
        Some(Value::String(relative)) => {
            let path = conformance_dir().join(relative);
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
    if let Expectation::Unsupported(reason) = expectation {
        return Outcome::Skipped(reason);
    }

    let messages: Vec<Value> = match payload {
        Value::Array(items) => items.clone(),
        other => vec![other.clone()],
    };

    let has_create = messages.iter().any(|m| m.get("createSurface").is_some());
    let components: Vec<Value> = messages
        .iter()
        .filter_map(|m| m.get("updateComponents"))
        .filter_map(|u| u.get("components"))
        .filter_map(Value::as_array)
        .flatten()
        .cloned()
        .collect();

    if components.is_empty() {
        return Outcome::Skipped(
            "payload carries no components (envelope-only JSON Schema case)".to_string(),
        );
    }

    // Upstream's structural validator checks references, roots and cycles; type
    // and required-property checking is JSON Schema's job there, and data
    // bindings are not checked at all. Matching that scope keeps the comparison
    // honest rather than failing cases on checks upstream never ran.
    let options = ValidateOptions {
        require_root: has_create,
        allow_dangling_children: !has_create,
        check_component_types: false,
        check_required_props: false,
        check_bindings: false,
        ..ValidateOptions::full_surface()
    };
    let report = Validator::with_options(catalog, options).validate_json(&components, None);

    match expectation {
        Expectation::Unsupported(reason) => Outcome::Skipped(reason),
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

/// What a conformance expectation means for this crate.
enum Expectation {
    /// The payload should validate cleanly.
    Valid,
    /// The payload should produce this error code.
    Code(ErrorCode),
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

    // A `details` list pins exact JSON Schema error codes (missing_field,
    // type_mismatch, ...) produced by envelope validation.
    if expect_error.get("details").is_some() {
        return Expectation::Unsupported(
            "expects JSON Schema envelope error details (no JSON Schema validator here)"
                .to_string(),
        );
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
        Expectation::Unsupported(
            "expects a recursion/nesting depth limit (not implemented)".to_string(),
        )
    } else if message.contains("Invalid path syntax") {
        Expectation::Unsupported(
            "expects JSON Pointer syntax validation (not implemented)".to_string(),
        )
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

fn truncate(text: &str) -> String {
    if text.chars().count() <= 60 {
        return text.to_string();
    }
    text.chars().take(57).collect::<String>() + "..."
}

// --- parser ---------------------------------------------------------------

fn run_parse_full(case: &Value) -> Outcome {
    let input = case
        .get("input")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let result = parse_response(input);

    if let Some(expect_error) = case.get("expect_error") {
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
        let path = conformance_dir().join(suite);
        let text = std::fs::read_to_string(&path)
            .unwrap_or_else(|e| panic!("cannot read {}: {e}", path.display()));
        let cases: Vec<Value> = serde_norway::from_str(&text)
            .unwrap_or_else(|e| panic!("{suite} is not a list of cases: {e}"));
        assert!(!cases.is_empty(), "{suite} is empty");
        for case in &cases {
            assert!(case.get("name").is_some(), "{suite} has an unnamed case");
            assert!(
                case.get("action").is_some(),
                "{suite} case {:?} has no action",
                case.get("name")
            );
        }
    }
}
