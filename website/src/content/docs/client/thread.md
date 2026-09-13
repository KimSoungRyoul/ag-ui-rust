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

Continue with the [component guide](/ag-ui-rust/client/state/).

## Approvals and declines

Continue with the [component guide](/ag-ui-rust/client/interrupts/).

## Observe, stop and restore

Continue with the [component guide](/ag-ui-rust/client/interrupts/).
