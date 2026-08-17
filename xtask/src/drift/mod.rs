//! Protocol drift detection between the AG-UI TypeScript source of truth and
//! this repo's Rust event types.
//!
//! Why this exists: the Rust event types are a hand-written port of the
//! upstream Zod schemas. Nothing in the compiler links the two, so upstream can
//! add an event type and this SDK will keep building, keep passing its tests,
//! and silently not speak the protocol any more. That is exactly how the
//! previous community SDK fell ten event types behind without anyone noticing.
//!
//! The link is this check:
//!
//! * `drift-check` — offline, deterministic, the CI gate. Compares the vendored
//!   baseline in `xtask/baseline/` against the Rust source, read as text.
//! * `drift-check --upstream` — additionally asks GitHub whether the baseline
//!   itself has gone stale. Needs the network, so it is a scheduled job, never
//!   the required check.
//! * `drift-check --refresh` — re-captures the baseline. How a human accepts an
//!   upstream protocol change.

pub mod baseline;
pub mod fetch;
pub mod rust_src;
pub mod text;
pub mod upstream;

use std::path::{Path, PathBuf};

use baseline::Baseline;
use rust_src::{RustEvent, RustSurface};

/// Where the vendored baseline lives, relative to the repo root.
const BASELINE: &str = "xtask/baseline/events.json";
/// Where the Rust event types live, relative to the repo root.
const EVENT_DIR: &str = "crates/ag-ui-core/src/event";

/// Exit codes: 0 clean, 1 drift found, 2 the check itself could not run.
pub const EXIT_OK: u8 = 0;
pub const EXIT_DRIFT: u8 = 1;

#[derive(Debug, Default, Clone, Copy)]
pub struct Args {
    /// Also check whether the vendored baseline is stale (needs network).
    pub upstream: bool,
    /// Re-capture the baseline from upstream (needs network).
    pub refresh: bool,
}

/// Runs the check. `Err` means the check could not be performed at all, which
/// is a different thing from finding drift.
pub fn run(args: Args) -> Result<u8, String> {
    let root = repo_root();
    let baseline_path = root.join(BASELINE);
    let event_dir = root.join(EVENT_DIR);

    if args.refresh {
        return refresh(&baseline_path);
    }

    let baseline = Baseline::load(&baseline_path)?;
    if !event_dir.is_dir() {
        return Err(format!(
            "{} does not exist.\n\
             drift-check compares the vendored baseline against the Rust event types; \
             without them there is nothing to compare.",
            event_dir.display()
        ));
    }
    let rust = rust_src::scan(&event_dir, &root)?;
    if rust.events.is_empty() {
        return Err(format!(
            "no event types found under {}.\n\
             {} .rs files were read, but none declared a `#[serde(tag = \"type\")]` enum or a \
             `<Name>Event` payload struct.\n\
             Either the event types moved, or this scanner needs teaching about a new shape \
             (xtask/src/drift/rust_src.rs).",
            event_dir.display(),
            rust.files.len()
        ));
    }

    let report = compare(&baseline, &rust);
    print!("{}", render(&baseline, &rust, &report));

    let mut exit = if report.is_clean() {
        EXIT_OK
    } else {
        EXIT_DRIFT
    };

    if args.upstream {
        println!();
        match check_upstream(&baseline) {
            Ok(lines) => {
                let stale = !lines.is_empty();
                print!("{}", render_upstream(&baseline, &lines));
                if stale {
                    exit = EXIT_DRIFT;
                }
            }
            Err(e) => {
                println!("UPSTREAM FRESHNESS CHECK — could not run");
                println!("  {}", indent(&e, "  "));
                println!(
                    "\n  The offline result above stands; only the freshness check was skipped."
                );
            }
        }
    }

    Ok(exit)
}

