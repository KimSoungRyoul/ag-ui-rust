//! Repo automation entry point.
//!
//! Run with `cargo run -p xtask -- <subcommand>`.

mod corpus;
mod docs_check;
mod drift;
mod upstream_copy;

use std::process::ExitCode;

/// The check could not be performed — distinct from having found drift.
const EXIT_ERROR: u8 = 2;

const HELP: &str = "\
xtask — repo automation for ag-ui-rust

USAGE
    cargo run -p xtask -- <subcommand> [options]

SUBCOMMANDS
    drift-check            Compare the Rust protocol types — the event union and
                           every other $defs shape: capabilities, messages,
                           content parts, outcomes, tools, patches — against the
                           vendored snapshot of the AG-UI 1.0 schema.
                           Offline and deterministic.
    docs-check             Check the skills, the plugin manifests and the
                           skills' sources.md against the repository: the
                           workspace version, the README's dependency blocks,
                           paths that exist, no merged crate names in e2e/ and
                           examples/. Offline; exits 1 on a stale statement.
    upstream-sync          Compare the upstream copy of crates/ag-ui (PR 2778 on
                           ag-ui-protocol/ag-ui) with the sync point recorded
                           in UPSTREAM.md, and list the local commits since it.
                           Needs the network.
    corpus-check           Compare the vendored AG-UI 1.0 conformance corpus
                           under crates/ag-ui/tests/fixtures/ag-ui-1.0 with the
                           upstream commit its SOURCE.json records, and say
                           whether upstream has moved since. Needs the network.

DRIFT-CHECK OPTIONS
    --local                Also compare the vendored schema at
                           crates/ag-ui/src/protocol/schema-1.0.json with the
                           reviewed baseline. Offline; this is the CI gate.
    --upstream             Additionally fetch upstream and report whether the
                           vendored baseline itself has gone stale. Needs the
                           network, so keep it out of the required CI job.
    --draft                Additionally fetch upstream's spec/draft/schema.json
                           and report what it adds to 1.0. Informational: it
                           never changes the exit code. Needs the network.
    --refresh              Re-fetch upstream and rewrite the vendored baseline,
                           recording the upstream commit and the fetch date.
                           This is how a human accepts an upstream change.

UPSTREAM-SYNC OPTIONS
    --publish-guard <ver>  Offline: exit 1 if <ver> is the version UPSTREAM.md
                           says the upstream copy reserves. publish.yml runs it.

EXIT CODES
    0  no drift
    1  drift found (or, with --upstream, the baseline is stale; for
       upstream-sync, the copy moved or the version is reserved; for
       corpus-check, the copy differs from its record or upstream moved)
    2  the check could not run
";

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match dispatch(&args) {
        Ok(code) => ExitCode::from(code),
        Err(message) => {
            eprintln!("xtask: {message}");
            ExitCode::from(EXIT_ERROR)
        }
    }
}

fn dispatch(args: &[String]) -> Result<u8, String> {
    let Some((subcommand, options)) = args.split_first() else {
        print!("{HELP}");
        return Ok(EXIT_ERROR);
    };

    match subcommand.as_str() {
        "-h" | "--help" | "help" => {
            print!("{HELP}");
            Ok(drift::EXIT_OK)
        }
        "drift-check" => drift::run(parse_drift_args(options)?),
        "docs-check" => docs_check::run(options),
        "upstream-sync" => upstream_copy::run(parse_sync_args(options)?),
        "corpus-check" if options.is_empty() => corpus::run(),
        "corpus-check" => Err("corpus-check takes no options.".to_owned()),
        other => Err(format!(
            "unknown subcommand `{other}`.\nRun `cargo run -p xtask -- --help` for the list."
        )),
    }
}

fn parse_sync_args(options: &[String]) -> Result<upstream_copy::Args, String> {
    match options {
        [] => Ok(upstream_copy::Args::default()),
        [flag, version] if flag == "--publish-guard" && !version.starts_with('-') => {
            Ok(upstream_copy::Args {
                publish_guard: Some(version.clone()),
            })
        }
        _ => Err(format!(
            "unexpected options `{}` for upstream-sync.\n\
             It takes none, or `--publish-guard <version>`.",
            options.join(" ")
        )),
    }
}

fn parse_drift_args(options: &[String]) -> Result<drift::Args, String> {
    let mut parsed = drift::Args::default();
    for option in options {
        match option.as_str() {
            "--local" => parsed.local = true,
            "--upstream" => parsed.upstream = true,
            "--draft" => parsed.draft = true,
            "--refresh" => parsed.refresh = true,
            other => {
                return Err(format!(
                    "unknown option `{other}` for drift-check.\n\
                     Valid options are --local, --upstream, --draft and --refresh."
                ));
            }
        }
    }
    if parsed.local && (parsed.refresh || parsed.upstream || parsed.draft) {
        return Err(
            "--local is the offline gate and cannot be combined with --upstream, --draft or \
             --refresh"
                .to_owned(),
        );
    }
    if parsed.refresh && parsed.draft {
        return Err(
            "--refresh rewrites the 1.0 baseline and reports nothing else; run \
             `drift-check --draft` on its own."
                .to_owned(),
        );
    }
    if parsed.refresh && parsed.upstream {
        return Err("--refresh already re-reads upstream; drop --upstream.\n\
             Run `drift-check` on its own afterwards to compare the Rust types against the \
             new baseline."
            .to_string());
    }
    Ok(parsed)
}
