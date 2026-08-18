---
title: Sessions
description: Opening a session against a remote agent, sending it a turn, and reading the messages and state it accumulates.
---

An AG-UI run arrives as deltas. A message opens, text arrives a fragment at a
time, tool arguments accumulate as partial JSON, state moves by RFC 6902 patch,
and the run may pause to ask a human something. A `Session` is what folds all of
that back into a conversation: a thread id, the messages both sides have said,
and the application state, updated as the events land.

`Session` is the high level. Below it, `RemoteAgent` hands you the events
exactly as the agent sent them, unassembled — the right level for a proxy, a
recorder, or a bridge to another protocol. This page is about the level a user
interface wants.

## Opening one

A session needs a transport and a thread id. Everything a run needs — the
conversation so far, the state so far, a fresh run id — the session builds
itself:

```rust
// src/main.rs
use ag_ui_client::{RunEnd, Session, Update, transport::ReplayTransport};
use ag_ui_core::{Event, TextMessageRole};
use futures_util::StreamExt;

#[tokio::main]
async fn main() {
    // A scripted agent, so this runs with no server and no network.
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
    drop(run);

    assert!(matches!(ended, Some(RunEnd::Success { .. })));
    // The user's turn and the agent's reply are both in the thread now, so the
    // next `send` carries them.
    assert_eq!(session.messages().len(), 2);
}
```

`send` returns a `RunStream`, which borrows the session mutably for as long as
the run is being polled — that borrow is what makes `session.messages()` correct
the moment the run ends. Drop the stream and the session is readable again.

Against a real agent, only the transport changes:

```rust,no_run
// src/main.rs
use ag_ui_client::{Session, Update, transport::HttpTransport};
use futures_util::StreamExt;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let transport = HttpTransport::builder("http://localhost:3000/agent")
        .header("authorization", "Bearer …")
        .connect_timeout(std::time::Duration::from_secs(5))
        .build()?;

    let mut session = Session::<_>::new(transport, "thread-1");

    let mut run = session.send("hello");
    while let Some(update) = run.next().await {
        if let Update::Message(message) = update {
            println!("{:?}", message.change);
        }
    }
    Ok(())
}
```

`HttpTransport` is one of the two transports that ship with the crate — the
other replays a script — and writing a third is one trait method. See
[Transports](/ag-ui-rust/client/transports/).

## The transport bound is on the constructor

`Session<T, S>` carries no `T: Transport` bound. `Session::new`,
`Session::builder` and `SessionBuilder::new` do, along with the methods that
actually make a request.

The immediate payoff is that the mistake worth catching is caught where it is
made. A URL is what a transport is *made from*, so passing one where a transport
belongs reads plausible:

```rust,compile_fail,E0277
use ag_ui_client::Session;

// error[E0277]: the trait bound `str: Transport` is not satisfied
//   — and the `help:` note lists the types that do implement it.
//
// `str` rather than `&str`, because of the blanket `impl Transport for &T`.
let session = Session::<_>::new("http://localhost:3000/agent", "thread-1");
```

Without the bound on the constructor that error would land on the first `send`,
which is usually in another file.

The second payoff is for everything downstream. A bound on a struct definition
is viral: put `T: Transport` on `Session<T, S>` and every application helper
that so much as names the type has to repeat it, including the ones that only
read `messages()`. A view layer never sends anything, so it never names a
transport:

```rust
// src/view.rs
use ag_ui_client::{Session, transport::ReplayTransport};
use ag_ui_core::Message;

/// The agent's last line, for a status bar. No `T: Transport` — this only reads.
fn last_reply<T, S>(session: &Session<T, S>) -> Option<&str> {
    session.messages().iter().rev().find_map(|message| match message {
        Message::Assistant(assistant) => assistant.content.as_deref(),
        _ => None,
    })
}

fn main() {
    let session = Session::<_>::builder(ReplayTransport::new([]), "thread-1")
        .messages(vec![Message::assistant("a-1", "Two open tasks.")])
        .build();

    assert_eq!(last_reply(&session), Some("Two open tasks."));
}
```

Both halves are pinned by tests: a `compile_fail,E0277` doctest on
`Session::new` for the first, and `crates/ag-ui-client/tests/bounds.rs` — written
the way an application writes helpers — for the second. It fails to compile if
the bound ever migrates back onto the type.

## The builder

