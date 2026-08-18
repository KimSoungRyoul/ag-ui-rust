# Sources

What this skill was written from. A link goes stale silently; a path does not — when one of
these files changes, the section it feeds is the one to re-read.

The `.md` pages listed here are the documentation site's, and their Rust blocks are already
compiled by `e2e/src/website.rs`. This skill's own blocks are compiled the same way, by
`e2e/src/skills.rs`, so a snippet that drifts is a red build rather than a bad answer.

## SKILL.md

- `website/src/content/docs/start/index.md` — the dependency declarations, and that the
  crates are not on crates.io
- `website/src/content/docs/client/session.md` — `Session`, the builder, the constructor-side
  transport bound, typed state, answering a pause, why there is no stop
- `website/src/content/docs/client/updates.md` — every `Update` variant, the three ways a run
  ends, why `RunEnd` is exhaustive and `Update` is not, `Success` alongside errors
- `website/src/content/docs/client/transports.md` — the trait, `HttpTransport`'s two timeouts,
  `ReplayTransport`, `SseDecoder`, turning `http` off
- `website/src/content/docs/client/rendering.md` — arrival order
- `crates/ag-ui-client/src/session.rs`, `crates/ag-ui-client/src/interrupts.rs`,
  `crates/ag-ui-client/src/transport/`, `crates/ag-ui-client/src/error.rs`

## references/rendering.md

- `website/src/content/docs/client/rendering.md`
- `crates/ag-ui-client/src/apply.rs` — `MessageChangeKind` and what is materialised
- `crates/ag-ui-client/src/chunks.rs` — the `*_CHUNK` normalizer
- `examples/board-watch/tests/client.rs` — both renderings, pinned
