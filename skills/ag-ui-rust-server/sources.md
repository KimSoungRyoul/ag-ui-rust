# Sources

What this skill was written from. A link goes stale silently; a path does not — when one of
these files changes, the section it feeds is the one to re-read.

The `.md` pages listed here are the documentation site's, and their Rust blocks are already
compiled by `e2e/src/website.rs`. This skill's own blocks are compiled the same way, by
`e2e/src/skills.rs`, so a snippet that drifts is a red build rather than a bad answer.

## SKILL.md

- `website/src/content/docs/start/index.md` — the dependency declarations, and that the
  crates are not on crates.io
- `website/src/content/docs/start/crates.md` — which crate does what
- `website/src/content/docs/server/agent.md` — the trait, `RunContext`, `RunOutcome`, steps
- `website/src/content/docs/server/text.md` — `say`, `assistant_message`, the RAII handles,
  the synchronous emit path, the `compile_fail` guarantee
- `website/src/content/docs/server/tools.md` — the two endings of a call, streamed args,
  capability-list semantics, parallel calls
- `website/src/content/docs/server/state.md` — `update_state` and the snapshot/delta choice
- `website/src/content/docs/server/axum.md` — `route_agui`, `AgentEndpoint`, the status codes
- `website/src/content/docs/server/subagents.md` — the subagent scope, attribution by the
  sink, agents as tools, suspension, concurrency by hand, `SubagentVisibility`
- `crates/ag-ui/src/server/agent.rs`, `crates/ag-ui/src/server/context.rs`,
  `crates/ag-ui/src/server/emit/mod.rs`, `crates/ag-ui/src/server/emit/subagent.rs`,
  `crates/ag-ui/src/server/state.rs`, `crates/ag-ui/src/server/transform.rs`
- `crates/ag-ui/tests/server_subagents.rs`, `crates/ag-ui/tests/server_transformers.rs` —
  what a scope emits and what each visibility mode strips, pinned
- `crates/ag-ui/src/axum/router.rs`

## references/state-and-interrupts.md

- `website/src/content/docs/server/interrupts.md` — the round trip, `Interrupt`'s fields,
  the multi-interrupt failure mode
- `website/src/content/docs/server/errors.md` — the error variants and codes, the eight
  verifier rules, cancellation
- `website/src/content/docs/server/subagents.md` — an interrupt raised inside a subagent,
  and the continuation on resume
- `website/src/content/docs/server/state.md`
- `crates/ag-ui/src/server/verify.rs`, `crates/ag-ui/src/server/error.rs`,
  `crates/ag-ui/src/server/cancel.rs`
- `e2e/tests/human_in_the_loop.rs` — the multi-interrupt case, pinned
