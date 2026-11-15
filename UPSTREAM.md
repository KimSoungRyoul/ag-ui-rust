# The upstream copy

The `ag-ui` crate here is also proposed upstream as the community Rust SDK:
`sdks/community/rust` in [ag-ui-protocol/ag-ui](https://github.com/ag-ui-protocol/ag-ui),
through [upstream PR #2778](https://github.com/ag-ui-protocol/ag-ui/pull/2778), opened from
the branch `feat/rust-sdk-migration` of
[KimSoungRyoul/ag-ui](https://github.com/KimSoungRyoul/ag-ui). The two copies are kept in
step by hand. This file records where they last agreed and what is meant to differ, and

```sh
cargo run --locked -p xtask -- upstream-sync
```

checks the record against the pull request's head. It needs the network, so it runs in the
scheduled `upstream freshness` CI job rather than on every pull request.

## Sync point

```toml
# Read by `xtask upstream-sync` and `xtask upstream-sync --publish-guard`.
# Update it in the pull request that ports a change across, or that records
# a new intended difference below.
repo = "ag-ui-protocol/ag-ui"
pull = 2778
head = "KimSoungRyoul/ag-ui:feat/rust-sdk-migration"
# The copy's workspace inside `repo`, and the directory compared, relative to
# the workspace root on each side.
root = "sdks/community/rust"
path = "crates/ag-ui/src"
# The pull request head last reviewed, the git tree of `root/path` there, and
# the git tree of `root` there.
commit = "564be451743105db9c46ef775dc90bdc69f30e2f"
tree = "53421a174bf9298fc72cff9d629a30a71fe3f80a"
root_tree = "8f7dfab5985cf0a44b764bf2288c77233a94723b"
# The local commit `tree` was compared with. Its `crates/ag-ui/src` was
# byte-identical to the copy's.
local_commit = "520a724cccaedeaa89d7ca37d69307f9e9f36401"
# The `ag-ui` version the copy declares for its own first release. publish.yml
# refuses to publish it from here. Empty when nothing is reserved.
reserved_version = "0.5.0"
```

Trees, not commits, are what is compared. The pull request gets rebased, and a rebase that
leaves the copy alone moves the head commit and neither tree.

## What the check reports

- **Moved** — the copy at the head is not what is recorded: either tree differs, the copy
  declares another version, or the pull request was merged, closed or reopened from another
  branch. Somebody has to read what changed upstream, port it here or record it below, and
  update the sync point. This fails the job.
- **Ahead** — commits here since `local_commit` that changed `crates/ag-ui/src`. Fixes land
  here first, so this is expected, and listed so that none is forgotten: each one labelled
  `upstream-pr-2778` still has to reach the pull request. This never fails.

## Intended differences

These differ on purpose. Outside `crates/ag-ui/src`, nothing compares this repository with
the copy at all (the check only notices the copy itself moving), so a test or manifest change
that belongs with a source fix has to travel with it.

- The tests that read the 1.0 conformance corpus read the copy vendored under
  `crates/ag-ui/tests/fixtures/ag-ui-1.0/`, whose `SOURCE.json` records the upstream commit
  and which `xtask corpus-check` compares with it; upstream reads the monorepo's own
  `spec/1.0/` instead. `conformance_1_0.rs` runs every stream and `fixtures_1_0.rs` every
  schema fixture as a table, and exist only here, with `tests/fixtures/local/`.
  `client_conformance_1_0.rs`, `client_thread_1_0.rs`, `tool_content_1_0.rs`,
  `optional_null_input.rs` and `upstream_payloads.rs` read single files from it.
- `crates/ag-ui/Cargo.toml` — the description. The `yoke-derive` pin that also differed
  went in #29.
- `crates/ag-ui/README.md` — install instructions for crates.io and this repository's
  guides; upstream's points at path dependencies and its migration proposal.
- `xtask/` — this repository's drift check, baseline, `upstream-sync` and `corpus-check`.
  Upstream carries an earlier fork of the drift check with a local-source protocol check.
- `e2e/` — here it also covers A2UI, the website, the skills and live models, and
  `e2e/tests/json_map_order.rs` checks A2UI data-model updates too. Upstream carries the
  AG-UI-only subset.
- The workspace manifests, `Cargo.lock` and `docs/`. So is everything only one side has:
  upstream's `crates/ag-ui-core`, `crates/ag-ui-client`, `interop/`, `MIGRATION.md` and
  `PROVENANCE.md`; this repository's `crates/ag-ui-a2ui`, `examples/`, `skills/` and
  `website/`.
