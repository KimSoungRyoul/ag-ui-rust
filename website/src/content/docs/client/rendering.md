---
title: Rendering a run
description: Why arrival order is the only nesting a run has, what buffering by entity costs, and the two renderings board-watch ships.
---

The update stream is per *event*, not per entity. A reply that streams in forty
deltas is forty `Update::Message`s under one id. Two tool calls in flight —
which a model produces whenever it asks for two things at once — interleave, and
only their ids separate them.

That is the fact a renderer is built on, and getting it wrong is quiet. Nothing
crashes; the run simply draws in an order it did not happen in. This page is
about what the order carries and what it costs to give it up.

## Consecutive updates need not belong together

Two calls open before either closes, and their argument fragments alternate on
the wire. The client does not reorder them, so they alternate in the update
stream too:

```rust
// src/main.rs
use ag_ui_client::{MessageChangeKind, Session, Update, transport::ReplayTransport};
use ag_ui_core::Event;
use futures_util::StreamExt;

#[tokio::main]
async fn main() {
    let transport = ReplayTransport::new([
        Event::run_started("thread-1", "run-1"),
        Event::tool_call_start("call-1", "add_task"),
        Event::tool_call_start("call-2", "add_task"),
        Event::tool_call_args("call-1", r#"{"title":"#),
        Event::tool_call_args("call-2", r#"{"title":"#),
        Event::tool_call_args("call-1", r#""write it down"}"#),
        Event::tool_call_args("call-2", r#""read it back"}"#),
        Event::tool_call_end("call-1"),
        Event::tool_call_end("call-2"),
        Event::run_finished_success("thread-1", "run-1"),
    ]);

    let mut session = Session::<_>::new(transport, "thread-1");
    let updates: Vec<_> = session.send("add two things").collect().await;

    let fragments: Vec<String> = updates
        .iter()
        .filter_map(|update| match update {
            Update::Message(message) => match &message.change {
                MessageChangeKind::ToolCallArgs { tool_call_id, .. } => {
                    Some(tool_call_id.to_string())
                }
                _ => None,
            },
            _ => None,
        })
        .collect();

    // Four fragments, two calls, strictly alternating. A renderer that appends
    // each delta to "the call it is in the middle of" writes one garbled line.
    assert_eq!(fragments, ["call-1", "call-2", "call-1", "call-2"]);
}
```

So the id on each change is not decoration. It is the only thing that says which
call a fragment belongs to.

## Arrival order is the only nesting

The protocol has no containment. It has a sequence, and where an event lands in
that sequence is all the wire says about what was open when it arrived.

The case that makes this concrete: an agent that does a tool's work *while the
call is open* publishes state between `TOOL_CALL_ARGS` and `TOOL_CALL_END` —
which `ag-ui-server`'s handles support, and which the protocol allows because
`STATE_*` is unordered. The `Update::State` that comes out carries no mention of
the call.

That is not an omission waiting for a field. Under parallel calls two calls are
open at once and the wire itself does not say which one the state belongs to, so
any attribution would be invented rather than reported. **The ordering is the
contract.**

A renderer that draws in arrival order therefore shows what happened. One that
buffers by entity is choosing to reorder.

## What buffering costs

Buffering a call's arguments so the whole call can be drawn on one line is a
reasonable thing to want — it reads better. The price is that the line cannot be
written until the call closes, so everything that arrived while the call was
open draws *before* it.

Here are both renderings over one update stream, so the difference is the
drawing and nothing else:

```rust
// src/render.rs
use ag_ui_client::{MessageChangeKind, Session, Update, transport::ReplayTransport};
use ag_ui_core::{Event, ToolCallId};
use futures_util::StreamExt;
use serde_json::json;

/// The tail of a call id — enough to tell two apart within one transcript.
fn short(id: &ToolCallId) -> &str {
    let id = id.as_str();
    id.rsplit('-').next().unwrap_or(id)
}

/// One line per update, in arrival order, each tool line naming its call.
fn in_order(update: &Update, out: &mut Vec<String>) {
    match update {
        Update::Message(message) => match &message.change {
            MessageChangeKind::ToolCallStarted { tool_call_id, name } => {
                out.push(format!("call {name} ({})", short(tool_call_id)));
            }
            // Named, because in arrival order two calls' fragments are adjacent
            // and the id is the only thing separating them.
            MessageChangeKind::ToolCallArgs { tool_call_id, delta } => {
                out.push(format!("args ({}) {delta}", short(tool_call_id)));
            }
            // `ToolCallEnded` carries only the id — the name arrived on
            // `ToolCallStarted`, so a renderer that wants it here keeps a map.
            MessageChangeKind::ToolCallEnded { tool_call_id } => {
                out.push(format!("end  ({})", short(tool_call_id)));
            }
            _ => {}
        },
        Update::State(state) => out.push(format!("state {state}")),
        _ => {}
    }
}

/// The whole call on one line, however many events it took — which means the
/// line is written when the call *closes*.
#[derive(Default)]
struct Grouped {
    open: Vec<(ToolCallId, String, String)>,
}

impl Grouped {
    fn draw(&mut self, update: &Update, out: &mut Vec<String>) {
        match update {
            Update::Message(message) => match &message.change {
                MessageChangeKind::ToolCallStarted { tool_call_id, name } => {
                    self.open
                        .push((tool_call_id.clone(), name.clone(), String::new()));
                }
                MessageChangeKind::ToolCallArgs { tool_call_id, delta } => {
                    if let Some(call) = self.open.iter_mut().find(|call| &call.0 == tool_call_id) {
                        call.2.push_str(delta);
                    }
                }
                MessageChangeKind::ToolCallEnded { tool_call_id } => {
                    if let Some(at) = self.open.iter().position(|call| &call.0 == tool_call_id) {
                        let (_, name, args) = self.open.remove(at);
                        out.push(format!("call {name} {args}"));
                    }
                }
                _ => {}
            },
            Update::State(state) => out.push(format!("state {state}")),
            _ => {}
        }
    }
}

#[tokio::main]
async fn main() {
    // An agent that publishes state from inside its own call, which is what
    // `examples/task-board` does and what the protocol allows.
    let transport = ReplayTransport::new([
        Event::run_started("thread-1", "run-1"),
        Event::tool_call_start("call-1", "add_task"),
        Event::tool_call_args("call-1", r#"{"title":"draft "#),
        Event::state_snapshot(json!({ "open": 1 })),
        Event::tool_call_args("call-1", r#"the agenda"}"#),
        Event::tool_call_end("call-1"),
        Event::run_finished_success("thread-1", "run-1"),
    ]);

    let mut session = Session::<_>::new(transport, "thread-1");
    let updates: Vec<_> = session.send("add one thing").collect().await;

    let mut ordered = Vec::new();
    let mut grouped = Vec::new();
    let mut state = Grouped::default();
    for update in &updates {
        in_order(update, &mut ordered);
        state.draw(update, &mut grouped);
    }

    // Arrival order puts the state between the call's arguments and its end,
    // which is where the wire put it.
    assert_eq!(
        ordered,
        [
            r#"call add_task (1)"#,
            r#"args (1) {"title":"draft "#,
            r#"state {"open":1}"#,
            r#"args (1) the agenda"}"#,
            r#"end  (1)"#,
        ]
    );

    // Grouping writes the call when it closes, so the state that happened
    // during it is already on screen.
    assert_eq!(
        grouped,
        [
            r#"state {"open":1}"#,
            r#"call add_task {"title":"draft the agenda"}"#,
        ]
    );
}
```