/// Re-captures the vendored baseline from upstream.
fn refresh(baseline_path: &Path) -> Result<u8, String> {
    let previous = Baseline::load(baseline_path).ok();
    let fetched = fetch::events_ts()?;
    let extracted = upstream::extract(&fetched.text)?;
    let next = Baseline::from_upstream(&extracted, fetched.source);
    next.save(baseline_path)?;

    println!("Refreshed {}", baseline_path.display());
    println!(
        "  from {}@{} ({} event types, captured {})",
        next.source.repo,
        short(&next.source.commit),
        next.event_types.len(),
        next.source.fetched_at
    );

    match previous {
        None => println!("\n  New baseline — review it in full before committing."),
        Some(previous) => {
            let changes = diff_baselines(&previous, &next);
            if changes.is_empty() {
                println!(
                    "\n  No change to the event surface (previous snapshot was {}@{}).",
                    previous.source.repo,
                    short(&previous.source.commit)
                );
            } else {
                println!("\n  Changes since {}:", short(&previous.source.commit));
                for line in &changes {
                    println!("    {line}");
                }
                println!(
                    "\n  Review these, update crates/ag-ui-core/src/event to match, then commit \
                     the baseline with that change."
                );
            }
        }
    }
    let unparsed = next.events.iter().filter(|e| e.unparsed.is_some()).count();
    if unparsed > 0 {
        println!(
            "\n  {unparsed} schema(s) could not be read confidently; their fields are recorded \
             as unparsed and are reported as warnings, not failures."
        );
    }
    Ok(EXIT_OK)
}

/// Fetches upstream and returns the ways the baseline no longer matches it.
fn check_upstream(baseline: &Baseline) -> Result<Vec<String>, String> {
    let fetched = fetch::events_ts()?;
    let extracted = upstream::extract(&fetched.text)?;
    let current = Baseline::from_upstream(&extracted, fetched.source);
    Ok(diff_baselines(baseline, &current))
}

/// Human-readable differences between two snapshots of the upstream surface.
fn diff_baselines(old: &Baseline, new: &Baseline) -> Vec<String> {
    let mut out = Vec::new();
    for event in &new.event_types {
        if !old.event_types.contains(event) {
            out.push(format!("+ {event} was added upstream"));
        }
    }
    for event in &old.event_types {
        if !new.event_types.contains(event) {
            out.push(format!("- {event} was removed upstream"));
        }
    }
    for event in &new.events {
        let Some(before) = old.event(&event.event_type) else {
            continue;
        };
        if before.unparsed.is_some() || event.unparsed.is_some() {
            continue;
        }
        for field in &event.fields {
            match before.fields.iter().find(|f| f.name == field.name) {
                None => out.push(format!(
                    "~ {}.{} was added upstream ({})",
                    event.event_type,
                    field.name,
                    optionality(field.required)
                )),
                Some(was) if was.required != field.required => out.push(format!(
                    "~ {}.{} is now {} upstream (was {})",
                    event.event_type,
                    field.name,
                    optionality(field.required),
                    optionality(was.required)
                )),
                Some(_) => {}
            }
        }
        for field in &before.fields {
            if !event.fields.iter().any(|f| f.name == field.name) {
                out.push(format!(
                    "~ {}.{} was removed upstream",
                    event.event_type, field.name
                ));
            }
        }
    }
    out
}

/// Everything the offline comparison found.
#[derive(Debug, Default)]
struct Report {
    missing_in_rust: Vec<baseline::Event>,
    /// Declared in Rust, but not a member of the tagged union.
    not_in_union: Vec<RustEvent>,
    not_in_upstream: Vec<RustEvent>,
    field_diffs: Vec<FieldDiff>,
    warnings: Vec<String>,
}

impl Report {
    fn is_clean(&self) -> bool {
        self.missing_in_rust.is_empty()
            && self.not_in_union.is_empty()
            && self.not_in_upstream.is_empty()
            && self.field_diffs.is_empty()
    }
}

