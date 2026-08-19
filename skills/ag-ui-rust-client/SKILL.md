---
name: ag-ui-rust-client
description: "MUST USE when writing Rust against ag-ui-rust to consume an agent — the crate ag-ui with its `http` or `client` feature (AG-UI protocol client: sessions, the update stream, transports, rendering a run). UNCONVENTIONAL, and wrong from memory: this is ONE crate named ag-ui, not ag-ui-client / ag-ui-core — those registry names belong to an unrelated community SDK — and the client lives under ag_ui::client behind a feature. Session::send returns a RunStream that borrows the session mutably — drop it before reading session.messages(); Session<T, S> carries the transport bound on the CONSTRUCTOR, not the type; Update is #[non_exhaustive] but RunEnd is EXHAUSTIVE with exactly three variants (Success, Interrupted, Failed) and wants no `_` arm; Update::Error is NOT terminal and a run can both complain and succeed; an unrecognised event type ends the run rather than being skipped; interrupts must all be answered in ONE request or the resume never terminates; tools travel from the client on every request because AG-UI has no tool discovery. Covers HttpTransport (connect_timeout vs timeout), ReplayTransport for tests, writing a Transport in one method, typed state, and rendering in arrival order. Triggers on: ag-ui-rust client, ag_ui::client, Session::send, RunStream, Update::Message, MessageChangeKind, RunEnd, HttpTransport, ReplayTransport, RemoteAgent, consume an AG-UI agent from Rust, AG-UI TUI or frontend in Rust."
---

# Consuming an AG-UI agent from Rust

Docs: <https://kimsoungryoul.github.io/ag-ui-rust/> · this skill is written against
workspace version **0.1.0**. If the API here disagrees with the compiler, the compiler is
right and the skill is stale — see `ag-ui-rust-update`.

## Adding the crates

**One crate, `ag-ui`.** Not `ag-ui-client` / `ag-ui-core` — those names on crates.io are a
different, unrelated project. Which half of the protocol you get is a feature:

```toml
# Cargo.toml
[dependencies]
ag-ui = { version = "0.1", features = ["http"] }
futures-util = "0.3"
tokio = { version = "1", features = ["rt-multi-thread", "macros"] }
```

`http` brings `reqwest`. Ask for `client` instead and the crate is executor-agnostic,
builds for `wasm32-unknown-unknown`, and you bring your own `Transport`.

## A session

`Session` folds the delta stream back into a conversation: messages, state, interrupts.
Below it, `RemoteAgent` hands you events unassembled — the level for a proxy or a recorder.

```rust
// src/main.rs
use ag_ui::client::{RunEnd, Session, Update, transport::ReplayTransport};
use ag_ui::{Event, TextMessageRole};
use futures_util::StreamExt;

#[tokio::main]
async fn main() {
    // A scripted agent: no server, no network. This is how you test a client.
    let transport = ReplayTransport::new([
        Event::run_started("thread-1", "run-1"),
        Event::text_message_start("msg-1", TextMessageRole::Assistant),
        Event::text_message_content("msg-1", "It is "),
        Event::text_message_content("msg-1", "sunny."),
        Event::text_message_end("msg-1"),
        Event::run_finished_success("thread-1", "run-1"),
    ]);

    let mut session = Session::<_>::new(transport, "thread-1");

    let mut ended = None;
    let mut run = session.send("what is the weather?");
    while let Some(update) = run.next().await {
        match update {
            Update::Message(message) => println!("{}: {:?}", message.id, message.change),
            Update::Done(end) => ended = Some(end),
            _ => {}
        }
    }
    drop(run); // the RunStream borrows the session mutably until it drops

    assert!(matches!(ended, Some(RunEnd::Success { .. })));
    assert_eq!(session.messages().len(), 2);
}
```

Against a real agent only the transport changes:

```rust,no_run
use ag_ui::client::{Session, transport::HttpTransport};
use std::time::Duration;

fn open() -> Result<(), ag_ui::client::Error> {
    let transport = HttpTransport::builder("http://localhost:3000/agent")
        .header("authorization", "Bearer …")
        // Bounds connection setup only. `timeout` bounds the WHOLE run and will
        // cut a thinking agent off mid-answer — not what you want here.
        .connect_timeout(Duration::from_secs(5))
        .build()?;

    let _session = Session::<_>::new(transport, "thread-1");
    Ok(())
}
```

