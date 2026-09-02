---
title: The crates
description: What the two crates are for, which features you need to serve an agent, to consume one, or to do both, and why it is two crates rather than five.
---

Two crates, and a set of features. The line they split along is one crate per *protocol*:
`ag-ui` is AG-UI, `ag-ui-a2ui` is A2UI. Inside `ag-ui`, what the protocol *is* compiles for
everyone, and what it takes to be an agent or to talk to one is a feature.

| Crate / feature | What it is |
| --- | --- |
| `ag-ui` | Protocol types, all 36 event variants, and wire encoding. `serde` and `serde_json` only. |
| ↳ `server` | Host an agent: the `Agent` trait, typestate event emitters, automatic state deltas, protocol verification. Executor-agnostic. |
| ↳ `client` | Consume a remote agent: transport, event application, materialised messages and state. |
| ↳ `http` | The `reqwest` transport for `client`. |
| ↳ `axum` | Mount an agent on an axum router. The only feature that pulls in tokio. |
| `ag-ui-a2ui` | A2UI protocol types, semantic validator, and agent-side authoring toolkit. |

## Which ones you need

**To serve an agent over HTTP:** `features = ["axum"]`. It implies `server` and `sse`, and
the protocol types you name in signatures are at the crate root either way.

**To serve an agent somewhere else** — a Lambda, a WebSocket, an in-process test —
`features = ["server"]`. `server::run(agent, input)` hands you a `Stream` of events and stops
there; serializing it is a transport's job, and `SseFormatter` at the crate root will frame
it if what you need is SSE over something other than axum.

**To consume an agent:** `features = ["http"]`. Ask for `client` instead and you get the
runtime with no transport, to implement `Transport` yourself for wasm, for a non-tokio
runtime, or for a socket that is not HTTP.

**Both, in one process:** `features = ["axum", "http"]`. That is not unusual — an agent that
calls another agent is a client and a server at once, which is why the client's trait is
`RemoteAgent` and not `Agent`: the two names would collide in a file that does both.

**A2UI:** `ag-ui-a2ui`, plus whichever of the above applies. It is the odd one out, and the
next section says why.

## Why two crates and not five

This was five crates — `ag-ui-core`, `-server`, `-client`, `-axum`, `-a2ui` — and the rule
that produced them is the same rule that collapsed them. A crate split has to be justified by
**dependency isolation** or **independent versioning**, because in Rust features are the
primary tool for both.

The split delivered neither. Feature gates isolate dependencies exactly as well:
`--no-default-features` compiles no axum, no tokio, no reqwest, and CI asserts that per
feature. And this workspace versions in lockstep — one `workspace.version`, everything
released together — so nothing was gaining an independent version either.

What is left is one crate per protocol. `ag-ui-a2ui` stays separate on the isolation
argument a feature cannot make: A2UI is a different protocol, drivable over A2A or MCP with
no AG-UI anywhere, and its users should not have to depend on a crate named `ag-ui`.

The cost is real and worth knowing: cargo unifies features across a dependency graph, so if
one crate in your build asks for `server` and another asks for `client`, both compile. That is
compile time in a mixed graph, not a runtime or correctness cost.

## The shape

```text
  ag-ui  (default)                         ag-ui-a2ui
  serde · serde_json · thiserror           jsonptr
            │                              (ag-ui optional)
  ┌─────────┼──────────┐
  │         │          │
server    client     axum
futures · futures ·  axum · tower · tokio
json-patch  json-patch     (implies server, sse)
            └ http
              reqwest
```

Three things about that picture are load-bearing.

**tokio enters with `axum` and nowhere else.** The protocol types and the `server` and
`client` runtimes use `futures` primitives — `futures::channel::mpsc` for the emit path
rather than `tokio::sync::mpsc` — so wasm targets and non-tokio executors keep working. CI
enforces it two ways: by building each feature for `wasm32-unknown-unknown`, and, because
tokio itself compiles for wasm, by asserting tokio is absent from those dependency graphs.
That assertion matters more now than it did when these were crates: cargo unifies features
across a graph, so one careless `dep:tokio` on `server` would reach every consumer that never
asked for `axum`. [Platforms and MSRV](/ag-ui-rust/reference/platforms/) has the specifics.