#[derive(Debug)]
struct FieldDiff {
    event_type: String,
    rust_type: String,
    file: String,
    /// Upstream field with no Rust counterpart.
    missing: Vec<baseline::Field>,
    /// Rust field upstream does not declare.
    extra: Vec<rust_src::RustField>,
    /// `(field, required upstream, required in Rust)`.
    optionality: Vec<(String, bool, bool)>,
}

impl FieldDiff {
    fn is_empty(&self) -> bool {
        self.missing.is_empty() && self.extra.is_empty() && self.optionality.is_empty()
    }
}

fn compare(baseline: &Baseline, rust: &RustSurface) -> Report {
    let mut report = Report::default();

    for event in &baseline.events {
        let Some(found) = rust.events.iter().find(|e| e.tag == event.event_type) else {
            report.missing_in_rust.push(event.clone());
            continue;
        };
        if let Some(reason) = &event.unparsed {
            report.warnings.push(format!(
                "{}: the upstream schema could not be read ({reason}); \
                 its fields were not compared",
                event.event_type
            ));
            continue;
        }
        let Some(fields) = &found.fields else {
            report.warnings.push(format!(
                "{}: no payload fields could be read from the Rust source; \
                 only the event type itself was compared",
                event.event_type
            ));
            continue;
        };

        let diff = FieldDiff {
            event_type: event.event_type.clone(),
            rust_type: found.rust_type.clone().unwrap_or_else(|| "?".to_string()),
            file: found.file.clone(),
            missing: event
                .fields
                .iter()
                .filter(|f| !fields.iter().any(|r| r.name == f.name))
                .cloned()
                .collect(),
            extra: fields
                .iter()
                .filter(|r| !event.fields.iter().any(|f| f.name == r.name))
                .cloned()
                .collect(),
            optionality: event
                .fields
                .iter()
                .filter_map(|f| {
                    let r = fields.iter().find(|r| r.name == f.name)?;
                    (r.required != f.required).then(|| (f.name.clone(), f.required, r.required))
                })
                .collect(),
        };
        if !diff.is_empty() {
            report.field_diffs.push(diff);
        }
    }

    // A payload type that never made it into the union cannot be sent or
    // received, so it is drift even though the type exists. Only checked when
    // the union's members were readable at all — otherwise a shape this scanner
    // does not understand would condemn every event type.
    let union_read = rust.events.iter().any(|e| e.from_enum);
    for event in &rust.events {
        if !baseline.event_types.contains(&event.tag) {
            report.not_in_upstream.push(event.clone());
        } else if union_read && event.from_struct && !event.from_enum {
            report.not_in_union.push(event.clone());
        }
    }

    report.warnings.extend(rust.notes.iter().cloned());
    report
}