`Session::builder` is for a session that starts with something already in it:
history loaded from a store, a state document the client owns, the tool set the
agent may call.

```rust
// src/main.rs
use ag_ui_client::{Session, transport::ReplayTransport};
use ag_ui_core::{Message, Tool};
use serde_json::json;

fn main() {
    let session = Session::<_>::builder(ReplayTransport::new([]), "thread-1")
        .messages(vec![
            Message::user("u-1", "what is on the board?"),
            Message::assistant("a-1", "Two open tasks."),
        ])
        .state(json!({ "open": 2, "done": 0 }))
        .tools(vec![Tool::new(
            "add_task",
            "Add a task to the board.",
            json!({
                "type": "object",
                "properties": { "title": { "type": "string" } },
                "required": ["title"],
            }),
        )])
        .build();

    assert_eq!(session.messages().len(), 2);
    assert_eq!(session.raw_state()["open"], 2);
}
```

`context` and `forwarded_props` set the two passthrough fields the protocol
never interprets, and `verify` turns [client-side protocol
verification](/ag-ui-rust/design/verification/) off. It is on by default; off is
for producers whose quirks you have decided to live with. The applier stays
tolerant either way, so what you lose is the diagnosis, not the conversation.

:::caution[Tools travel from the client, on every request]
AG-UI has no tool discovery and no negotiation. An agent cannot ask for a tool
it was not sent, so offering none to an agent that needs one does not produce a
missing-tool error from this crate — it produces the *agent's* own error ("the
client offered no add_task tool", or whatever that agent says), arriving as an
ordinary failed run. That reads like a bug in the agent and is not one. A client
written against no particular agent has to be configured with a tool set the way
it is configured with a URL: `SessionBuilder::tools`, or `Session::set_tools`
from the next run on. See [Tool calls](/ag-ui-rust/server/tools/) for the other
end of it.
:::

## Typed state

`Session<T, S = Value>` has a second parameter: the type the application state
deserializes into. It is inferred from whatever you do with an `Update::State`,
so a session handed to a function expecting `Session<T, Board>`, or a match arm
that keeps the state in a typed local, needs no turbofish. Spell it only when
nothing else names it.

```rust
// src/main.rs
use ag_ui_client::{Session, Update, transport::ReplayTransport};
use ag_ui_core::{Event, PatchOperation};
use futures_util::StreamExt;
use serde::Deserialize;
use serde_json::json;

/// The agent's state, in the client's own type.
#[derive(Clone, Debug, Deserialize, PartialEq)]
struct Board {
    open: u32,
    done: u32,
}

#[tokio::main]
async fn main() {
    let transport = ReplayTransport::new([
        Event::run_started("thread-1", "run-1"),
        Event::state_snapshot(json!({ "open": 2, "done": 0 })),
        Event::state_delta(vec![PatchOperation::replace("/done", 1)]),
        Event::run_finished_success("thread-1", "run-1"),
    ]);

    let mut session = Session::<_, Board>::new(transport, "thread-1");

    let mut latest = None;
    let mut run = session.send("mark one done");
    while let Some(update) = run.next().await {
        if let Update::State(board) = update {
            latest = Some(board);
        }
    }
    drop(run);

    // A snapshot and a patch arrive as the same kind of update; nothing here
    // can tell which was which, and nothing here needs to.
    assert_eq!(latest, Some(Board { open: 2, done: 1 }));
    assert_eq!(session.state(), Some(&Board { open: 2, done: 1 }));
    // The raw JSON is always there too, whether or not it fits the type.
    assert_eq!(session.raw_state()["done"], 1);
}
```

To stream updates, `S` must be `Deserialize + Clone + Unpin`. An `Update::State`
carries the state by value so a view can hold it after the run has moved on;
`#[derive(Clone, Deserialize)]` on a plain struct is all that takes.

State that does not fit the type is not a lost run. `raw_state` is still updated
and correct — only the typed view is out of date, and that arrives as an
`Update::Error` saying so.

:::note
`session.state()` returning `None` is not "the state is empty". It means no
`STATE_*` event has arrived at all, which is a different thing and often a
broken agent. `board-watch` draws the two differently on purpose.
:::

## Starting a run

Three ways in, all returning the same `RunStream`:

| Call | What it sends |
| --- | --- |
| `send(text)` | Appends a user message, then runs. |
| `send_message(message)` | Appends a message of any role, then runs. |
| `run()` | Runs with the conversation exactly as it stands. |

