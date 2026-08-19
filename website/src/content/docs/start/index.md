---
title: Getting started
description: Add the crates to a Cargo project, serve a first agent, and talk to it from a Rust client.
---

AG-UI is the protocol between a user-facing application and an agent backend: a POST
carrying a run input, answered by a stream of typed events. This SDK gives you both ends
of that exchange in Rust — an agent you can host, and a client that consumes one.

By the end of this page an agent is answering on `http://127.0.0.1:3000/agent`, and a
second program is holding a conversation with it.

:::note
Every Rust block on this site is compiled by the workspace's test suite, the same way the
`README` quickstarts are. A snippet that has gone stale is a red build rather than
something you find out by pasting it.
:::

## What you need

Rust **1.85 or newer**. The workspace sets `rust-version = "1.85"` and
`edition = "2024"`, and 1.85 is the first compiler that understands that edition.

That is the whole list. There is no protobuf compiler and no code generation step:
`ag-ui` depends on `serde` and `serde_json` and nothing else, and the crates that
build on it add `futures` primitives rather than a runtime. tokio enters only when you
reach for `ag_ui::axum`.

The one thing in the tree that reaches outside Rust is TLS. `ag_ui::client`'s default
`http` feature pulls `reqwest` with `rustls`, whose crypto backend compiles C, so a client
build wants a C toolchain. Nothing on the agent side does.

## Adding the crates

:::caution[`ag-ui` here is not the community crate]
The `ag-ui-core`, `ag-ui-server` and `ag-ui-client` names on crates.io belong to an
earlier, unrelated community SDK and are not this project. This project is the single
`ag-ui` crate, plus `ag-ui-a2ui`.
:::

One crate, and which half of the protocol you get is a feature. For an agent, that is
`axum` — which implies `serve` — and a web server:

```toml
# Cargo.toml
[dependencies]
ag-ui = { version = "0.1", features = ["axum"] }
axum = "0.8"
tokio = { version = "1", features = ["rt-multi-thread", "macros", "net"] }
```

For a client, `http`:

```toml
# Cargo.toml
[dependencies]
ag-ui = { version = "0.1", features = ["http"] }
futures-util = "0.3"
tokio = { version = "1", features = ["rt-multi-thread", "macros"] }
```

The protocol types — `Tool`, `Message`, `Event` — are at the crate root either way, so
they are there as soon as you do anything beyond printing text.

`http` is what brings the `reqwest` transport with it. Ask for `client` instead and the
crate is executor-agnostic and builds for `wasm32-unknown-unknown`, with your own
transport underneath.

Which feature does what, and why this is one crate rather than five, is
[The crates](/ag-ui-rust/start/crates/).

## Your first agent

An agent is one trait implementation. `Agent::run` is handed a run context, emits events
through it, and returns how the run ended.

```rust,no_run
// src/main.rs
use ag_ui::axum::RouterExt;
use ag_ui::RunOutcome;
use ag_ui::serve::{Agent, Result, RunContext};
use axum::Router;

struct Greeter;

impl Agent for Greeter {
    type State = ();

    async fn run(&self, ctx: &mut RunContext<()>) -> Result<RunOutcome> {
        // Streams as TEXT_MESSAGE_START / _CONTENT / _END.
        let mut message = ctx.assistant_message()?;
        message.delta("Hello from Rust.")?;
        message.end()?;

        Ok(RunOutcome::Success)
    }
}

#[tokio::main]
async fn main() {
    let app: Router = Router::new().route_agui("/agent", Greeter);

    let listener = tokio::net::TcpListener::bind("127.0.0.1:3000").await.unwrap();
    println!("agent on http://127.0.0.1:3000/agent");
    axum::serve(listener, app).await.unwrap();
}
```

`cargo run`, and that is a working AG-UI endpoint.

Four things in there are worth naming.

`type State = ()` says this agent shares no application state with the client. Give it a
`serde` type instead and the client mirrors it; see
[Shared state](/ag-ui-rust/server/state/).

`ctx.assistant_message()` returns a *handle* that borrows the context mutably. While it is
alive you cannot open a second message, so interleaving two of them is a borrow-check
error rather than a stream a frontend has to survive. The handle also emits its
`TEXT_MESSAGE_END` when it drops, so the explicit `end()` above is a convenience and not
an obligation — [Streaming text](/ag-ui-rust/server/text/) goes through both.

`message.delta(…)?` takes no `.await`. Because a handle emits its terminator on `Drop` and
`Drop` cannot be async, the whole emit path is synchronous: handles push into a channel
and the transport drains it. That trade is explained in
[Design commitments](/ag-ui-rust/design/commitments/).

`route_agui` is `route(path, post(handler))` and nothing more. The router it returns is an
ordinary axum `Router`, so your own routes, layers and state go on it as usual —
[Serving over HTTP](/ag-ui-rust/server/axum/).

## What comes back

```sh
curl -N -X POST http://127.0.0.1:3000/agent \
  -H 'content-type: application/json' \
  -d '{"threadId":"thread-1","runId":"run-1","messages":[],"tools":[],"context":[]}'
```

```text
data: {"type":"RUN_STARTED","threadId":"thread-1","runId":"run-1"}

data: {"type":"TEXT_MESSAGE_START","messageId":"run-1-msg-1","role":"assistant"}

data: {"type":"TEXT_MESSAGE_CONTENT","messageId":"run-1-msg-1","delta":"Hello from Rust."}

data: {"type":"TEXT_MESSAGE_END","messageId":"run-1-msg-1"}

data: {"type":"RUN_FINISHED","threadId":"thread-1","runId":"run-1","outcome":{"type":"success"}}
```