fn render(baseline: &Baseline, rust: &RustSurface, report: &Report) -> String {
    let mut out = String::new();
    let src = &baseline.source;
    out.push_str("drift-check\n");
    out.push_str(&format!(
        "  baseline  {BASELINE}  ({}@{}, captured {})\n",
        src.repo,
        short(&src.commit),
        src.fetched_at
    ));
    out.push_str(&format!(
        "  upstream  {} event types\n",
        baseline.event_types.len()
    ));
    out.push_str(&format!(
        "  rust      {EVENT_DIR}  ({} files, {} event types{})\n",
        rust.files.len(),
        rust.events.len(),
        match &rust.tagged_enum {
            Some(name) => format!(", tagged enum `{name}`"),
            None => String::new(),
        }
    ));

    if !report.missing_in_rust.is_empty() {
        out.push_str(&format!(
            "\nMISSING IN RUST — {}\n",
            report.missing_in_rust.len()
        ));
        out.push_str(
            "  Upstream declares these event types and this SDK does not. A stream that\n  \
             carries one of them cannot be handled.\n\n",
        );
        for event in &report.missing_in_rust {
            out.push_str(&format!(
                "    {:<32}{}\n",
                event.event_type,
                event.schema.as_deref().unwrap_or("(no upstream schema)")
            ));
            if !event.fields.is_empty() {
                out.push_str(&format!(
                    "    {:<32}fields: {}\n",
                    "",
                    event
                        .fields
                        .iter()
                        .map(|f| format!("{}{}", f.name, if f.required { "" } else { "?" }))
                        .collect::<Vec<_>>()
                        .join(", ")
                ));
            }
        }
        out.push_str(&format!(
            "\n  Fix: add the payload type under {EVENT_DIR}/ and wire it into the event enum.\n"
        ));
    }

    if !report.not_in_union.is_empty() {
        out.push_str(&format!(
            "\nNOT IN THE EVENT UNION — {}\n",
            report.not_in_union.len()
        ));
        out.push_str(&format!(
            "  These payload types exist but are not members of `{}`, so nothing can\n  \
             serialize or deserialize them. Upstream declares them, so this is drift.\n\n",
            rust.tagged_enum.as_deref().unwrap_or("the event enum")
        ));
        for event in &report.not_in_union {
            out.push_str(&format!(
                "    {:<32}{} ({})\n",
                event.tag,
                event.rust_type.as_deref().unwrap_or("?"),
                event.file
            ));
        }
        out.push_str(&format!(
            "\n  Fix: add a variant for each to `{}`.\n",
            rust.tagged_enum.as_deref().unwrap_or("the event enum")
        ));
    }

    if !report.not_in_upstream.is_empty() {
        out.push_str(&format!(
            "\nNOT IN UPSTREAM — {}\n",
            report.not_in_upstream.len()
        ));
        out.push_str(
            "  These exist in Rust but are not in the baseline's EventType enum. Either\n  \
             upstream removed them, or the tag is misspelled.\n\n",
        );
        for event in &report.not_in_upstream {
            out.push_str(&format!(
                "    {:<32}{} ({})\n",
                event.tag,
                event.rust_type.as_deref().unwrap_or("?"),
                event.file
            ));
        }
        out.push_str(
            "\n  Fix: correct the tag, delete the type, or — if upstream moved — re-capture\n  \
             the baseline with `cargo run -p xtask -- drift-check --refresh`.\n",
        );
    }

    if !report.field_diffs.is_empty() {
        out.push_str(&format!(
            "\nFIELD MISMATCHES — {}\n",
            report.field_diffs.len()
        ));
        out.push_str(
            "  The event type exists on both sides but its payload does not match.\n  \
             `?` marks an optional field.\n\n",
        );
        for diff in &report.field_diffs {
            out.push_str(&format!(
                "    {}  ->  {} ({})\n",
                diff.event_type, diff.rust_type, diff.file
            ));
            for field in &diff.missing {
                out.push_str(&format!(
                    "        missing in Rust    {:<24}upstream: {}\n",
                    field.name,
                    optionality(field.required)
                ));
            }
            for field in &diff.extra {
                out.push_str(&format!(
                    "        not upstream       {:<24}Rust: {}\n",
                    field.name,
                    optionality(field.required)
                ));
            }
            for (name, up, rs) in &diff.optionality {
                out.push_str(&format!(
                    "        optionality        {:<24}upstream: {}, Rust: {}\n",
                    name,
                    optionality(*up),
                    optionality(*rs)
                ));
            }
        }
    }

    if !report.warnings.is_empty() {
        out.push_str(&format!(
            "\nWARNINGS — {} (not failures)\n",
            report.warnings.len()
        ));
        for warning in &report.warnings {
            out.push_str(&format!("    {warning}\n"));
        }
    }

    out.push('\n');
    if report.is_clean() {
        out.push_str(&format!(
            "OK  {} event types match the baseline{}.\n",
            baseline.event_types.len(),
            match report.warnings.len() {
                0 => String::new(),
                n => format!(", {n} warning(s)"),
            }
        ));
    } else {
        out.push_str(&format!(
            "FAILED  {} missing in Rust, {} not in the union, {} not upstream, \
             {} with field mismatches.\n",
            report.missing_in_rust.len(),
            report.not_in_union.len(),
            report.not_in_upstream.len(),
            report.field_diffs.len()
        ));
        out.push_str(
            "        The baseline is the protocol. If the baseline is what changed, re-capture\n\
             \x20       it with `--refresh` and review that diff; otherwise fix the Rust side.\n",
        );
    }
    out
}

