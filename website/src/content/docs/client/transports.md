---
title: Transports
description: The one async layer in the client — the HTTP transport, the SSE decoder, the replay transport, and what writing your own takes.
---

Everything else in `ag_ui::client` is synchronous. Application, chunk
normalization, verification: all plain state machines you can drive from a loop,
a test, or an event handler. The transport is the one layer that talks to the
outside world, and the only place `async` appears.

That is what lets the rest of the crate run under any executor, or none — and it
is why the one async layer is a trait. A wasm frontend, an in-process agent, a
websocket, a recorded fixture: each is an `impl Transport`, and nothing above it
changes.

## The trait

```rust
// The whole of it, restated here rather than quoted — so this page stops
// compiling if the signature ever moves.
use ag_ui::client::transport::TransportFuture;
use ag_ui::RunAgentInput;

trait Transport {
    fn run(&self, input: RunAgentInput) -> TransportFuture;
}
```

One method. Hand it a `RunAgentInput`, get back a future that resolves to a
stream of events. Failing to *connect* is an error from the future; failing
mid-stream is an error item in the stream — and the two are different things a
client wants to say differently.

`TransportFuture` is `Pin<Box<dyn Future<Output = Result<EventStream>> + Send>>`.
Nothing in that names a lifetime, which means the default one for a boxed trait
object — `'static` — and that is load-bearing. A transport is usually held
inside a `Session`, which mutates its own state as events arrive. If the
returned future borrowed the transport, that borrow would live as long as the
run and the session could not touch itself while streaming. So `run` clones what
it needs — `reqwest::Client` is explicitly designed for exactly that — and the
future stands alone.

`&T`, `Box<T>` and `Arc<T>` are transports when `T` is, so a client can pick one
at runtime without a generic parameter reaching through the whole application:

```rust
// src/main.rs
use ag_ui::client::transport::{ReplayTransport, Transport};
use ag_ui::client::{RunEnd, Session, Update};
use ag_ui::Event;
use futures_util::StreamExt;

#[tokio::main]
async fn main() {
    // In an application this is where a `--replay` flag would choose between a
    // fixture and an `HttpTransport`.
    let transport: Box<dyn Transport> = Box::new(ReplayTransport::new([
        Event::run_started("thread-1", "run-1"),
        Event::run_finished_success("thread-1", "run-1"),
    ]));

    let mut session = Session::<_>::new(transport, "thread-1");
    let updates: Vec<_> = session.send("hello").collect().await;

    assert!(matches!(updates.last(), Some(Update::Done(RunEnd::Success { .. }))));
}
```

On wasm the `EventStream` and `TransportFuture` aliases drop their `Send` bound.
The browser APIs a transport would be built on there are single-threaded and not
`Send` at all, and requiring it would make the wasm case — the reason this crate
abstracts the transport in the first place — impossible to satisfy.

## `HttpTransport`

The default, behind the `http` feature. One POST of the `RunAgentInput` as JSON,
one `text/event-stream` response decoded frame by frame. It is the only place in
the crate that pulls in an HTTP client.

```rust
// src/main.rs
use ag_ui::client::transport::HttpTransport;
use std::time::Duration;

fn main() -> Result<(), ag_ui::client::Error> {
    let transport = HttpTransport::builder("https://example.com/agent")
        .header("authorization", "Bearer token")
        .header("x-tenant", "acme")
        // Bounds connection setup only, leaving the stream itself unbounded.
        .connect_timeout(Duration::from_secs(5))
        .build()?;

    assert_eq!(transport.url().as_str(), "https://example.com/agent");
    // Every request asks for the stream format, without the caller saying so.
    assert_eq!(transport.headers()["accept"], "text/event-stream");
    Ok(())
}
```

Header values are validated in `build`, not in the setters, so a chain of
setters stays a chain instead of threading a `Result` through every step. A URL
that does not parse, a header name that is not a header name, or a client that
cannot be built are all `Error::Config`.

:::caution[`timeout` and `connect_timeout` are not variations of each other]
`timeout` bounds the *whole* run: connecting, headers, and streaming the body.
An agent that thinks for longer than that has its stream cut off mid-answer,
which arrives at the client as a truncated run. A long-running agent wants
`connect_timeout`, which bounds only the setup and leaves the stream unbounded.
:::

`client(…)` takes a pre-configured `reqwest::Client` for proxies, custom TLS
roots, or a connection pool shared with the rest of an application.

