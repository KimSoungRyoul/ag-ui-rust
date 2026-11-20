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
commit = "26c174d15f99cb8d3583288aebb46e6ecad7f8e7"
tree = "6b9813f4d4e0ea88571b8d57dcf7718236c9cb82"
root_tree = "8e0b0ff3bddf30308b30d32b2be2e2ff2c98280e"
# The local commit `tree` was compared with. Its `crates/ag-ui/src` was
# byte-identical to the copy's.
local_commit = "77061a66aba60e9d387aac77b65dcf2f87f7d03c"
# Both workspaces declare 1.0.0. A reservation is only needed
# when upstream plans to publish a future version first.
declared_version = "1.0.0"
reserved_version = ""
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

The library source in `crates/ag-ui/src` and its public Rust API match the PR
at the recorded sync point. The standalone workspace carries the A2UI companion
crate, application examples, website and skills. The monorepo includes the
existing `ag-ui-core` and `ag-ui-client` community crates and its migration
and provenance documents.

The conformance tests use the standalone vendored corpus or the monorepo's
`spec/1.0/` according to where they run. Workspace manifests, package
metadata, lockfiles, drift tooling and CI use their respective repository's
paths. The monorepo's interoperability checks cover AG-UI only.

This repository publishes 1.0.0. The release includes the state, stream, and
conversation-merge fixes from [standalone PR #108](https://github.com/KimSoungRyoul/ag-ui-rust/pull/108).
The upstream PR at the recorded sync point predates those fixes. A future
monorepo publication needs an unpublished crate version and an upstream
maintainer's approval.
