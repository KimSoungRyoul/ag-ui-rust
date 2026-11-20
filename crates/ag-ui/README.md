# ag-ui

A Rust SDK for the [AG-UI protocol](https://github.com/ag-ui-protocol/ag-ui) — hosting an
agent and consuming one.

AG-UI is the protocol between a user-facing application and an agent backend. A run is a
stream of events: the agent opens messages, streams text and reasoning, calls tools,
publishes state, reports subagent activity, and finishes — or pauses for human input.

This independent SDK provides protocol types, server emitters and client conversations.
It is not affiliated with or endorsed by the AG-UI protocol organisation. The
AG-UI 1.0 schema and event baseline are checked in CI.

See the [1.0.0 guide](https://kimsoungryoul.github.io/ag-ui-rust/v1.0.0/start/)
for the Rust API and the [migration notes](https://github.com/KimSoungRyoul/ag-ui-rust/blob/v1.0.0/docs/migration-0.5.md#changes-in-100)
for changes since 0.5.0-alpha.3.

To host an agent behind axum:

```toml
[dependencies]
ag-ui = { version = "1.0.0", features = ["axum"] }
```

To consume an agent over HTTP:

```toml
[dependencies]
ag-ui = { version = "1.0.0", features = ["http"] }
```

For local source development, clone the repository and use
`ag-ui = { path = "../ag-ui-rust/crates/ag-ui", features = ["http"] }`
to test that checkout (use `axum` for the server).

## What is in the box

The crate root is the shared vocabulary — the types, their exact JSON representation, and
optional SSE framing that carries them. No runtime, no I/O, no async; that part compiles for
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
is always a protocol error; `ag_ui::server::Error` is a hosting error. Collapsing them into
the root would hide a distinction that matters at every `?`.

## Identifiers are strings

`ThreadId`, `RunId` and friends wrap `String`, not `Uuid`. The spec types them as strings
and real backends such as LangGraph send arbitrary values, so a stricter type would reject
valid traffic.

## Features

| Feature | Default | What it adds |
| --- | --- | --- |
| `sse` | yes | `SseFormatter` and `text/event-stream` framing. No extra dependencies. |
| `verify` | yes | `server`'s ordering state machine. Off, the internal ordering checks compile away. The public `EventVerifier` requires both `server` and `verify`. |
| `server` | no | Host an agent: the `Agent` trait, typestate emitters, state deltas. Executor-agnostic — no tokio. |
| `client` | no | Consume a remote agent, transport-agnostic. |
| `http` | no | Adds the reqwest-backed transport to `client`. What most consumers want; leave it off for wasm. |
| `axum` | no | Mount a hosted agent on an axum router. Implies `server` and `sse`, and is the one feature that pulls in tokio. |
| `protobuf` | no | A documented stub that names the binary media type and encodes nothing. Upstream's 1.0 `events.proto` covers all 31 1.0 event types, but the stock TypeScript client asks for `text/event-stream`; `encode::protobuf` explains. |
| `schemars` | no | Derives `schemars::JsonSchema` on the public types. |
| `utoipa` | no | Derives `utoipa::ToSchema` on the public types. |

`verify` sits in `default` rather than being implied by `server` so that
`default-features = false` can drop it: a feature cannot be subtracted from the set another
feature pulls in.

## Part of ag-ui-rust

| Crate | What it is |
| --- | --- |
| `ag-ui` | This crate: protocol types, wire encoding, and the server, client and axum runtimes behind features. |
| `ag-ui-a2ui` | A2UI protocol types, validators, agent authoring, and optional adapters. A different protocol, usable with no AG-UI at all, which is why it is its own crate. |

See the [repository](https://github.com/KimSoungRyoul/ag-ui-rust) for the design rationale.

## License

MIT

## Client conversations

`HttpAgent::new(url)` configures a connection. `agent.thread(id)` creates a local
conversation; `thread.send(text)?` returns a run stream. `Update::RunEnded`
reports each run. `Update::Done` reports the response stream's outcome.
`collect_report().await` retains individual runs and diagnostics. Reasoning
messages join the thread's history and travel back on later requests. A
`MESSAGES_SNAPSHOT` replaces the conversation baseline. `state()` returns a Result
for the current typed view; `raw_state()` always exposes current raw JSON.

Use two local `Thread` values with the same conversation ID to run two requests
at once, then `Thread::merge_messages_from` to reconcile messages. The app
chooses how to reconcile state and settles pending work on each thread first.

A subagent event scope records work executed by the application. Finish, fail or
suspend it explicitly. Drop restores attribution without inventing success.

New requests declare AG-UI protocol `1.0`. A successful run can still have
frontend tool calls awaiting answers; `RunEnd::SuccessWithPendingToolCalls`
preserves their IDs. Remote cancellation is `RunEnd::Cancelled`, distinct from
local `RunEnd::Aborted`. Tool results now use `ToolContent::Text` or ordered
`ToolContent::Parts`; use `as_text()` when text is required, or preserve the
parts when forwarding multimodal results. See the
[0.5 migration notes](https://github.com/KimSoungRyoul/ag-ui-rust/blob/main/docs/migration-0.5.md).

## Validate events from another runtime

With `server` and `verify`, `EventVerifier` exposes the same ordering and
attribution checks used by `RunContext`, without creating a transport queue. For a run that outlives its HTTP response,
`Runner::into_parts` separates the driver and events. `RunProgress` builds a
prelude for a late subscriber. `Runner::continue_run` restores protocol state
from sent events, and `EventVerifier::seed` or `replay` initializes ordering
checks for externally emitted events. The host supplies storage, scheduling
and recovery.

```rust
# fn main() -> Result<(), Box<dyn std::error::Error>> {
# #[cfg(all(feature = "server", feature = "verify"))] {
use ag_ui::{Event, TextMessageRole};
use ag_ui::server::EventVerifier;
let mut verifier = EventVerifier::new();
verifier.observe(&Event::run_started("thread", "run"))?;
verifier.observe(&Event::text_message_start("message", TextMessageRole::Assistant))?;
verifier.observe(&Event::text_message_content("message", ""))?;
verifier.observe(&Event::text_message_end("message"))?;
verifier.observe(&Event::run_finished("thread", "run"))?;
# }
# Ok(())
# }
```

Empty deltas are valid protocol data. Choosing to suppress them, requiring
snapshots before approval, and deciding how to stop open subagents belong to
the producer's publication policy, not additional wire validation rules.
