# ag-ui-rust

A Rust SDK for the [AG-UI protocol](https://docs.ag-ui.com) — build agent backends **and** agent clients in Rust.

AG-UI standardises how an AI agent talks to a user-facing application: a POST carrying
`RunAgentInput`, answered by a stream of typed events. Official SDKs exist for TypeScript,
Python, and .NET. Rust has a community SDK, but it is client-only and has drifted from the
spec. This project fills the gap, with the server story as the priority.

## Crates

| Crate | What it is |
| --- | --- |
| `ag-ui-core` | Protocol types, all event variants, and wire encoding. `serde` + `serde_json` only. |
| `ag-ui-server` | Host an agent: `Agent` trait, typestate event emitters, automatic state deltas, protocol verification. Executor-agnostic. |
| `ag-ui-axum` | Mount an agent on an axum router. The only crate that pulls in tokio. |
| `ag-ui-client` | Consume a remote agent: transport, event application, materialised messages and state. |
| `ag-ui-a2ui` | [A2UI](https://a2ui.org) protocol types, semantic validator, and agent-side authoring toolkit. |

## Design commitments

**The `Agent` trait is the boundary.** The .NET SDK builds on `Microsoft.Extensions.AI`
because .NET has a blessed chat abstraction. Rust does not — the ecosystem is split across
`async-openai`, `rig-core`, and `genai`. So this SDK depends on no LLM crate at all. Bring
your own client; implement `Agent`.

**Executor-agnostic below the web binding.** `core`, `server`, and `client` use
`futures` primitives rather than tokio, so wasm targets and non-tokio executors keep working.
tokio enters at `ag-ui-axum` and nowhere else.

**Protocol misuse should not compile.** Event ordering (`Start` → `Content*` → `End`) is
enforced by typestate handles that borrow the run context, so interleaving two messages is a
borrow-check error. Handles emit their terminating event on `Drop`, so it cannot be forgotten.
Because Rust has no async `Drop`, the emit path is synchronous by design. What the borrow
checker cannot catch, a runtime ordering verifier catches in debug builds.

**IDs are strings.** `ThreadId`, `RunId`, and friends are newtypes over `String`, not `Uuid`.
The spec says string; real backends such as LangGraph send arbitrary strings.

## Status

Early. See `docs/` for the design rationale and the upstream analysis this is based on.

## License

MIT
