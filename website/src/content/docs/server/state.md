---
title: Shared state
description: Publishing an agent's state to the client, and how snapshots and JSON Patch deltas are chosen between.
---

An AG-UI run carries a piece of shared state that the client mirrors: the board an agent is
editing, the form it is filling in, the document it is drafting. The client sends its copy
in `RunAgentInput.state`, the agent changes it, and every change goes back out as a
`STATE_SNAPSHOT` or a `STATE_DELTA`.

On the server side that state is a typed value — `Agent::State`, whatever your struct is —
and the events are chosen for you.

## Reading and writing it

```rust
use ag_ui::{Event, EventType, RunAgentInput};
use ag_ui::server::RunContext;
use serde::{Deserialize, Serialize};

#[derive(Default, Serialize, Deserialize)]
struct Doc {
    step: u32,
    notes: Vec<String>,
}

fn main() -> ag_ui::server::Result<()> {
    let (mut ctx, mut events) = RunContext::<Doc>::new(RunAgentInput::new("t", "r"))?;

    // Mutate and publish in one call.
    ctx.update_state(|doc| {
        doc.step = 1;
        doc.notes.push("the document the user is editing".repeat(8));
    })?;

    // Or mutate now and publish when you are ready.
    ctx.state_mut().step = 2;
    ctx.publish_state()?;

    // A publish with nothing to say emits nothing at all.
    ctx.publish_state()?;

    let types: Vec<EventType> = events.drain().iter().map(Event::event_type).collect();
    assert_eq!(types, [EventType::StateSnapshot, EventType::StateDelta]);
    assert_eq!(ctx.state().step, 2);
    Ok(())
}
```

Five methods, and what separates most of them is only when the event goes out:

| Method | What it does |
| --- | --- |
| `state()` | the typed state, as of the last publish |
| `state_mut()` | the typed state, mutably. Emits nothing |
| `publish_state()` | sends whatever `state_mut` left behind. A no-op when nothing changed |
| `update_state(\|s\| …)` | mutate and publish, in one call |
| `set_state(&s)` | replace the whole value and publish |

## Snapshot or delta

Sending the whole state on every change is wasteful for a large document that gained one
field; sending a patch is wasteful for a small document that changed completely. The
publisher decides per publish:

1. the first publish of a run is always a `STATE_SNAPSHOT` — the client's copy may have
   drifted, and a patch against an unknown base is inapplicable;