`send` appends the user's turn *before* the request goes out, so it is in
`session.messages()` whatever happens to the run. `run()` is for continuing
after something the client did on its own — a tool result computed locally and
pushed with `push_message`, or a state the client set with `set_state`.

Run ids are generated as `{thread}-run-{n}`. `set_next_run_id` names the next
one explicitly, which servers that key resumption on a run id need and most do
not.

## What a session accumulates

Reading a session takes no run in flight and no transport bound:

| Accessor | What it holds |
| --- | --- |
| `messages()` | The assembled conversation, oldest first, across every run. |
| `state()` | The application state as `S`, once one has arrived that deserializes. |
| `raw_state()` | The same state as JSON. Always current. |
| `reasoning()` | Reasoning messages, kept out of the transcript. |
| `interrupts()` | What the agent is waiting for, if the last run paused. |
| `thread_id()` | The conversation this session is part of. |
| `applier()` | The state machine underneath, for a view that wants the raw materialised form. |
| `agent()` | The `RemoteAgent`, for dropping down a level. |

## Answering a pause

A run does not only succeed or fail. It can pause: the agent finishes with an
interrupt outcome listing what it needs a human to decide, and the conversation
continues when the client sends the answers back.

```rust
// src/main.rs
use ag_ui_client::{RunEnd, Session, Update, transport::ReplayTransport};
use ag_ui_core::{Event, Interrupt};
use futures_util::StreamExt;
use serde_json::json;

#[tokio::main]
async fn main() {
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

    let mut session = Session::<_>::new(transport, "thread-1");

    let mut paused = Vec::new();
    let mut run = session.send("delete the staging database");
    while let Some(update) = run.next().await {
        if let Update::Interrupt(interrupt) = update {
            paused.push(interrupt);
        }
    }
    drop(run);

    // The same interrupts are on the session until the next run starts.
    assert_eq!(session.interrupts().len(), 1);

    // Ask the human, then answer the agent. `session.cancel(&interrupt)` is
    // the other half: the human said no.
    let mut ended = None;
    let mut resumed = session.resume(&paused[0], json!({ "approved": true }));
    while let Some(update) = resumed.next().await {
        if let Update::Done(end) = update {
            ended = Some(end);
        }
    }
    drop(resumed);

    assert!(matches!(ended, Some(RunEnd::Success { .. })));
}
```

A run can pause on more than one decision at once, and they are answered
together, in **one** request. Answering one per request never terminates,
because the resumed run supersedes the paused one and the agent only sees what
the resuming request carries — anything left unanswered is dropped.
`resume_many` takes the lot, and `ResumeBuilder` collects them one decision at a
time:

```rust
// src/main.rs
use ag_ui_client::interrupts::ResumeBuilder;
use ag_ui_core::{Interrupt, ResumeStatus};
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
    // `session.resume_many(entries)` sends both answers in one request.
}
```

The answer's shape is the agent's business; when the interrupt carried a
`responseSchema`, the payload should satisfy it. The one shape this crate knows
about is `resolve_with_edits`, which writes the `editedArgs` key that agents
advertising `approveWithEdits` expect, so callers do not have to remember it.

The resumed run gets a new run id, not the paused one: it emits its own
`RUN_STARTED`, and reusing the finished run's id would make two runs in one
thread indistinguishable in a log.

The agent's half of this is on [Human in the
loop](/ag-ui-rust/server/interrupts/).

## Stopping

`Session::cancel` answers an interrupt; it does not stop a run. There is no
method that does, because polling the stream is what pulls bytes — so letting go
of it is the whole of client-side cancellation. That the drop reaches the far
end is not obvious from the client, so `board-watch` proves it from the other
side: its integration test drops a run mid-stream against an agent that reports
its own cancellation state as its future exits, and asserts the run was
cancelled rather than merely dropped. The session stays usable afterwards, and
the next run is a run like any other.

## Next

- [The update stream](/ag-ui-rust/client/updates/) — every `Update` variant, and
  the three ways a run ends.
- [Rendering a run](/ag-ui-rust/client/rendering/) — why arrival order is the
  only nesting there is, and what a renderer that ignores it gets wrong.
- [`Session`](/ag-ui-rust/api/ag_ui_client/session/struct.Session.html) in the
  API docs.