Neither is more correct. The grouped view reads better and reorders what
happened inside a call; the faithful one is noisier and can show it. What cannot
be had is a call drawn as one line **and** kept in order, because the line
cannot be written until the call closes.

:::tip
Legibility under parallel calls comes from tagging each line with the call id,
not from buffering. Without the tag the faithful rendering is unreadable when
two calls are open; with it, it is fine. That was the wrong conclusion the first
time this was written down in this repo, and the correction is pinned by a test
in `examples/board-watch/tests/client.rs`.
:::

## The two renderings board-watch ships

`examples/board-watch` is a terminal client for any AG-UI agent, and it ships
both. Against `task-board`, which publishes state from inside its call, the
default grouped view draws:

```text
  state  1 open · 0 done
  call   add_task {"title":"draft the agenda"}
  result {"id":1,"title":"draft the agenda"}
```

and `--in-order` draws:

```text
  call   add_task (1)
  args   (1) {"title":"draft the agenda"}
  state  1 open · 0 done
  end    add_task (1)
  result {"id":1,"title":"draft the agenda"}
```

Its advice is to pick the grouped view to read a conversation and `--in-order`
to debug one. Both are driven by the integration tests — the same function the
binary runs, with a scripted `&[u8]` for a keyboard and a `Vec<u8>` for a screen
— so the transcripts in its README are assertions rather than illustrations. The
tests assert the reordering in one direction and the faithful order in the
other, which is how a change to either renderer stops being silent.

[board-watch](/ag-ui-rust/examples/board-watch/) has the rest of it.

## What you do not have to handle

Some of what looks like a rendering problem has already been dealt with before
the update reaches you.

**Chunk events.** A provider adapter that cannot bracket its output sends
`*_CHUNK` events, which carry their id only on the first one. The normalizer
turns those into explicit `Started` / `Content` / `Ended` triples, so a renderer
only ever sees the bracketed form and never has to remember an id across events.

**Unterminated messages.** If the producer never closes its last message, the
end of the stream closes it: the terminators the normalizer still owes are
emitted before the run ends. A view that hides its typing indicator on
`MessageChangeKind::Ended` would otherwise spin forever.

**Malformed streams.** An event that breaks an ordering rule is reported as an
`Update::Error` and — unless it is the event that ends the run, which is applied
anyway so the caller is not left waiting — **not applied**. So the conversation
never contains state assembled from a broken stream. Turning verification off
applies it regardless; what that costs is the diagnosis, not the conversation.

**Reasoning lifecycle.** The protocol brackets a thought twice — `REASONING_START`
opens the block and `REASONING_MESSAGE_START` the message inside it, both under
the same id. `ReasoningChangeKind::Started` and `Ended` arrive **once** per id
regardless, so a view that prints a finished thought prints it once and needs no
dedupe.

## Redraw hints

- `MessageUpdate::index` is the row that changed. Redraw one row.
- `Update::Messages` means `MESSAGES_SNAPSHOT` replaced the conversation.
  Messages may have *disappeared*, so redraw all of it.
- `Update::State` carries the new state by value, so a view can hold it after
  the run has moved on.
- `Update::Error` is not terminal. Print it and keep going; the run says when it
  is over.
- `Update::Done` is the last update of a run, on every path out. It is where the
  input goes live again — see [the update
  stream](/ag-ui-rust/client/updates/#the-three-ways-a-run-ends).

## Next

- [The update stream](/ag-ui-rust/client/updates/) — the variants themselves.
- [Sessions](/ag-ui-rust/client/session/) — what is accumulating while you draw.
- [`MessageChangeKind`](/ag-ui-rust/api/ag_ui_client/apply/enum.MessageChangeKind.html)
  in the API docs.