A run always opens with `RUN_STARTED` and closes with exactly one of `RUN_FINISHED` or
`RUN_ERROR`, and everything between is deltas: a message opens, text arrives a fragment at
a time, the message closes. The message id was derived from the run id rather than a UUID,
which is what makes a recorded stream diffable.

A *failed* run is still a `200`. By the time an agent can fail the status line has long
since been sent, so the failure arrives as a `RUN_ERROR` event inside a well-formed
stream. That is what lets a client tell "the agent errored" from "the network died".

[How AG-UI works](/ag-ui-rust/start/protocol/) covers the request body, the event families
and the framing properly.

## The same run, without a port

`ag_ui::axum` is a wrapper. Underneath it, `ag_ui::serve::run` turns an agent into a
`Stream` of events and has no opinion about how they reach anyone — which means an agent
is testable as a pure stream, with no server, no port and no client:

```rust
// tests/greeter.rs
use ag_ui::{Event, EventStreamFormatter, EventType, RunAgentInput, RunOutcome, SseFormatter};
use ag_ui::serve::{Agent, Result, RunContext, run};
use futures_util::StreamExt;

struct Greeter;

impl Agent for Greeter {
    type State = ();

    async fn run(&self, ctx: &mut RunContext<()>) -> Result<RunOutcome> {
        let mut message = ctx.assistant_message()?;
        message.delta("Hello from Rust.")?;
        message.end()?;

        Ok(RunOutcome::Success)
    }
}

#[tokio::main]
async fn main() {
    let events: Vec<Event> = run(Greeter, RunAgentInput::new("thread-1", "run-1"))
        .map(|event| event.expect("the stream should not break"))
        .collect()
        .await;

    let types: Vec<EventType> = events.iter().map(Event::event_type).collect();
    assert_eq!(
        types,
        [
            EventType::RunStarted,
            EventType::TextMessageStart,
            EventType::TextMessageContent,
            EventType::TextMessageEnd,
            EventType::RunFinished,
        ]
    );

    // And this is the body the endpoint writes: the same events, SSE-framed.
    let formatter = SseFormatter::new();
    let body: String = events
        .iter()
        .map(|event| formatter.encode_to_string(event).unwrap())
        .collect();

    assert!(body.starts_with(r#"data: {"type":"RUN_STARTED","threadId":"thread-1","runId":"run-1"}"#));
    assert!(body.ends_with("\"outcome\":{\"type\":\"success\"}}\n\n"));
}
```

[Testing](/ag-ui-rust/design/testing/) is about writing agent tests in that shape.

## Talking to it from Rust

The other half of the SDK consumes an agent. `Session` holds a thread — its messages and
its state — and folds the delta stream back into them, so what you handle is "this message
grew" rather than "a `TEXT_MESSAGE_CONTENT` arrived":

```rust,no_run
// src/main.rs
use std::io::Write;

use ag_ui::client::{MessageChangeKind, RunEnd, Session, Update, transport::HttpTransport};
use futures_util::StreamExt;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let transport = HttpTransport::new("http://127.0.0.1:3000/agent")?;
    let mut session = Session::<_>::new(transport, "thread-1");

    let mut run = session.send("hello");
    while let Some(update) = run.next().await {
        match update {
            Update::Message(message) => {
                if let MessageChangeKind::Content { delta } = message.change {
                    print!("{delta}");
                    std::io::stdout().flush()?;
                }
            }
            Update::Error(error) => eprintln!("\n{error}"),
            Update::Done(RunEnd::Success { .. }) => println!(),
            _ => {}
        }
    }
    drop(run);

    println!("{} messages in the thread", session.messages().len());
    Ok(())
}
```

Run the agent in one terminal and this in another, and it prints `Hello from Rust.` a
fragment at a time, then `2 messages in the thread` — yours and the agent's.

Two things that are easy to miss. The thread lives in the *client*: the session carries
the conversation and the state from one run to the next, and the agent is handed both on
every request, which is why a second client joining the same thread id starts empty. And
`drop(run)` is not ceremony — the run borrows the session while it streams, and dropping
it early is also how you cancel, because polling the stream is what pulls the bytes.

[Sessions](/ag-ui-rust/client/session/) and
[The update stream](/ag-ui-rust/client/updates/) take it from here.

## Where to go next

- [How AG-UI works](/ag-ui-rust/start/protocol/) — the wire: the request body, the run
  lifecycle, the event families, the SSE framing.
- [The crates](/ag-ui-rust/start/crates/) — five crates, what each is for, and which you
  need for which job.
- [The Agent trait](/ag-ui-rust/server/agent/) — the server side properly: tool calls,
  shared state, human-in-the-loop pauses, errors and cancellation.
- [Sessions](/ag-ui-rust/client/session/) — the client side properly, including the lower
  level a proxy or a recorder wants.
- [task-board](/ag-ui-rust/examples/task-board/) and
  [board-watch](/ag-ui-rust/examples/board-watch/) — two worked examples, each an agent
  and a client that talk to each other over a real port.
- [API docs](/ag-ui-rust/api/ag_ui/index.html) — the rustdoc for every crate.
