---
title: Approvals and recovery
description: Implement approval responses, cancellation and snapshot recovery.
---

When the server returns an interrupt, collect a decision and respond through the same `Thread`.
Render approval waits separately from execution failures.

## Approvals and declines

Answer every pending decision together. Partial responses, duplicate IDs and unknown IDs
are rejected before sending. `decline()` answers a single approval; it does not stop a run.

```rust
use ag_ui::client::{Thread, InterruptExt, RunEnd, transport::ReplayTransport};
use ag_ui::{Event, Interrupt};
use serde_json::json;

#[tokio::main]
async fn main() {
    let transport = ReplayTransport::with_runs([
        vec![Event::run_started("t", "r1"),
             Event::run_finished_interrupt("t", "r1", vec![
                 Interrupt::new("budget", "approval"),
                 Interrupt::new("date", "approval"),
             ])],
        vec![Event::run_started("t", "r2"), Event::run_finished_success("t", "r2")],
    ]).matching_requests();
    let mut thread = Thread::new(transport, "t");
    thread.send("Plan the trip").unwrap().collect_report().await;
    let pending = thread.interrupts().to_vec();
    assert!(thread.resume(&pending[0], json!(true)).is_err());
    assert_eq!(thread.interrupts().len(), 2);
    let report = thread.resume_many([
        pending[0].resolve(json!(true)), pending[1].cancel(),
    ]).unwrap().collect_report().await;
    assert!(matches!(report.end, RunEnd::Success { .. }));
}
```

A lost response after submitting approvals leaves an `Unconfirmed` submission. The SDK does
not retry it automatically. The application checks server state and restores an authoritative
snapshot. Response-schema validation and execution authorization belong to the server/application.

## Observe, stop and restore

`on_event` installs a synchronous read-only observer before normalization. Calling it again
replaces it; `clear_event_observer()` removes it. An abort handle stops only its own run and
does not confirm remote business cancellation. Snapshots retain conversation and pending state,
not transport, credentials or observers. Reconfigure tools/context after restoration.

```rust,no_run
use ag_ui::{Event, Tool};
use ag_ui::client::HttpAgent;
use serde_json::json;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let agent = HttpAgent::builder("http://localhost:3000/agent")
        .header("authorization", "Bearer token").build()?;
    let mut thread = agent.thread("t");
    thread.set_tools(vec![Tool::new("search", "Search", json!({"type":"object"}))]);
    thread.on_event(|event| {
        if let Event::Custom(progress) = event { println!("{}", progress.value); }
    });
    let run = thread.send("Search")?;
    let abort = run.abort_handle();
    // An application's stop handler can call abort.abort().
    let report = run.collect_report().await;
    println!("{:?}", report.end);
    drop(abort);
    let saved = serde_json::to_vec(&thread.snapshot())?;
    let snapshot = serde_json::from_slice(&saved)?;
    let restored = agent.restore_thread(snapshot)?;
    assert_eq!(restored.thread_id(), thread.thread_id());
    Ok(())
}
```

See [updates](/ag-ui-rust/client/updates/) and
[transports](/ag-ui-rust/client/transports/) for the lower-level interfaces.