**The default build stays small on purpose.** With no features past `sse`, the crate has no
runtime, no I/O and no async: the types, their exact JSON representation, and the SSE framing
that carries them. That is what lets it sit under both halves without dragging anything into
either.

**`ag-ui-a2ui` does not depend on the rest.** A2UI is a separate protocol — an agent
streams JSON describing a surface, and a renderer draws it — and this crate is the agent
half of that exchange. Its `ag-ui` feature is what adds interop with `ag-ui`; turn it
off and you have a crate you can drive over A2A or MCP instead:

```rust
use ag_ui_a2ui::{catalog::Catalog, message::Component, validate::Validator};
use serde_json::json;

let catalog = Catalog::basic();
let components = vec![
    Component::new("root", "Card").with("child", json!("greeting")),
    Component::new("greeting", "Text").with("text", json!("Hello!")),
];

let report = Validator::new(&catalog).validate(&components);
assert!(report.is_valid());
```

Nothing in there mentions AG-UI. [A2UI](/ag-ui-rust/a2ui/) is the section on it.

## What was folded in earlier

The same rule had already collapsed seven crates into five, and those two folds stand.

`ag-ui-encoder` became `ag_ui::encode`. SSE framing is a few hundred lines with zero
extra dependencies, so there was nothing to isolate. The only heavy part is protobuf, and
an optional dependency already handles that.

`ag-ui-a2ui-toolkit` became `ag-ui-a2ui`'s `toolkit` feature. It is prompt strings and
orchestration — again, nothing to isolate.

The full argument, including what it costs, is in `docs/DESIGN.md` and summarised in
[Design commitments](/ag-ui-rust/design/commitments/).

## Features at a glance

| Crate | Feature | Default | What it adds |
| --- | --- | --- | --- |
| `ag-ui` | `sse` | yes | `SseFormatter` and `text/event-stream` framing. No extra dependencies. |
| `ag-ui` | `verify` | yes | `server`'s protocol ordering state machine. Off, it compiles away entirely. |
| `ag-ui` | `server` | no | Hosting an agent. `futures`, `json-patch`. |
| `ag-ui` | `client` | no | Consuming one, transport-agnostic. `futures`, `json-patch`. |
| `ag-ui` | `http` | no | The `reqwest`-backed transport. Implies `client` and `sse`. |
| `ag-ui` | `axum` | no | The axum binding. Implies `server` and `sse`; the only feature that pulls tokio. |
| `ag-ui` | `protobuf` | no | The binary transport's media type and content negotiation. No encoder — `events.proto` covers 21 of 36 event types. |
| `ag-ui` | `schemars` | no | Derives `schemars::JsonSchema` on the public types. |
| `ag-ui` | `utoipa` | no | Derives `utoipa::ToSchema` on the public types. |
| `ag-ui-a2ui` | `toolkit` | yes | Agent-side authoring: op builders, prompt assembly, the recovery loop. |
| `ag-ui-a2ui` | `ag-ui` | yes | Interop with `ag-ui`. Implies `toolkit`. |

`ag_ui::axum` has no features. [Feature flags](/ag-ui-rust/reference/features/) explains what
each one costs and when to turn it off.

## What is not here

There is no LLM crate anywhere in the tree, and that is a decision rather than an omission.
The .NET SDK builds on `Microsoft.Extensions.AI` because .NET has one blessed chat
abstraction; Rust does not, and the ecosystem is split across `async-openai`, `rig-core`
and `genai`. So `trait Agent` *is* the boundary, you bring your own model client, and a
framework integration is an `impl Agent for …` in a crate of its own. Both worked examples
talk to a real model using `reqwest` and two `serde` structs, which is the demonstration.

There is also no renderer. `ag-ui-a2ui` produces, validates and transports A2UI; drawing it
needs a widget toolkit, an event loop and a reactive data model, and that is a different
program.

## Next

- [Getting started](/ag-ui-rust/start/) — the dependency declarations, and a running agent.
- [The Agent trait](/ag-ui-rust/server/agent/) — what `ag_ui::server` asks of you.
- [Sessions](/ag-ui-rust/client/session/) — what `ag_ui::client` gives you.
- [API docs](/ag-ui-rust/api/ag_ui/index.html) — rustdoc for both crates.