fn render_upstream(baseline: &Baseline, changes: &[String]) -> String {
    let mut out = String::from("UPSTREAM FRESHNESS CHECK\n");
    out.push_str(&format!(
        "  baseline captured {} from {}@{}\n",
        baseline.source.fetched_at,
        baseline.source.repo,
        short(&baseline.source.commit)
    ));
    if changes.is_empty() {
        out.push_str("\nOK  The vendored baseline still matches upstream.\n");
        return out;
    }
    out.push_str(&format!(
        "\nSTALE  upstream has moved in {} way(s) since the baseline was captured:\n\n",
        changes.len()
    ));
    for change in changes {
        out.push_str(&format!("    {change}\n"));
    }
    out.push_str(
        "\n  Accept these with `cargo run -p xtask -- drift-check --refresh`, then update\n  \
         the Rust types in the same pull request.\n",
    );
    out
}

fn optionality(required: bool) -> &'static str {
    if required { "required" } else { "optional" }
}

fn short(sha: &str) -> String {
    sha.chars().take(10).collect()
}

fn indent(text: &str, prefix: &str) -> String {
    text.replace('\n', &format!("\n{prefix}"))
}

/// The repo root, from the compile-time location of this crate.
fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("xtask/ always has a parent")
        .to_path_buf()
}

#[cfg(test)]
mod tests {
    use super::*;
    use baseline::{Event, Field, Source};

    fn baseline_of(events: Vec<Event>) -> Baseline {
        Baseline {
            note: String::new(),
            format: baseline::FORMAT,
            source: Source {
                repo: "ag-ui-protocol/ag-ui".into(),
                path: "events.ts".into(),
                commit: "0123456789abcdef".into(),
                commit_date: "2026-08-01".into(),
                fetched_at: "2026-08-01".into(),
            },
            base_event_fields: vec![],
            event_types: events.iter().map(|e| e.event_type.clone()).collect(),
            events,
        }
    }

    fn event(ty: &str, fields: &[(&str, bool)]) -> Event {
        Event {
            event_type: ty.into(),
            schema: Some(format!("{ty}Schema")),
            fields: fields
                .iter()
                .map(|(name, required)| Field {
                    name: (*name).into(),
                    required: *required,
                })
                .collect(),
            unparsed: None,
        }
    }

    fn rust_event(tag: &str, fields: Option<&[(&str, bool)]>) -> RustEvent {
        RustEvent {
            tag: tag.into(),
            rust_type: Some(format!("{tag}Event")),
            file: "crates/ag-ui-core/src/event/x.rs".into(),
            fields: fields.map(|fields| {
                fields
                    .iter()
                    .map(|(name, required)| rust_src::RustField {
                        name: (*name).into(),
                        required: *required,
                    })
                    .collect()
            }),
            from_enum: false,
            from_struct: true,
        }
    }

    fn surface(events: Vec<RustEvent>) -> RustSurface {
        RustSurface {
            events,
            tagged_enum: None,
            files: vec!["crates/ag-ui-core/src/event/x.rs".into()],
            notes: vec![],
        }
    }

    #[test]
    fn identical_surfaces_are_clean() {
        let baseline = baseline_of(vec![event("RAW", &[("event", true), ("source", false)])]);
        let rust = surface(vec![rust_event(
            "RAW",
            Some(&[("event", true), ("source", false)]),
        )]);
        let report = compare(&baseline, &rust);
        assert!(report.is_clean());
        assert!(render(&baseline, &rust, &report).contains("OK  1 event types match"));
    }

