# ag-ui-client

Consume a remote [AG-UI](https://github.com/ag-ui-protocol/ag-ui) agent: turn its event
stream into messages and state.

An AG-UI run arrives as deltas — a message opens, text arrives a fragment at a time, tool
arguments accumulate as partial JSON, state moves by RFC 6902 patch, and the run may pause
to ask a human something. This crate is the consumer half of that protocol: the state
machines that fold a stream back into a conversation, the wire-format decoder that feeds
them, and two levels of API over the top.

```toml
[dependencies]
ag-ui-client = "0.1"
ag-ui-core = "0.1"
```

```rust
use ag_ui_client::{RunEnd, Session, Update, transport::ReplayTransport};
use ag_ui_core::{Event, PatchOperation, TextMessageRole};
use futures_util::StreamExt;
use serde::Deserialize;

/// The agent's state, in your own type.
#[derive(Clone, Debug, Deserialize, PartialEq)]
struct Weather {
    checked: bool,
}

#[tokio::main]
async fn main() {
    // A transport that replays a scripted run, so this example needs no network.
    let transport = ReplayTransport::new([
        Event::run_started("thread-1", "run-1"),
        Event::text_message_start("msg-1", TextMessageRole::Assistant),
        Event::text_message_content("msg-1", "It is "),
        Event::text_message_content("msg-1", "sunny."),
        Event::text_message_end("msg-1"),
        Event::state_delta(vec![PatchOperation::add("/checked", true)]),
        Event::run_finished_success("thread-1", "run-1"),
    ]);

    let mut session = Session::new(transport, "thread-1");
    let mut weather = None;
    let mut ended = None;

    let mut run = session.send("what is the weather?");
    while let Some(update) = run.next().await {
        match update {
            Update::Message(message) => println!("{:?}", message.change),
            // The state arrives already typed — and this is where the type
            // comes from, so `Session` needs no turbofish.
            Update::State(state) => weather = Some(state),
            Update::Error(error) => eprintln!("{error}"),
            Update::Done(end) => ended = Some(end),
            _ => {}
        }
    }
    drop(run);

    assert!(matches!(ended, Some(RunEnd::Success { .. })));
    assert_eq!(session.messages().len(), 2);
    assert_eq!(weather, Some(Weather { checked: true }));
    // The raw JSON is always there too, whether or not it fits the type.
    assert_eq!(session.raw_state()["checked"], true);
}
```

## Two levels

`RemoteAgent` is the low level: `agent.run(params)` gives you the events exactly as the
agent sent them, unassembled. That is what a proxy, a recorder or a bridge to another
protocol wants. (It is `RemoteAgent` and not `Agent` because `ag-ui-server::Agent` is the
trait you implement to *be* an agent, and an agent that calls another agent imports both.)

`Session` is the high level: a thread, its accumulated messages, and typed state.
`session.send(text)` yields `Update`s — "this message grew", "the state changed", "the
agent is waiting on you" — with chunk normalization, protocol verification and delta
application already done.

## The pieces underneath

- `apply` — the event applier. Deltas in, materialised messages and state out, plus a
  report of what changed so a view can redraw one row.
- `chunks` — normalizes `*_CHUNK` events into explicit start/content/end triples. Chunks
  carry their id only on the first one, so this stage remembers.
- `verify` — the ordering rules, checked client-side as the TypeScript SDK does. A
  malformed stream produces one clear error instead of a confused UI.
- `interrupts` — the human-in-the-loop round trip.
- `transport` — where events come from: `Transport`, an SSE decoder, a `reqwest` client,
  and a replay transport for tests.

## Features

| Feature | Default | What it adds |
| --- | --- | --- |
| `http` | yes | A `reqwest`-backed HTTP transport. Turn it off for wasm or a custom transport. |

With `http` off the crate is executor-agnostic and builds for `wasm32-unknown-unknown`;
bring your own `Transport`. CI enforces that by asserting tokio is absent from the
dependency graph in that configuration.

See the [repository](https://github.com/KimSoungRyoul/ag-ui-rust) for the design rationale.

## License

MIT
