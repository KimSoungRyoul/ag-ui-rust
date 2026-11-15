# ag-ui-rust

An independent Rust SDK for the AG-UI protocol, not an official AG-UI project. In scope: protocol
types, server event emitters, client conversations. Out of scope: agent execution, scheduling,
model loops, durable workflows.

- `crates/ag-ui`: protocol types at the root; `server`, `client`, `http`, `axum` behind features.
- `crates/ag-ui-a2ui`: A2UI, a separate protocol, usable without AG-UI.
- `ag-ui-core`, `ag-ui-server` and `ag-ui-client` on crates.io are an unrelated SDK. Never depend
  on them.

Design rationale is in `docs/DESIGN.md`.

## Checks

```sh
cargo nextest run --workspace --all-features
cargo test --doc --workspace --all-features   # nextest never runs doctests
cargo clippy --workspace --all-targets --all-features -- -D warnings
prek run --all-files                          # what CI runs
```

## Issues, branches, commits

- Work starts from an issue. Name the branch `<issue>-<type>-<slug>`, e.g. `20-fix-subagents-key`.
- Commit subjects read `[#20] fix(core): …`. After `prek install`, the hook adds the `[#20]` prefix.
- A tool-made worktree branch is named `worktree-<name>`. Rename it (`git branch -m …`), or the
  hook adds nothing.
- The prefix only links. Close the issue with `Closes #20` in the pull request.
- Commits for the upstream copy carry no prefix: `#20` there is another project's issue.