    #[test]
    fn reports_each_kind_of_drift() {
        let baseline = baseline_of(vec![
            event("RAW", &[("event", true), ("source", false)]),
            event("ACTIVITY_DELTA", &[("patch", true)]),
        ]);
        let rust = surface(vec![
            rust_event("RAW", Some(&[("event", false), ("extra", true)])),
            rust_event("MADE_UP", Some(&[])),
        ]);
        let report = compare(&baseline, &rust);

        assert_eq!(report.missing_in_rust.len(), 1);
        assert_eq!(report.missing_in_rust[0].event_type, "ACTIVITY_DELTA");
        assert_eq!(report.not_in_upstream.len(), 1);
        assert_eq!(report.field_diffs.len(), 1);
        let diff = &report.field_diffs[0];
        assert_eq!(
            diff.missing.iter().map(|f| &f.name).collect::<Vec<_>>(),
            ["source"]
        );
        assert_eq!(
            diff.extra.iter().map(|f| &f.name).collect::<Vec<_>>(),
            ["extra"]
        );
        assert_eq!(diff.optionality, [("event".to_string(), true, false)]);

        let text = render(&baseline, &rust, &report);
        assert!(text.contains("MISSING IN RUST — 1"));
        assert!(text.contains("NOT IN UPSTREAM — 1"));
        assert!(text.contains("FIELD MISMATCHES — 1"));
        assert!(text.contains("FAILED"));
    }

    #[test]
    fn payload_type_outside_the_union_is_drift() {
        let baseline = baseline_of(vec![event("RAW", &[]), event("CUSTOM", &[])]);
        let mut wired = rust_event("RAW", Some(&[]));
        wired.from_enum = true;
        let stranded = rust_event("CUSTOM", Some(&[]));
        let mut rust = surface(vec![wired, stranded]);
        rust.tagged_enum = Some("Event".into());

        let report = compare(&baseline, &rust);
        assert_eq!(report.not_in_union.len(), 1);
        assert_eq!(report.not_in_union[0].tag, "CUSTOM");
        assert!(render(&baseline, &rust, &report).contains("NOT IN THE EVENT UNION — 1"));
    }

    #[test]
    fn a_union_this_scanner_cannot_read_does_not_condemn_every_event() {
        let baseline = baseline_of(vec![event("RAW", &[]), event("CUSTOM", &[])]);
        // Nothing came from the enum: the shape was not understood.
        let mut rust = surface(vec![
            rust_event("RAW", Some(&[])),
            rust_event("CUSTOM", Some(&[])),
        ]);
        rust.tagged_enum = Some("Event".into());
        assert!(compare(&baseline, &rust).is_clean());
    }

    #[test]
    fn unparsed_upstream_schema_warns_instead_of_failing() {
        let mut events = vec![event("RAW", &[("event", true)])];
        events[0].unparsed = Some("unsupported modifier `.superRefine()`".into());
        let baseline = baseline_of(events);
        let rust = surface(vec![rust_event("RAW", Some(&[("something_else", true)]))]);
        let report = compare(&baseline, &rust);
        assert!(report.is_clean());
        assert_eq!(report.warnings.len(), 1);
        assert!(render(&baseline, &rust, &report).contains("WARNINGS — 1"));
    }

    #[test]
    fn unreadable_rust_payload_warns_instead_of_failing() {
        let baseline = baseline_of(vec![event("RAW", &[("event", true)])]);
        let rust = surface(vec![rust_event("RAW", None)]);
        let report = compare(&baseline, &rust);
        assert!(report.is_clean());
        assert_eq!(report.warnings.len(), 1);
    }

    #[test]
    fn baseline_diff_names_what_moved_upstream() {
        let old = baseline_of(vec![event("RAW", &[("event", true)]), event("GONE", &[])]);
        let new = baseline_of(vec![
            event("RAW", &[("event", true), ("source", false)]),
            event("BRAND_NEW", &[]),
        ]);
        let changes = diff_baselines(&old, &new);
        assert_eq!(
            changes,
            [
                "+ BRAND_NEW was added upstream",
                "- GONE was removed upstream",
                "~ RAW.source was added upstream (optional)",
            ]
        );
    }
}
