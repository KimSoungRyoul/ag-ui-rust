---
title: The update stream
description: Stream updates, retain diagnostics, and distinguish remote completion from local cancellation.
---

`Thread::send`, `run` and `resume_many` return `Result<RunStream>`. The result checks
local request conditions. The stream then normalizes chunks, validates protocol ordering
and applies messages and state. Each `Update` describes one change; it is not a complete
message or an independent run.

| Update | What a view receives |
| --- | --- |
| `Message` | Message ID, index, change and current assembled message |
| `Messages` | The conversation after a messages snapshot |
| `State(S)` | Current state after a snapshot or patch |
| `Reasoning` | Reasoning content, separate from the transcript |
| `Subagent` | Invocation name, parent links and lifecycle status |
| `Interrupt` | A pending question from the terminal interrupt outcome |
| `Error` | A diagnostic; continue consuming until `Done` |
| `Done` | The run's terminal outcome |

`Update` is non-exhaustive. `RunEnd` has four variants:

```rust
use ag_ui::client::RunEnd;

fn label(end: &RunEnd) -> &'static str {
    match end {
        RunEnd::Success { .. } => "completed",
        RunEnd::Interrupted { .. } => "waiting for input",
        RunEnd::Failed { .. } => "failed",
        RunEnd::Aborted => "stopped locally",
    }
}

assert_eq!(label(&RunEnd::Aborted), "stopped locally");
```

A successful server result can coexist with local diagnostics such as a rejected state
patch. `collect_report()` retains the outcome and diagnostics from the **whole run**,
even if some updates were already consumed. `new_messages` contains messages whose IDs
were absent when the run was prepared; modifications to existing IDs are in
`thread.messages()`. Reports retain 100 diagnostics by default; `diagnostic_limit` on the
builder or `set_diagnostic_limit` changes the limit. `diagnostics_omitted` counts the rest.
Every diagnostic is still delivered as an `Update::Error`.

```rust
use ag_ui::client::{Thread, RunEnd, transport::ReplayTransport};
use ag_ui::Event;

# #[tokio::main]
# async fn main() -> Result<(), ag_ui::client::Error> {
let transport = ReplayTransport::new([
    Event::run_started("t", "r"),
    Event::custom("progress", serde_json::json!({"percent": 50})),
    Event::run_finished_success("t", "r"),
]).matching_requests();
let mut thread = Thread::new(transport, "t");
thread.on_event(|event| {
    if let Event::Custom(progress) = event {
        println!("{}: {}", progress.name, progress.value);
    }
});
let report = thread.send("Check the order")?.collect_report().await;
assert!(matches!(report.end, RunEnd::Success { .. }));
assert!(report.diagnostics.is_empty());
# Ok(())
# }
```

The observer receives each decoded wire event exactly once, before normalization,
validation and state application. It sees custom, raw and step events too. It is a
synchronous read-only callback; enqueue slow work in the application. Setting another
observer replaces the old one; `clear_event_observer()` removes it. Native callbacks must
be `Send + 'static`; wasm allows local callbacks. Decode failures are diagnostics, not events.

Use `run.abort_handle()` to stop a pending connection or response stream. An already
applied terminal result wins over a later abort. Otherwise local cancellation yields
`RunEnd::Aborted` exactly once. Dropping an unpolled stream sends nothing and appends no
message. Dropping a dispatched stream records local abortion but has no consumer to
receive `Done`. Neither action proves that remote work stopped.

A remote state that cannot deserialize into `S` remains available through `raw_state()`;
`state()` returns an error until a later valid update restores the typed view. Local
`set_state()` validates before changing either representation. See
[Threads](/ag-ui-rust/client/thread/) for initialization, snapshots and approval recovery.

Activity messages remain in the local thread and its snapshot. High-level Thread requests
omit them from the conversation sent to the server, matching the reference client.
Raw `RemoteAgent::run_events` requests are transmitted as supplied.