Three ways to start a run, all returning the same `RunStream`: `send(text)` appends a user
message first, `send_message(message)` takes any role, `run()` uses the conversation as it
stands. Run ids are `{thread}-run-{n}`.

Reading a session needs no run in flight and no transport bound: `messages()`, `state()`,
`raw_state()`, `reasoning()`, `interrupts()`, `thread_id()`, `applier()`, `agent()`.

**The transport bound is on the constructor**, not on `Session<T, S>`. That is deliberate —
a helper that only reads `messages()` never names a transport, and passing a URL where a
transport belongs fails at the call site instead of at the first `send`.

## The update stream

One `Update` is one redraw. It is per **event**, not per entity: forty deltas are forty
`Update::Message`s under one id, and two tool calls in flight interleave.

| Variant | Meaning |
| --- | --- |
| `Message(MessageUpdate)` | `index`, `id`, `change`, and the whole assembled `message` |
| `Messages(Vec<Message>)` | `MESSAGES_SNAPSHOT` replaced the conversation — redraw all of it |
| `State(S)` | the state, in your type, by value. Snapshot and patch arrive identically |
| `Reasoning(..)` | reasoning text, kept out of the transcript |
| `Interrupt(Interrupt)` | the run paused; one update per pending interrupt |
| `Error(ag_ui::client::Error)` | **not terminal** — print it and keep going |
| `Done(RunEnd)` | always the last update of a run, on every path out |

`Update` is `#[non_exhaustive]` (a view model). `RunEnd` is **exhaustive** — write three arms
and no `_`, because this is the match that decides whether the input goes live again:

```rust
use ag_ui::client::RunEnd;

fn prompt_again(end: &RunEnd) -> bool {
    match end {
        RunEnd::Success { .. } => true,             // result: Option<Value>
        RunEnd::Interrupted { .. } => false,        // answer the interrupts instead
        RunEnd::Failed { .. } => true,              // message: String, code: Option<String>
    }
}

fn main() {
    assert!(prompt_again(&RunEnd::Success { result: None }));
    assert!(!prompt_again(&RunEnd::Interrupted { interrupts: Vec::new() }));
}
```

**`Success` does not mean nothing went wrong.** A protocol violation the client's verifier
caught, or a patch that would not apply, arrives as `Update::Error` and the run carries on to
succeed — the agent is neither told nor asked. Track errors as they land if the difference
matters. When a failure *is* fatal, `RunEnd::Failed` always has its `Update::Error` in front
of it, including when the transport simply stopped.

**An unknown event type ends the run.** `Event` is exhaustive on purpose, so a frontend
talking to a newer agent stops with an error naming the type rather than quietly rendering
three quarters of a conversation. What arrived before it is still in `session.messages()`.

## Answering a pause

```rust
use ag_ui::client::interrupts::ResumeBuilder;
use ag_ui::{Interrupt, ResumeStatus};
use serde_json::json;

fn main() {
    let budget = Interrupt::new("approve-budget", "tool_approval");
    let date = Interrupt::new("confirm-date", "tool_approval");

    let entries = ResumeBuilder::new()
        .resolve(&budget, json!({ "approved": true }))
        .cancel(&date)
        .build();

    assert_eq!(entries[0].interrupt_id, "approve-budget");
    assert_eq!(entries[1].status, ResumeStatus::Cancelled);
    // session.resume_many(entries) — both answers, one request.
}
```

`session.resume(&interrupt, payload)` for one, `resume_many` for several, `session.cancel(..)`
when the human said no. **Answer every pending interrupt in one request.** The resumed run
supersedes the paused one and the agent only sees what the resuming request carries, so
answering one at a time never terminates. The resumed run gets a new run id.

`resolve_with_edits` writes the `editedArgs` key that agents advertising `approveWithEdits`
expect.

## Tools travel from the client

AG-UI has no tool discovery and no negotiation: an agent cannot ask for a tool it was not
sent. Offering none to an agent that needs one produces *the agent's* error ("the client
offered no add_task tool") as an ordinary failed run — which reads like an agent bug and is
not one. Configure the tool set the way you configure a URL: `SessionBuilder::tools`, or
`Session::set_tools` from the next run on.

`Session::builder` also takes `messages`, `state`, `context`, `forwarded_props`, and `verify`
(client-side verification, on by default).

