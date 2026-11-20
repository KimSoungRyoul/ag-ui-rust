---
title: Connect and manage threads
description: Connect to an agent, stream a conversation, retain state, and answer interrupts.
---

An `HttpAgent` holds connection settings. A `Thread` owns one conversation's messages,
state and pending decisions. A run is one request and its update stream. Creating a
thread is local; it does not retrieve server history. Two threads created with the same
ID are independent local views, not automatically synchronized objects.

## Connect and stream

```rust,no_run
use ag_ui::client::{HttpAgent, Update};
use futures_util::StreamExt;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let agent = HttpAgent::new("http://localhost:3000/agent")?;
    let mut thread = agent.thread("conversation-1");
    {
        let mut run = thread.send("Hello")?;
        while let Some(update) = run.next().await {
            match update {
                Update::Message(message) => println!("{:?}", message.change),
                Update::Error(error) => eprintln!("{error}"),
                Update::Done(end) => println!("{end:?}"),
                _ => {}
            }
        }
    }
    let report = thread.send("Continue")?.collect_report().await;
    println!("{:?}, diagnostics: {:?}", report.end, report.diagnostics);
    Ok(())
}
```

The `?` checks pending decisions and request configuration before mutation. HTTP starts
on the first poll. The run borrows its thread; read additional state through `run.thread()`.
`collect_report()` retains diagnostics even when the server reports success.

## Typed state

Continue with the [component guide](/ag-ui-rust/v1.0.0/client/state/).

## Approvals and declines

Continue with the [component guide](/ag-ui-rust/v1.0.0/client/interrupts/).

## Observe, stop and restore

Continue with the [component guide](/ag-ui-rust/v1.0.0/client/interrupts/).

## Run two requests on one conversation

A request borrows its `Thread` until the stream ends. For concurrent work, make
a second `Thread` with the same conversation ID and a shared starting snapshot.
Reapply tools, context, and forwarded properties after restoring. Use
`merge_messages` to draw a single list while both work, then
`Thread::merge_messages_from` to retain new messages after both streams end.

```rust
use ag_ui::client::{RunEnd, Thread, merge_messages, transport::ReplayTransport};
# use ag_ui::{Event, TextMessageRole};
use futures_util::StreamExt;
# fn reply(id: &str, text: &str) -> Vec<Event> {
#     vec![
#         Event::run_started("thread-1", "run"),
#         Event::text_message_start(id, TextMessageRole::Assistant),
#         Event::text_message_content(id, text),
#         Event::text_message_end(id),
#         Event::run_finished_success("thread-1", "run"),
#     ]
# }
# let transport = ReplayTransport::with_runs([
#     reply("report", "Here is the report."),
#     reply("time", "It is noon."),
#     reply("welcome", "You are welcome."),
# ])
# .matching_requests();
# let rt = tokio::runtime::Builder::new_current_thread().build().unwrap();
# rt.block_on(async {
let mut main = Thread::new(transport.clone(), "thread-1");
// A second thread on the same conversation, holding the same messages.
let mut side = Thread::<_>::restore(transport.clone(), main.snapshot())?;

// The long run starts, and borrows `main` until its stream ends ...
let mut long = main.send("Write the report")?;
long.next().await;
// ... so the quick question goes to `side`, and is answered meanwhile.
let quick = side.send("What time is it?")?.collect_report().await;
assert_eq!(quick.end, RunEnd::Success { result: None });
// Both can be drawn as one list at any moment.
let on_screen = merge_messages(long.thread().messages(), side.messages());
assert_eq!(on_screen.len(), 4);
long.collect_report().await;

// Both streams have ended: `main` takes the messages it lacks ...
let merged = main.merge_messages_from(&side)?;
assert_eq!(merged.added.len(), 2);
assert!(merged.differing.is_empty());
// ... and its next request carries both runs.
main.send("Thanks")?.collect_report().await;
assert_eq!(transport.last_request().unwrap().messages.len(), 5);
# Ok::<(), ag_ui::client::Error>(())
# })
# .unwrap();
```

The merge keys on message ID and reports differing content in
`MergeReport::differing`. The app reconciles shared state separately. Settle
unanswered calls, pending interrupts and unconfirmed resume submissions on each
thread first. A copy made mid-run can use `abandon_tool_call` for an outstanding
call. When the original has received the real result, merging the copy into the
original leaves out the stand-in and lists it in `MergeReport::superseded`.
