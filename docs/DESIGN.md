# Design decisions

Why this SDK looks the way it does. Written for someone about to change something here.

## Why another Rust AG-UI SDK

`sdks/community/rust` in the upstream repo landed in October 2025 and then stalled: there is
no CODEOWNERS entry for it, so nobody has merge authority. As of August 2026 four Rust PRs sit
with zero reviews, including a server crate (#972, +5,916 lines) open since January. The core
team acknowledged the situation on issue #2256.

The concrete result: `ag-ui-core 0.1.0` declares 24 event variants against a spec with 32, is
missing the entire `REASONING_*` family and both `ACTIVITY_*` events, and has no
`RunFinished.outcome` field — which means human-in-the-loop is not expressible at all. And
both published crates are client-side; there is no way to *host* an AG-UI agent in Rust.

## The source of truth is the TypeScript Zod schemas

Not the protobuf definitions. `sdks/typescript/packages/proto/src/proto/events.proto` has an
`Event` oneof with 18 arms — no reasoning, no activity, no thinking, no `tool_call_result`.
The binary transport is a lossy subset of the protocol, so it cannot serve as the port target.
There is also no JSON Schema export upstream (`zod-to-json-schema` and `toJSONSchema` appear
nowhere in the repo).

So the port is hand-written against `core/src/events.ts`, and `xtask drift-check` in CI is what
keeps it honest. Detection, not generation: it parses the upstream `EventType` enum and Zod
object keys and fails the build when they diverge from the Rust side. Full code generation would
mean writing and maintaining a Zod-to-Rust compiler, which is not worth it yet.

## Crate boundaries

Five crates, not seven. The first draft mirrored the .NET assembly split one-for-one, which is
the wrong instinct: in .NET an assembly is the deployment and versioning unit, so splitting is
cheap and natural. In Rust, **features are the primary tool** and a crate split should be
justified by dependency isolation or independent versioning.

Two crates were folded in as a result:

- `ag-ui-encoder` → `ag-ui-core::encode`. SSE framing is a few hundred lines with zero extra
  dependencies. Only protobuf is heavy, and an optional dependency already handles that.
- `ag-ui-a2ui-toolkit` → `ag-ui-a2ui`, `toolkit` feature. It is prompt strings and
  orchestration; nothing to isolate.

What stayed separate, and why: `ag-ui-axum` drags in axum/tokio/tower, `ag-ui-client` is useful
on its own, `ag-ui-a2ui` is a different protocol that can be used without AG-UI at all.

## No LLM abstraction

`AGUI.Server` in .NET is built on `Microsoft.Extensions.AI`'s `IChatClient`. That works because
.NET has one blessed chat abstraction. Rust does not: recent 90-day downloads run roughly
`async-openai` 2.3M, `rig-core` 1.3M, `genai` 113k, while `agent-framework-core` — the closest
MEAI analogue — sits near 1k.

So `trait Agent` *is* the boundary and this SDK depends on no LLM crate. A framework
integration is then just an `impl Agent for …` in its own crate.

## Executor-agnostic below the web binding

`core`, `server`, and `client` use `futures` primitives — notably `futures::channel::mpsc` for
the emit path rather than `tokio::sync::mpsc`. tokio appears only in `ag-ui-axum`. This keeps
wasm targets and non-tokio executors viable, and the CI wasm job enforces it.

## Synchronous emit, because Rust has no async Drop

The emitter API is typestate: `ctx.assistant_message()` returns a handle that borrows the run
context mutably, so starting a second overlapping message is a borrow-check error rather than a
runtime protocol violation. The handle emits its terminating event on `Drop`, so forgetting
`end()` is harmless.

That last guarantee is what forces the design. `Drop` cannot be async, so a handle cannot
`await` while emitting its terminator. The emit path is therefore synchronous end to end —
handles push into an unbounded channel and the transport layer drains it. The first draft of
this API had `msg.delta(t).await?`, copied from the TypeScript and .NET SDKs, and it simply
cannot coexist with the RAII guarantee.

## One extension point, not two

An early draft carried both a `StreamOptions` builder with `map_content` / `map_call` /
`map_result` / `map_interrupt` closures (a direct port of .NET's `AGUIStreamOptions`) *and* a
middleware chain. Two ways to do the same thing, and the closure version degrades into a pile of
`Box<dyn Fn>` in Rust. Everything composes through `StreamTransformer`; the former hooks are
provided as built-in transformers.

## IDs are strings

The spec types `threadId`/`runId`/`messageId` as strings. The existing community crate parses
them as UUIDs, which breaks immediately against LangGraph and anything else using its own id
scheme (upstream #2195, #2196). Newtypes over `String` preserve type distinction without
inventing a constraint the protocol does not have.

## Server-side protocol verification

Neither the TypeScript SDK (which verifies on the client) nor .NET (which does not verify at
all) checks event ordering on the server. Emitting `TEXT_MESSAGE_CONTENT` without a preceding
`START` is a bug that currently surfaces as a confused frontend. `ag-ui-server` runs an ordering
state machine, on by default, so it surfaces where it was caused.

## A2UI pins to v0.9

The A2UI spec is at v1.0, but every shipping toolkit — TypeScript, .NET, Python — still stamps
`v0.9`, and .NET's constants file marks these values a "cross-language wire contract" that "must
not diverge". Implementing v1.0 wire values today would mean not interoperating with any of them.
v1.0 goes behind a feature when the toolkits move.