## Typed state

`Session<T, S = Value>`. `S` is inferred from what you do with an `Update::State`, so a
turbofish is rarely needed. To stream it, `S: Deserialize + Clone + Unpin`.

State that does not fit `S` is not a lost run: `raw_state()` stays correct and the mismatch
arrives as an `Update::Error`. And `session.state() == None` is **not** "the state is empty"
— it means no `STATE_*` event has arrived at all, which is usually a broken agent.

## Rendering

Arrival order is the only nesting the protocol has; there is no containment. Two open calls
alternate their argument fragments, so `MessageChangeKind::ToolCallArgs { tool_call_id, .. }`
— the id, not adjacency — says which call a fragment belongs to.

`MessageChangeKind`: `Started`, `Content { delta }`, `Ended`, `ToolCallStarted`,
`ToolCallArgs`, `ToolCallEnded`, `ToolResult`, `Activity`, `EncryptedValue`.

Buffering a call so it draws on one line is legitimate and reorders the run: the line cannot
be written until the call closes, so anything that arrived during the call draws first.
Legibility under parallel calls comes from tagging each line with the call id, not from
buffering. `references/rendering.md` has both renderings side by side, and what the client
already handles for you (chunk events, unterminated messages, malformed streams).

## Transports

One method, and the returned future must not borrow `self`:

```rust
use ag_ui::client::transport::{EventStream, Transport, TransportFuture};
use ag_ui::client::{RunEnd, Session, Update};
use ag_ui::{Event, RunAgentInput, TextMessageRole};
use futures_util::StreamExt;

struct StaticTransport {
    events: Vec<Event>,
}

impl Transport for StaticTransport {
    fn run(&self, _input: RunAgentInput) -> TransportFuture {
        // Cloned, not borrowed: `TransportFuture` is 'static, which is what
        // lets the session mutate itself while the run streams.
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
}
```

A transport that reads bytes puts `decode_events` in the middle and `boxed_stream` around the
result. Failing to *connect* is an error from the future; failing mid-stream is an error item
in the stream — clients say those differently. `&T`, `Box<T>` and `Arc<T>` are transports when
`T` is, so `Box<dyn Transport>` picks one at runtime. On wasm the aliases drop their `Send`
bound. `Error::transport(e)` wraps any exotic runtime's error.

`SseDecoder` is the wire parser, usable on its own: chunks split lines and UTF-8 sequences,
`finish` dispatches a body that ended without a blank line, and a frame over `max_frame_size`
(8 MiB) is refused.

## Stopping

There is no `stop()` method. Polling the stream is what pulls bytes, so **dropping the
`RunStream` is client-side cancellation** — and it reaches the far end, which trips the
agent's cancellation token. `Session::cancel` answers an interrupt; it does not stop a run.
The session stays usable afterwards.

## Do not write

| Instead of | Write |
| --- | --- |
| `ag-ui-client = "0.1"` | the git dependency above — the registry name is someone else's crate |
| `session.messages()` while a run is alive | `drop(run)` first; the stream holds the borrow |
| `_ => {}` in a `RunEnd` match | three arms; `RunEnd` is exhaustive so a fourth is a compile error |
| treating `Update::Error` as the end | keep going; `Update::Done` says when it is over |
| one `resume` per interrupt | `resume_many` / `ResumeBuilder` — all answers, one request |
| appending args to "the call in progress" | key by `tool_call_id` |
| `HttpTransport::builder(..).timeout(..)` for a slow agent | `.connect_timeout(..)` |

## Deeper

- [Sessions](https://kimsoungryoul.github.io/ag-ui-rust/client/session/) ·
  [The update stream](https://kimsoungryoul.github.io/ag-ui-rust/client/updates/) ·
  [Rendering a run](https://kimsoungryoul.github.io/ag-ui-rust/client/rendering/) ·
  [Transports](https://kimsoungryoul.github.io/ag-ui-rust/client/transports/)
- [board-watch](https://kimsoungryoul.github.io/ag-ui-rust/examples/board-watch/) — a terminal
  client for any AG-UI agent, with both renderings
- rustdoc: <https://kimsoungryoul.github.io/ag-ui-rust/api/ag_ui/client/index.html>
- The agent half is the `ag-ui-rust-server` skill.