A response outside 2xx never becomes a stream: it is an `Error::Http` carrying
the status and the first 2048 characters of the body, which is enough to read a
gateway's HTML error page without putting a megabyte of it in a log line.

`HttpAgent` is the same transport at the lower level — `RemoteAgent<HttpTransport>`,
with a builder that forwards to this one.

## `ReplayTransport`

Testing a client against a live agent is slow, flaky, and needs a model. It is
also unnecessary: the agent's half of the conversation is just a list of events.

```rust
// tests/client.rs
use ag_ui::client::{Session, transport::ReplayTransport};
use ag_ui::{Event, Interrupt};
use futures_util::StreamExt;
use serde_json::json;

#[tokio::main]
async fn main() {
    // One list per run, so a human-in-the-loop round trip is scriptable: the
    // first run pauses, the second — the resume — carries on.
    let transport = ReplayTransport::with_runs([
        vec![
            Event::run_started("thread-1", "run-1"),
            Event::run_finished_interrupt(
                "thread-1",
                "run-1",
                vec![Interrupt::new("i-1", "tool_approval")],
            ),
        ],
        vec![
            Event::run_started("thread-1", "run-2"),
            Event::run_finished_success("thread-1", "run-2"),
        ],
    ]);

    // Cloning shares the script and the recording, so a test keeps a handle
    // after handing one to the session.
    let mut session = Session::<_>::new(transport.clone(), "thread-1");
    session.send("delete the staging database").collect::<Vec<_>>().await;

    let paused = session.interrupts().to_vec();
    session.resume(&paused[0], json!({ "approved": true })).collect::<Vec<_>>().await;

    // What the client actually sent — how a test asserts that a resume carried
    // the right answers.
    let requests = transport.requests();
    assert_eq!(requests.len(), 2);
    assert_eq!(requests[1].resume.as_deref().unwrap()[0].interrupt_id, "i-1");
    assert_eq!(transport.remaining(), 0);
}
```

`new` scripts a single run and answers every later one with an error, which is
usually what you want: a test that runs twice by accident should say so.

## The SSE decoder

Any HTTP transport needs one, so it ships separately from the `http` feature and
is usable on its own. It is a wire-format parser fed by a network, so it assumes
nothing about how the bytes arrive: chunks split lines, split UTF-8 sequences,
and end without the terminating blank line the format calls for.

```rust
// src/transport.rs
use ag_ui::client::transport::SseDecoder;

fn main() -> Result<(), ag_ui::client::Error> {
    let mut decoder = SseDecoder::new();

    // A proxy's heartbeat, then half an event.
    decoder.push(b": keep-alive\n\ndata: {\"type\":\"RUN_ERROR\",\"mes")?;
    assert!(decoder.next_frame()?.is_none());

    decoder.push(b"sage\":\"boom\"}\n\n")?;
    let frame = decoder.next_frame()?.expect("a complete frame");
    assert_eq!(frame.into_event()?.event_type().as_str(), "RUN_ERROR");
    Ok(())
}
```

What it handles, because real servers and proxies do all of it: `data:` repeated
over several lines and rejoined with `\n`; comment lines that dispatch nothing;
a body that ends without a blank line, where `finish` dispatches the last frame
rather than dropping it; `\n`, `\r\n` and lone `\r` endings, including a `\r\n`
split across two chunks; a leading byte-order mark; a field with no colon; and a
frame with no `data` field, which the format says not to dispatch.

What it refuses: invalid UTF-8, and a single frame larger than
`max_frame_size` — 8 MiB by default. An unterminated line is otherwise an
unbounded allocation driven by the other end. The cap is on the *frame*, not on
the chunk: one read carrying a thousand complete frames is ordinary, and
counting that against a per-frame limit would reject a well-behaved server.

`decode_events` is the adapter that turns a stream of byte chunks into a stream
of events. An error from the byte stream ends the stream; a frame whose payload
is not a valid event becomes an error item and the stream **continues**, because
one malformed event should not silence the rest of a run.

## Writing your own

The whole of it is one method. This is the shape an in-process agent or a wasm
frontend takes — no HTTP client anywhere in it:

