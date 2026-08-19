# ag-ui

An independent Rust SDK for the [AG-UI protocol](https://github.com/ag-ui-protocol/ag-ui) —
hosting an agent and consuming one.

AG-UI is the protocol between a user-facing application and an agent backend. A run is a
stream of events: the agent opens messages, streams text and reasoning, calls tools,
publishes state, and finishes — or pauses for human input.

> **Not an official AG-UI SDK.** This is not affiliated with or endorsed by the AG-UI
> protocol organisation. It is a separate implementation that tracks the protocol's
> TypeScript schemas, with a drift check in CI that fails the build when they move.

```toml
[dependencies]
# host an agent behind axum
ag-ui = { version = "0.1", features = ["axum"] }
# or consume one over HTTP
ag-ui = { version = "0.1", features = ["http"] }
```

## What is in the box

The crate root is the shared vocabulary — the types, their exact JSON representation, and
the SSE framing that carries them. No runtime, no I/O, no async; that part compiles for
everyone, including `wasm32-unknown-unknown`.

Everything past it is a feature, because most programs want one side of the protocol and
should not pay to compile the other.

```rust
use ag_ui::{Event, EventStreamFormatter, SseFormatter, TextMessageRole};

let formatter = SseFormatter::new();
let run = [
    Event::run_started("thread-1", "run-1"),
    Event::text_message_start("msg-1", TextMessageRole::Assistant),
    Event::text_message_content("msg-1", "Hello"),
    Event::text_message_end("msg-1"),
    Event::run_finished_success("thread-1", "run-1"),
];

let body: String = run
    .iter()
    .map(|event| formatter.encode_to_string(event).unwrap())
    .collect();

assert!(body.starts_with(r#"data: {"type":"RUN_STARTED","threadId":"thread-1""#));
```

Each runtime keeps its own `Error` and `Result` under its own module. A bare `ag_ui::Error`
is always a protocol error; `ag_ui::serve::Error` is a hosting error. Collapsing them into
the root would hide a distinction that matters at every `?`.

## Identifiers are strings

`ThreadId`, `RunId` and friends wrap `String`, not `Uuid`. The spec types them as strings
and real backends such as LangGraph send arbitrary values, so a stricter type would reject
valid traffic.

## Features

| Feature | Default | What it adds |
| --- | --- | --- |
| `sse` | yes | `SseFormatter` and `text/event-stream` framing. No extra dependencies. |
| `verify` | yes | `serve`'s ordering state machine. Off, the verifier is a zero-sized type whose checks compile away. |
| `serve` | no | Host an agent: the `Agent` trait, typestate emitters, state deltas. Executor-agnostic — no tokio. |
| `client` | no | Consume a remote agent, transport-agnostic. |
| `http` | no | Adds the reqwest-backed transport to `client`. What most consumers want; leave it off for wasm. |
| `axum` | no | Mount a hosted agent on an axum router. Implies `serve` and `sse`, and is the one feature that pulls in tokio. |
| `protobuf` | no | The binary transport's media type and a documented stub. `events.proto` covers only 18 of the 33 event types, so there is no encoder. |
| `schemars` | no | Derives `schemars::JsonSchema` on the public types. |
| `utoipa` | no | Derives `utoipa::ToSchema` on the public types. |

`verify` sits in `default` rather than being implied by `serve` so that
`default-features = false` can drop it: a feature cannot be subtracted from the set another
feature pulls in.

## Part of ag-ui-rust

| Crate | What it is |
| --- | --- |
| `ag-ui` | This crate: protocol types, wire encoding, and the server, client and axum runtimes behind features. |
| `ag-ui-a2ui` | A2UI protocol types, validator, and authoring toolkit. A different protocol, usable with no AG-UI at all, which is why it is its own crate. |

See the [repository](https://github.com/KimSoungRyoul/ag-ui-rust) for the design rationale.

## License

MIT