2. afterwards the state is diffed against the last published snapshot with
   [RFC 6902](https://datatracker.ietf.org/doc/html/rfc6902) and sent as `STATE_DELTA`;
3. unless the serialized patch is no smaller than the serialized snapshot, in which case it
   snapshots instead.

That is why the run above produced a snapshot and then a delta: the second change touched
one small field of a value dominated by its `notes`.

`StateManager` is the same logic on its own, for a transport or a test that needs it outside
a run:

```rust
use ag_ui::PatchOperation;
use ag_ui::server::{StateManager, StatePublish};
use serde_json::json;

fn main() -> ag_ui::server::Result<()> {
    let mut states = StateManager::new();
    let notes = "the document the user is editing, at some length";

    // First publish: a snapshot, whatever the size.
    let first = states.publish(json!({"step": 1, "notes": notes}))?;
    assert!(matches!(first, StatePublish::Snapshot(_)));

    // One field of a large document: a patch, because it is smaller.
    assert_eq!(
        states.publish(json!({"step": 2, "notes": notes}))?,
        StatePublish::Delta(vec![PatchOperation::replace("/step", 2)])
    );

    // Nothing moved: nothing to send.
    assert_eq!(
        states.publish(json!({"step": 2, "notes": notes}))?,
        StatePublish::Unchanged
    );

    // A small document changing wholesale: back to a snapshot, because the
    // patch would be bigger than the state it describes.
    let mut small = StateManager::new();
    small.publish(json!({"a": 1}))?;
    assert_eq!(
        small.publish(json!({"b": 2}))?,
        StatePublish::Snapshot(json!({"b": 2}))
    );
    Ok(())
}
```

`reset()` forgets the last publish so the next one is a snapshot again — what you want after
emitting a `STATE_SNAPSHOT` by hand, or after a reconnect where the client's copy is no
longer known.

## What arrives, and what does not

The state the agent starts with is `RunAgentInput.state`, deserialized into `S`. A `null` or
an empty object — what clients send for "no state yet" — becomes `S::default()` rather than
a deserialization error, so a stateless agent works against every client. A state that is
present but does not fit `S` is an error, and because the run driver decodes before it hands
the context over, it reaches the client as a `RUN_ERROR` rather than as a panic.

```rust
use ag_ui::RunAgentInput;
use ag_ui::server::{Error, RunContext};
use serde::{Deserialize, Serialize};
use serde_json::json;

#[derive(Debug, Default, Serialize, Deserialize)]
struct Counter {
    clicks: u32,
}

fn main() {
    // No state yet: the default, not a failure.
    let mut input = RunAgentInput::new("t", "r");
    input.state = json!({});
    let (ctx, _events) = RunContext::<Counter>::new(input).expect("an empty state is fine");
    assert_eq!(ctx.state().clicks, 0);

    // A state that does not fit.
    let mut input = RunAgentInput::new("t", "r");
    input.state = json!({"clicks": "three"});
    let error = RunContext::<Counter>::new(input).expect_err("should not decode");
    assert!(matches!(error, Error::Json(_)));
    assert_eq!(error.code(), "SERIALIZATION");
}
```

## Publishing while something is open

A message or tool-call handle borrows two *fields* of the run context — the event sink and
the state — rather than the context itself. So the state stays reachable for as long as a
call is open, and a tool can announce itself, do its work, and only then report:

```rust
use ag_ui::{Event, EventType, RunAgentInput};
use ag_ui::server::RunContext;
use serde::{Deserialize, Serialize};
use serde_json::json;

#[derive(Default, Serialize, Deserialize)]
struct Board {
    tasks: Vec<String>,
}

fn main() -> ag_ui::server::Result<()> {
    let (mut ctx, mut events) = RunContext::<Board>::new(RunAgentInput::new("t", "r"))?;

    let mut call = ctx.tool_call("add_task")?;
    call.args_json(&json!({"title": "ship it"}))?;

    call.state_mut().tasks.push("ship it".to_owned());
    call.publish_state()?;

    call.result_json(&json!({"ok": true}))?;

    let types: Vec<EventType> = events.drain().iter().map(Event::event_type).collect();
    assert_eq!(
        types,
        [
            EventType::ToolCallStart,
            EventType::ToolCallArgs,
            // The board moving, inside the call's brackets.
            EventType::StateSnapshot,
            EventType::ToolCallEnd,
            EventType::ToolCallResult,
        ]
    );
    Ok(())
}
```

This is legal because the `STATE_*` family is **unordered** on the wire: a state event
belongs to no bracket and may appear anywhere in a run. The server's ordering verifier
agrees, and so does the client applier.

It is worth insisting on because the alternative produces the same events in a worse order.
An earlier draft of this crate gave handles only the event sink, which left the state
unreachable while anything was open and forced every agent to mutate *before* announcing the
call it was mutating for. Same five events, reordered — and the order is what decides whether
a client can watch a call land or only see it already done. Holding the state beside the sink
widens what a handle can reach without widening what it can open: there is still no run
context behind it to open a second block with.

:::note
Because `STATE_*` is unordered, a client cannot infer *which* tool call a state change
belongs to from position alone. If that association matters to your UI, put it in the call's
result or in the state itself rather than relying on the interleaving.
:::

## One publish per change, not one per run

`examples/task-board` publishes once per task added, so a message adding two tasks makes the
server choose an encoding twice — the first publish is a snapshot and the second is a delta
only if the patch comes out smaller than the whole board. A client mirroring the state has to
survive both, and the example's `tests/flows.rs` pins both. Batching every change into one
publish at the end of a run would be fewer events and a worse experience: the client would
see nothing until the run was over.

## API

- [`RunContext::state`](/ag-ui-rust/api/ag_ui/server/struct.RunContext.html#method.state),
  [`state_mut`](/ag-ui-rust/api/ag_ui/server/struct.RunContext.html#method.state_mut),
  [`publish_state`](/ag-ui-rust/api/ag_ui/server/struct.RunContext.html#method.publish_state),
  [`update_state`](/ag-ui-rust/api/ag_ui/server/struct.RunContext.html#method.update_state),
  [`set_state`](/ag-ui-rust/api/ag_ui/server/struct.RunContext.html#method.set_state)
- [`ag_ui::server::StateManager`](/ag-ui-rust/api/ag_ui/server/struct.StateManager.html) and
  [`StatePublish`](/ag-ui-rust/api/ag_ui/server/enum.StatePublish.html)
- [`ag_ui::PatchOperation`](/ag-ui-rust/api/ag_ui/enum.PatchOperation.html)
- The client side of the same story: [Sessions](/ag-ui-rust/client/session/)
