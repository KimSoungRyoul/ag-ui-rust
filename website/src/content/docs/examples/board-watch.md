---
title: board-watch (client)
description: A read through the terminal client that consumes the task-board agent over a real port.
---

[task-board](/ag-ui-rust/examples/task-board/) was an agent with a client attached.
`board-watch` is the other way round: the application here is the **client**, and the
server in the same crate exists only to give it streams worth surviving.

That inversion is the point. A client written against one agent you also wrote is not
tested against anything, so this one is written against no particular agent, and the
transcripts below were recorded against a deliberately awkward backend — chunked output,
interleaved parallel calls, pauses on several decisions at once, a run that never finishes,
and streams the protocol forbids.

[Read the source on GitHub](https://github.com/KimSoungRyoul/ag-ui-rust/tree/main/examples/board-watch).

| Command | What it is |
| --- | --- |
| `watch` | The application: send a line, render the run, answer what it pauses on, draw the board. |
| `trace` | The same conversation one level down — events exactly as they arrived. |
| `replay` | The whole client with the network taken out, over a recorded fixture. |
| `serve-fake` | The awkward backend the transcripts were recorded against. |

It needs no key and no network beyond loopback.

## Running it

Two terminals. The backend, on port 8090:

```sh
cargo run -p board-watch -- serve-fake
```

Then the client, pointed at it:

```sh
cargo run -p board-watch -- watch --url http://127.0.0.1:8090/agent --fragments
```

Scenarios are chosen by the first word you type, so a transcript names what it exercised:
`chunks`, `call`, `parallel`, `mixed`, `approve`, `busy`, `slow`, `fail`, and anything else
for a well-behaved turn.

`--fragments` brackets every delta as it arrives, so what the client had to reassemble is
visible in the output. `--in-order` draws one line per update instead of grouping a tool
call onto one line — which turns out to be a real trade, and it gets its own section below.

## Chunked streams

A provider adapter often cannot bracket its output: the upstream API does not say a message
ended until the next one begins. So it sends `*_CHUNK` events, which fold start, content
and end into one and carry their id **only on the first**. Five events, one message:

```text
> chunks
  text   [Chunked text arrives in frag][ments, and the client rejoins ][them — emoji included: 👩][‍][💻.]
  done   success
```

Remembering that id is the first thing the client does, before anything else looks at the
stream:

```rust
use ag_ui::client::chunks::normalize_all;
use ag_ui::{Event, EventType, MessageId};

let events = normalize_all([
    Event::text_message_chunk(Some(MessageId::new("msg-1")), Some("Hel".into())),
    Event::text_message_chunk(None, Some("lo".into())),
])
.unwrap();

let types: Vec<EventType> = events.iter().map(Event::event_type).collect();
assert_eq!(
    types,
    [
        EventType::TextMessageStart,
        EventType::TextMessageContent,
        EventType::TextMessageContent,
        EventType::TextMessageEnd,
    ]
);
```

The last three fragments in that transcript split a ZWJ emoji between its parts. Every
fragment is valid UTF-8 on its own — a Rust `String` cannot be otherwise — but the
*grapheme* only exists once they are rejoined.

## Arguments split mid-escape

Tool arguments are worse, because they are JSON split at arbitrary byte offsets:

```text
> call
  call   add_task [{"no][te":"line\][nbreak","ti][tle":"ship ][the SDK","depth":3}]
  result {"id":1,"title":"ship the SDK"}
```

The seam after `line\` is the case every adapter gets wrong once: the backslash and the `n`
it escapes arrive in different events, so anything that parses a fragment on its own sees
invalid JSON. What the client hands over is the whole thing, which parses:

```rust
use ag_ui::client::{MessageChangeKind, Session, Update, transport::ReplayTransport};
use ag_ui::Event;
use futures_util::StreamExt;

#[tokio::main]
async fn main() {
    let transport = ReplayTransport::new([
        Event::run_started("thread-1", "run-1"),
        Event::tool_call_start("call-1", "add_task"),
        Event::tool_call_args("call-1", r#"{"no"#),
        Event::tool_call_args("call-1", r#"te":"line\"#),
        Event::tool_call_args("call-1", r#"nbreak","ti"#),
        Event::tool_call_args("call-1", r#"tle":"ship the SDK"}"#),
        Event::tool_call_end("call-1"),
        Event::run_finished_success("thread-1", "run-1"),
    ]);

    let mut session = Session::<_>::new(transport, "thread-1");
    let mut args = String::new();

    let mut run = session.send("call");
    while let Some(update) = run.next().await {
        if let Update::Message(message) = update {
            if let MessageChangeKind::ToolCallArgs { delta, .. } = message.change {
                args.push_str(&delta);
            }
        }
    }
    drop(run);

    // No fragment above parses on its own. The whole does.
    let parsed: serde_json::Value = serde_json::from_str(&args).unwrap();
    assert_eq!(parsed["note"], "line\nbreak");
    assert_eq!(parsed["title"], "ship the SDK");
}
```

## Two calls at once, and a trade with no good answer

A model that asks for two things at once produces interleaved events —
`args(a) args(b) args(a) end(a) end(b)`. The obvious renderer, which prints a prefix on
`ToolCallStarted` and a newline on `ToolCallEnded`, produces one garbled line. This one
buffers by call id:

```text
> parallel
  call   add_task [{"title":]["write it down"}]
  call   add_task [{"title":]["read it back"}]
  result {"id":1,"title":"write it down"}
  result {"id":2,"title":"read it back"}
  state  2 open · 0 done
```

That buffering has a price. A call is printed when it *closes*, so anything the agent
emitted while the call was open prints **before** it — and `task-board` publishes state
from inside its call, so the grouped view reorders exactly the thing
[task-board](/ag-ui-rust/examples/task-board/) went to trouble to get right:

```text
  state  1 open · 0 done
  call   add_task {"title":"draft the agenda"}
  result {"id":1,"title":"draft the agenda"}
```

`--in-order` takes the other side of the trade — one line per update, in arrival order,
each tool line tagged with its call:

```text
  call   add_task (1)
  args   (1) {"title":"draft the agenda"}
  state  1 open · 0 done
  end    add_task (1)
  result {"id":1,"title":"draft the agenda"}
```

That is where the wire put it, and arrival order *is* the nesting: an `Update::State`
carries no association with the call it arrived during, because under parallel calls two
calls are open and the wire does not attribute the state either. Inventing an association
would be a guess reported as a fact.

So what cannot be had is a call drawn as one line **and** kept in order, because the line
cannot be written until the call closes. Legibility under parallel calls comes from the id
tag instead. Pick the grouped view to read a conversation and `--in-order` to debug one.

## Pausing on more than one thing

A run can pause on several decisions at once, and they are answered in **one** request.
Answering one per request never terminates, because the agent only ever sees what the
resuming request carries:

```text
> approve
  pause  approve-budget · Approve the budget?
  pause  confirm-date · Confirm the date?
  done   interrupted on 2
  approve approve-budget [y/N] y
  answer approve-budget · approved
  approve confirm-date [y/N] n
  answer confirm-date · declined
  text   Declined: confirm-date. Nothing booked.
  done   success
```

`--approve` and `--decline` answer everything unattended, for scripts.

A run can also do work *and then* pause. The `busy` scenario adds two tasks, publishes
state and only then asks — and resuming carries what the first half produced rather than
starting over. [Human in the loop](/ag-ui-rust/server/interrupts/) is the reference.

## Stopping

Polling the stream is what pulls bytes, so letting go of it is the whole of client-side
cancellation. `--stop-after N` drops the run stream after N updates:

```text
> slow
  text   working on it, this will take a while
  stop   dropped the stream after 3 updates
```

The agent is thirty seconds into a call it will never finish, and the drop reaches it: the
integration test asserts the run's cancellation token had been tripped by the time the
agent's future exited. The session stays usable — the next run is a run like any other.
[Errors and cancellation](/ag-ui-rust/server/errors/) covers the other end of that.

## Streams the protocol forbids

`ag_ui::server` will not emit a malformed stream, which is what it is for. So the fake
backend's `/raw` endpoint frames the bytes by hand with `SseFormatter`, the way a producer
in another language does, and the client's own verifier is what has to catch it:

```text
$ board-watch watch --url http://127.0.0.1:8090/raw/unbracketed
> go
  error  protocol violation: TEXT_MESSAGE_CONTENT for message "ghost", which was never opened
  done   success
```

The offending event is *not* applied — the conversation holds one message, the user's.
`--no-verify` applies it anyway; the applier is tolerant either way, and what verification
costs is the diagnosis rather than the conversation.

Note also that the run still ends `success`. An `Update::Error` is not necessarily fatal,
so a client that reads only `Update::Done` misses this entirely — which is the kind of
thing this example exists to make visible. [Verification](/ag-ui-rust/design/verification/)
has the rules.

## Against a real agent

`task-board` from round one, unmodified, on port 8080:

```sh
cargo run -p task-board -- serve
cargo run -p board-watch -- watch --url http://127.0.0.1:8080/agent \
    --tools examples/board-watch/fixtures/task-board-tools.json
```

```text
> add draft the agenda, book the room
  think  adding 2 task(s)
  state  1 open · 0 done
  call   add_task {"title":"draft the agenda"}
  result {"id":1,"title":"draft the agenda"}
  state  2 open · 0 done
  call   add_task {"title":"book the room"}
  result {"id":2,"title":"book the room"}
  text   Added #1 draft the agenda, #2 book the room. 2 open · 0 done
  call   render_a2ui {"surfaceId":"task-board"}
  surface
    Workshop board
    2 open · 0 done
    [ ] #1 draft the agenda
    [ ] #2 book the room
  done   success
┌ board
│ 2 open · 0 done
│ [ ] #1 draft the agenda
│ [ ] #2 book the room
└ run board-run-1 · 8 messages · surface task-board (6)
```

Three things in that panel are worth naming. The board is this client's **own** view model,
declared in `src/board.rs` independently of the agent's — a front-end team is handed a JSON
shape, not a crate. The surface is drawn by walking the A2UI component tree and resolving
each binding. And `surface task-board (6)` is recovered from the **conversation**, not from
the tool result that happened to carry it.

### Why `--tools` exists

In AG-UI the *client* offers the tools and the agent picks from them. There is no
discovery: nothing lets an agent ask for a tool it was not sent, and an agent handed none
simply fails.

```text
$ board-watch watch --url http://127.0.0.1:8080/agent      # no --tools
> add anything
  think  adding 1 task(s)
  error  run failed: agent error: the client offered no add_task tool
  done   failed [AGENT_ERROR] agent error: the client offered no add_task tool
```

That reads like a bug in the agent and is not one. A client not written against one
specific agent therefore has to be *configured* with a tool set, the way it is configured
with a URL. The bundled fixture is exactly what `task-board` offers, and a test asserts it
has not drifted.

## One level down, and offline

`trace` prints the events unassembled — what a proxy, a recorder or a person debugging a
stream wants. It also does the human-in-the-loop round trip with no session at all:
`interrupts_of` reads what the run paused on, `resume_run` builds the request that answers
it.

```sh
cargo run -p board-watch -- trace --url http://127.0.0.1:8090/agent --approve approve
```

`Transport` is a trait, so a fixture on disk substitutes for a server and nothing above it
changes:

```sh
cargo run -p board-watch -- replay examples/board-watch/fixtures/chunked-run.json --fragments
```

Refresh the recording with `fixtures/capture.py` against a running `serve-fake`.

## Against a real model

Optional, and nothing in the default path touches it. The workspace's LLM agent is an AG-UI
endpoint like any other, so the client needs no changes:

```sh
export GEMINI_API_KEY=…                    # or AG_UI_LLM_API_KEY
cargo run -p ag-ui-e2e --example llm_agent
cargo run -p board-watch -- watch --url http://127.0.0.1:8080/agent --fragments
```

`tests/live.rs` does the same thing as a test. It is `#[ignore]`, so `cargo test` and CI
never touch the network, and it skips rather than fails when there is no key:

```sh
cargo test -p board-watch --test live -- --ignored --nocapture
```

What that buys is the one part of chunk handling a fixture cannot fake: deltas fragmented
by an actual provider on its own schedule rather than by this crate's idea of one.

## The code

| File | What is in it |
| --- | --- |
| `src/watch.rs` | The driver and both renderers, generic over input and output |
| `src/view.rs` | The panel, the A2UI walk, and helpers that name a `Session` without bounding its transport |
| `src/board.rs` | The client's own view model of the agent's state |
| `src/trace.rs` | The unassembled view, and resume without a session |
| `src/fake.rs` | The awkward agent and the hand-framed illegal streams |
| `src/main.rs` | The CLI |
| `tests/client.rs` | Every flow above, against both backends on real sockets |
| `tests/live.rs` | The same client against a real model, `#[ignore]`d |

`src/fake.rs` is the one to read if you are writing a client of your own. It is a catalogue
of what real producers do, and each entry has a test named after the behaviour it pins —
`tool_arguments_split_mid_escape_reassemble_into_valid_json`,
`an_event_published_inside_a_call_loses_its_nesting`,
`a_truncated_stream_ends_the_run_rather_than_hanging`.

```sh
cargo test -p board-watch
```

## Next

- [Sessions](/ag-ui-rust/client/session/) and
  [The update stream](/ag-ui-rust/client/updates/) — the API this example is built on.
- [Rendering a run](/ag-ui-rust/client/rendering/) — the grouping trade, as a reference
  rather than a transcript.
- [Transports](/ag-ui-rust/client/transports/) — what `replay` is doing.
- [task-board](/ag-ui-rust/examples/task-board/) — the agent this one is pointed at.
