# ag-ui-core

Core protocol types, events, and wire encoding for the [AG-UI protocol](https://github.com/ag-ui-protocol/ag-ui).

AG-UI is the protocol between a user-facing application and an agent backend. A run is a
stream of events: the agent opens messages, streams text and reasoning, calls tools,
publishes state, and finishes — or pauses for human input.

This crate is the shared vocabulary. It has no runtime, no I/O and no async: just the
types, their exact JSON representation, and the SSE framing that carries them. Servers and
clients build on top.

```toml
[dependencies]
ag-ui-core = { git = "https://github.com/KimSoungRyoul/ag-ui-rust" }
```

Not on crates.io — these crates are unpublished, and some of the `ag-ui-*` names there
belong to other projects, so depend on the repository rather than on a version number.

```rust
use ag_ui_core::{Event, EventStreamFormatter, SseFormatter, TextMessageRole};

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

## Identifiers are strings

`ThreadId`, `RunId` and friends wrap `String`, not `Uuid`. The spec types them as strings
and real backends such as LangGraph send arbitrary values, so a stricter type would reject
valid traffic.

## Features

| Feature | Default | What it adds |
| --- | --- | --- |
| `sse` | yes | `SseFormatter` and `text/event-stream` framing. No extra dependencies. |
| `protobuf` | no | The binary transport's media type and a documented stub. `events.proto` covers only 18 of the event types, so there is no encoder. |
| `schemars` | no | Derives `schemars::JsonSchema` on the public types. |
| `utoipa` | no | Derives `utoipa::ToSchema` on the public types. |

The crate is executor-agnostic and builds for `wasm32-unknown-unknown`.

## Part of ag-ui-rust

| Crate | What it is |
| --- | --- |
| `ag-ui-core` | Protocol types, all event variants, and wire encoding. |
| `ag-ui-server` | Host an agent: `Agent` trait, typestate emitters, state deltas, verification. |
| `ag-ui-axum` | Mount an agent on an axum router. |
| `ag-ui-client` | Consume a remote agent. |
| `ag-ui-a2ui` | A2UI protocol types, validator, and authoring toolkit. |

See the [repository](https://github.com/KimSoungRyoul/ag-ui-rust) for the design rationale.

## License

MIT
