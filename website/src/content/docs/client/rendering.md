---
title: Rendering a run
description: Render interleaved message, tool and subagent updates without inventing ordering or completion.
---

AG-UI events arrive in order, but multiple messages and tool calls may be open together.
Use message IDs and tool call IDs to identify what changed. A state event between two
argument fragments has no implicit association with that tool call: it updates the run's
shared state, including when its source is a subagent.

`MessageUpdate` includes the current assembled message and a `MessageChangeKind`.
`Started`, `Content`, and `Ended` track text. `ToolCallStarted`, `ToolCallArgs`,
`ToolCallEnded`, and `ToolResult` describe tools. `Activity` and `EncryptedValue` report
other message updates. `Aborted` stops an incomplete text indicator without claiming
that the producer successfully completed the message.

```rust
use ag_ui::client::{MessageChangeKind, Update};
use serde_json::Value;

fn render(update: Update<Value>) {
    match update {
        Update::Message(message) => match message.change {
            MessageChangeKind::Content { delta } => print!("{delta}"),
            MessageChangeKind::Ended => println!(),
            MessageChangeKind::Aborted => println!(" [interrupted]"),
            MessageChangeKind::ToolCallArgs { tool_call_id, delta } => {
                println!("[{tool_call_id}] {delta}");
            }
            _ => {}
        },
        Update::State(state) => println!("state: {state}"),
        Update::Error(error) => eprintln!("{error}"),
        Update::Done(end) => println!("{end:?}"),
        _ => {}
    }
}
```

Drawing each delta immediately preserves arrival order. Buffering a tool call until its
arguments finish makes a cleaner terminal line, but moves that line after everything
that arrived while it was open. This is a rendering choice, not a different protocol
execution order. [board-watch](/ag-ui-rust/examples/board-watch/) demonstrates both styles.

## Grouping subagents

`Update::Subagent` carries lifecycle only. The child's text remains an ordinary
`Update::Message`, with its owner in `message.subagent_run_id()`. Use `run.thread()` to
read the registry while the run holds the mutable thread borrow.

```rust
use ag_ui::client::{Thread, Update, transport::ReplayTransport};
use ag_ui::Event;
use futures_util::StreamExt;

# #[tokio::main]
# async fn main() -> Result<(), ag_ui::client::Error> {
let transport = ReplayTransport::new([
    Event::run_started("t", "r"),
    Event::subagent_started("child", "researcher"),
    Event::text_message_chunk(Some("reply".into()), Some("Three sources.".into()))
        .with_subagent_run_id("child"),
    Event::subagent_finished_success("child"),
    Event::run_finished_success("t", "r"),
]).matching_requests();
let mut thread = Thread::new(transport, "t");
let mut run = thread.send("Research this")?;
while let Some(update) = run.next().await {
    match update {
        Update::Message(message) => {
            let name = message.message.subagent_run_id()
                .and_then(|id| run.thread().subagent(id))
                .map_or("agent", |subagent| subagent.name.as_str());
            println!("[{name}] {:?}", message.change);
        }
        Update::Subagent(change) => println!("{}: {:?}", change.subagent.name, change.change),
        _ => {}
    }
}
# Ok(())
# }
```

The registry distinguishes running, finished, suspended, failed and locally aborted
invocations. A suspended invocation can be announced again with the same ID and becomes
`Resumed`. Parent `RunError`, a disconnected stream or local abort marks remaining running
children `Aborted`; the SDK does not invent their business outcomes. Attribution without
lifecycle announcements is valid, so use a fallback label for unknown owners.

## State and errors

Use `thread.state()?` for the current typed state and `thread.raw_state()` for JSON.
Initialization and local setters validate immediately. A remote type mismatch invalidates
the typed view and emits a diagnostic; an old typed value is never returned as current.
A failed JSON Patch leaves the previous raw and typed state intact.

Custom progress and step events do not create redraw updates. Observe them with
`thread.on_event`, while continuing to use the SDK's reducer. Keep consuming after
`Update::Error`: only `Update::Done` ends the run. `collect_report()` preserves diagnostics
from previously consumed updates as well as those from the remainder.
