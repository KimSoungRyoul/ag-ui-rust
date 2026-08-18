---
title: The crates
description: What each of the five crates is for, and which of them you need to serve an agent, to consume one, or to do both.
---

The workspace is five crates. They split along one line: what the protocol *is*,
what it takes to be an agent, what it takes to talk to one, and the two bindings that only
some people need.

| Crate | What it is |
| --- | --- |
| `ag-ui-core` | Protocol types, all 33 event variants, and wire encoding. `serde` and `serde_json` only. |
| `ag-ui-server` | Host an agent: the `Agent` trait, typestate event emitters, automatic state deltas, protocol verification. Executor-agnostic. |
| `ag-ui-axum` | Mount an agent on an axum router. The only crate that pulls in tokio. |
| `ag-ui-client` | Consume a remote agent: transport, event application, materialised messages and state. |
| `ag-ui-a2ui` | A2UI protocol types, semantic validator, and agent-side authoring toolkit. |

## Which ones you need

**To serve an agent over HTTP:** `ag-ui-server` and `ag-ui-axum`, with `ag-ui-core` for the
types you name in signatures. `ag-ui-axum` re-exports nothing, so you depend on all three.

**To serve an agent somewhere else** — a Lambda, a WebSocket, an in-process test —
`ag-ui-server` alone. `run(agent, input)` hands you a `Stream` of events and stops there;
serializing it is a transport's job, and `ag-ui-core`'s `SseFormatter` will frame it if
what you need is SSE over something other than axum.

**To consume an agent:** `ag-ui-client` and `ag-ui-core`. The default `http` feature brings
a `reqwest` transport; turn it off and implement `Transport` yourself for wasm, for a
non-tokio runtime, or for a socket that is not HTTP.

**Both, in one process:** all four. That is not unusual — an agent that calls another agent
is a client and a server at once, which is why the client's trait is `RemoteAgent` and not
`Agent`: the two names would collide in a file that does both.

**A2UI:** `ag-ui-a2ui`, plus whichever of the above applies. It is the odd one out, and the
next section says why.

## The shape

```text
                      ag-ui-core
                serde · serde_json · thiserror
                            │
             ┌──────────────┼──────────────┐
             │              │              │
      ag-ui-server    ag-ui-client   ag-ui-a2ui
       futures ·       futures ·      jsonptr
      json-patch      json-patch ·   (core optional)
             │        reqwest (opt)
             │
       ag-ui-axum
    axum · tower · tokio
```

Three things about that picture are load-bearing.

**tokio enters at `ag-ui-axum` and nowhere else.** `core`, `server` and `client` use
`futures` primitives — `futures::channel::mpsc` for the emit path rather than
`tokio::sync::mpsc` — so wasm targets and non-tokio executors keep working. CI enforces it
two ways: by building those crates for `wasm32-unknown-unknown`, and, because tokio itself
compiles for wasm, by asserting tokio is absent from their dependency graphs.
[Platforms and MSRV](/ag-ui-rust/reference/platforms/) has the specifics.

**`ag-ui-core` stays small on purpose.** It has no runtime, no I/O and no async: the types,
their exact JSON representation, and the SSE framing that carries them. That is what lets
it sit under both halves without dragging anything into either.

**`ag-ui-a2ui` does not depend on the rest.** A2UI is a separate protocol — an agent
streams JSON describing a surface, and a renderer draws it — and this crate is the agent
half of that exchange. Its `ag-ui` feature is what adds interop with `ag-ui-core`; turn it
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

## Why five, and not seven

The first draft mirrored the .NET SDK's assembly split one-for-one, which is the wrong
instinct. In .NET an assembly is the deployment and versioning unit, so splitting is cheap
and natural. In Rust, **features are the primary tool**, and a crate split has to be
justified by something features cannot do: dependency isolation, or independent versioning.

Two crates failed that test and were folded in.

`ag-ui-encoder` became `ag-ui-core::encode`. SSE framing is a few hundred lines with zero
extra dependencies, so there was nothing to isolate. The only heavy part is protobuf, and
an optional dependency already handles that.

`ag-ui-a2ui-toolkit` became `ag-ui-a2ui`'s `toolkit` feature. It is prompt strings and
orchestration — again, nothing to isolate.

The three that stayed separate each pass the test for a different reason. `ag-ui-axum`
drags in axum, tower and tokio, and that is exactly the dependency isolation the rule is
about. `ag-ui-client` is useful entirely on its own — a frontend has no reason to compile
the server. And `ag-ui-a2ui` is a different protocol, usable without AG-UI at all.

The full argument, including what it costs, is in `docs/DESIGN.md` and summarised in
[Design commitments](/ag-ui-rust/design/commitments/).

## Features at a glance

| Crate | Feature | Default | What it adds |
| --- | --- | --- | --- |
| `ag-ui-core` | `sse` | yes | `SseFormatter` and `text/event-stream` framing. No extra dependencies. |
| `ag-ui-core` | `protobuf` | no | The binary transport's media type and content negotiation. No encoder — `events.proto` covers 18 of 33 event types. |
| `ag-ui-core` | `schemars` | no | Derives `schemars::JsonSchema` on the public types. |
| `ag-ui-core` | `utoipa` | no | Derives `utoipa::ToSchema` on the public types. |
| `ag-ui-server` | `verify` | yes | The protocol ordering state machine. Off, it compiles away entirely. |
| `ag-ui-client` | `http` | yes | The `reqwest`-backed HTTP transport. Off, the crate builds for wasm. |
| `ag-ui-a2ui` | `toolkit` | yes | Agent-side authoring: op builders, prompt assembly, the recovery loop. |
| `ag-ui-a2ui` | `ag-ui` | yes | Interop with `ag-ui-core`. Implies `toolkit`. |

`ag-ui-axum` has no features. [Feature flags](/ag-ui-rust/reference/features/) explains what
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
- [The Agent trait](/ag-ui-rust/server/agent/) — what `ag-ui-server` asks of you.
- [Sessions](/ag-ui-rust/client/session/) — what `ag-ui-client` gives you.
- [API docs](/ag-ui-rust/api/ag_ui_core/index.html) — rustdoc for all five crates.