```rust
// src/transport.rs
use ag_ui::client::transport::{EventStream, Transport, TransportFuture};
use ag_ui::client::{RunEnd, Session, Update};
use ag_ui::{Event, RunAgentInput, TextMessageRole};
use futures_util::StreamExt;

/// Serves a fixed list of events, whatever it is asked.
#[derive(Clone, Debug)]
struct StaticTransport {
    events: Vec<Event>,
}

impl Transport for StaticTransport {
    fn run(&self, _input: RunAgentInput) -> TransportFuture {
        // Cloned, not borrowed: the future outlives this call.
        let events = self.events.clone();
        Box::pin(async move {
            let stream = futures_util::stream::iter(events.into_iter().map(Ok));
            Ok(Box::pin(stream) as EventStream)
        })
    }
}

#[tokio::main]
async fn main() {
    let transport = StaticTransport {
        events: vec![
            Event::run_started("thread-1", "run-1"),
            Event::text_message_start("msg-1", TextMessageRole::Assistant),
            Event::text_message_content("msg-1", "From somewhere else entirely."),
            Event::text_message_end("msg-1"),
            Event::run_finished_success("thread-1", "run-1"),
        ],
    };

    let mut session = Session::<_>::new(transport, "thread-1");
    let updates: Vec<_> = session.send("hello").collect().await;

    assert!(matches!(updates.last(), Some(Update::Done(RunEnd::Success { .. }))));
    assert_eq!(
        session.applier().text_of("msg-1"),
        Some("From somewhere else entirely.")
    );
}
```

A transport that reads bytes rather than events puts `decode_events` in the
middle, and `boxed_stream` is the helper that boxes the result into the shape
the trait returns:

```rust
// src/transport.rs
use ag_ui::client::transport::{Transport, TransportFuture, boxed_stream, decode_events};
use ag_ui::client::{RunEnd, Session, Update};
use ag_ui::{Event, RunAgentInput, SseFormatter};
use futures_util::StreamExt;

/// Answers every run from a recorded response body.
struct Recorded(String);

impl Transport for Recorded {
    fn run(&self, _input: RunAgentInput) -> TransportFuture {
        let body = self.0.clone();
        Box::pin(async move {
            // One chunk here; a real body arrives in as many as the network
            // decides, and the decoder is written for that.
            let chunks = futures_util::stream::iter([Ok::<_, std::io::Error>(body)]);
            Ok(boxed_stream(decode_events(chunks)))
        })
    }
}

#[tokio::main]
async fn main() {
    let sse = SseFormatter::new();
    let mut body = String::new();
    for event in [
        Event::run_started("thread-1", "run-1"),
        Event::run_finished_success("thread-1", "run-1"),
    ] {
        body.push_str(&sse.encode_to_string(&event).expect("encodes"));
    }

    let mut session = Session::<_>::new(Recorded(body), "thread-1");
    let updates: Vec<_> = session.send("hello").collect().await;

    assert!(matches!(updates.last(), Some(Update::Done(RunEnd::Success { .. }))));
}
```

Errors from an exotic runtime do not need a variant in this crate's error enum:
`Error::transport(e)` wraps anything that is
`std::error::Error + Send + Sync + 'static`.

## Turning `http` off

`http` is on by default and pulls in `reqwest`. Turning it off is what keeps the
crate wasm-viable, and it removes `HttpTransport` and `HttpAgent` with it —
bring your own `Transport`.

```toml
[dependencies.ag-ui]
version = "0.1"
default-features = false
features = ["client", "sse"]
```

CI enforces both halves of that claim. `cargo check -p ag-ui
--no-default-features --features client --target wasm32-unknown-unknown` fails if anything outside
the feature reaches for `reqwest`, and a separate job asserts that `tokio` is
absent from the dependency graph in that configuration — because tokio itself
compiles for wasm, so a green wasm build alone would not have caught it. A
manifest edit that made `reqwest` unconditional would pass every compile, so
`crates/ag-ui/tests/client_features.rs` reads the manifest and checks that too.

More on what each feature costs is in the [feature
reference](/ag-ui-rust/reference/features/), and what builds where is in
[platforms and MSRV](/ag-ui-rust/reference/platforms/).

## Next

- [Sessions](/ag-ui-rust/client/session/) — what sits on top of a transport.
- [`Transport`](/ag-ui-rust/api/ag_ui/client/transport/trait.Transport.html) and
  [`HttpTransport`](/ag-ui-rust/api/ag_ui/client/transport/http/struct.HttpTransport.html)
  in the API docs.
