---
name: ag-ui-rust-client
description: "Use when writing Rust consumers of AG-UI agents. The client is ag_ui::client in crate ag-ui with client/http features. HttpAgent creates owned Thread conversations; send/resume return Result<RunStream>, state returns Result<&S>, and RunEnd has Success, Interrupted, Failed, Aborted. Covers current typed state, observers, cancellation, reports, snapshots, safe approval resumption, custom transports and subagent rendering."
---

# Consuming an AG-UI agent from Rust

This skill targets workspace version **0.4.0**. Check the actual checkout before copying
APIs into an older released consumer. One crate, `ag-ui`, contains protocol/server/client
features; `ag-ui-client` and `ag-ui-core` are unrelated registry packages.

## Start with HttpAgent and Thread

```toml
[dependencies]
ag-ui = { version = "0.4", features = ["http"] }
futures-util = "0.3"
tokio = { version = "1", features = ["rt-multi-thread", "macros"] }
```

`http` is opt-in. `client` alone accepts custom transports and supports wasm without
Tokio or reqwest. The public relationship is endpoint → conversation → run:

```rust,no_run
use ag_ui::client::{HttpAgent, RunEnd};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let agent = HttpAgent::builder("http://localhost:3000/agent")
        .connect_timeout(std::time::Duration::from_secs(5))
        .build()?;
    let mut thread = agent.thread("thread-1");
    let another = agent.thread("thread-2");
    let report = thread.send("Check my order")?.collect_report().await;
    match report.end {
        RunEnd::Success { .. } => println!("completed"),
        RunEnd::Interrupted { .. } => println!("waiting for answers"),
        RunEnd::Failed { .. } => println!("failed"),
        RunEnd::Aborted => println!("stopped locally"),
    }
    println!("diagnostics: {:?}", report.diagnostics);
    assert!(another.messages().is_empty());
    Ok(())
}
```

Threads own a shared transport handle and independent history/state. They can outlive
the agent. Creating a thread does not query server history. Two local objects with the
same thread ID do not synchronize automatically.

`thread.send`, `send_message`, `run`, `resume`, `resume_many` and `decline` all return
`Result<RunStream>`. Preflight errors preserve conversation state. The first poll records
the new message/run and calls the transport. Dropping or aborting before first poll leaves
the thread unchanged. Default IDs use platform entropy and are checked for reuse.

## State and configuration

Use `agent.thread_with_state(id, initial_state)?` for a serializable typed state. For
history, tools or detailed configuration, use `agent.thread_builder::<State>(id)` with
`.state(json)`, `.messages(history)`, `.tools(tools)`, `.context(context)`,
`.forwarded_props(props)`, `.verify(bool)`, `.diagnostic_limit(limit)` and `.build()?`.
The generic `Thread::<T, S>::builder(transport, id)` supports custom transports.
`Thread::new(transport, id)` creates an empty JSON thread.

```rust
use ag_ui::client::{Thread, transport::ReplayTransport};
use serde::Deserialize;
use serde_json::json;

#[derive(Clone, Deserialize)]
struct Counter { count: u32 }

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut thread = Thread::<_, Counter>::builder(ReplayTransport::new([]), "t")
        .state(json!({"count": 1})).build()?;
    assert_eq!(thread.state()?.count, 1);
    thread.set_state(json!({"count": 2}))?;
    assert!(thread.set_state(json!({"count": "wrong"})).is_err());
    assert_eq!(thread.state()?.count, 2);
    Ok(())
}
```

`state()` returns `Result<&S, StateViewError>`, including for JSON threads. A remote JSON
value that does not fit `S` invalidates the previous typed view and yields `Update::Error`.
`raw_state()` stays current and the next request uses it. Later valid remote state restores
typed access. Local `set_state` validates before changing either representation; an invalid
JSON Patch changes neither. Streaming typed state needs `DeserializeOwned + Clone + Unpin`.

AG-UI has no tool discovery. Configure tools yourself through builder `.tools` or
`set_tools`. `set_context` and `set_forwarded_props` configure subsequent requests.

## Updates, reports and observers

`RunStream` borrows its thread until dropped. Inside the stream use read-only
`run.thread()`; outside, finish/drop it before reading `thread.messages()`.

- `Update::Message` carries the ID, index, change and assembled message.
- `Update::Messages` replaces the transcript view; `State` carries typed current state.
- `Reasoning` is separate from the transcript; `Subagent` carries lifecycle only.
- `Interrupt` announces a pending question.
- `Error` is a diagnostic; keep consuming until `Done`.
- `Done` contains `RunEnd::{Success, Interrupted, Failed, Aborted}`. Match all four.

`Update` is non-exhaustive. `RunEnd` is exhaustive. A successful terminal can coexist with
local patch/validation diagnostics. `collect_report()` returns `end`, `new_messages`,
`diagnostics` and `diagnostics_omitted` for the whole run even after partial consumption.
It retains 100 diagnostics by default, without suppressing individual Error updates.
`new_messages` selects IDs absent before this run; edits to old IDs are in the thread.

`thread.on_event` replaces a synchronous read-only callback. It receives every decoded
raw event once before normalize/verify/apply, including custom/raw/step events. It does
not observe synthetic normalized events. Use application queues for slow I/O and
`clear_event_observer` to remove it. Callbacks are `'static` and `Send` on native; wasm
accepts local callbacks. Decode failures are diagnostics and terminate the high-level run.

## Approvals and uncertainty

Use `thread.resume_many(entries)?` to answer **all** pending interrupts exactly once.
`resume(&interrupt, payload)` and `decline(&interrupt)` are single-interrupt conveniences.
Unknown/duplicate/missing IDs and expired stored interrupts fail before dispatch. The
caller-supplied Interrupt object cannot replace the stored expiry. `response_schema` is
preserved for form rendering; arbitrary JSON Schema validation and execution authorization
remain application/server responsibilities.

`ResumeBuilder` and `InterruptExt` construct responses:

```rust
use ag_ui::client::interrupts::ResumeBuilder;
use ag_ui::{Interrupt, ResumeStatus};
let first = Interrupt::new("first", "approval");
let second = Interrupt::new("second", "approval");
let entries = ResumeBuilder::new().resolve(&first, true).cancel(&second).build();
assert_eq!(entries[1].status, ResumeStatus::Cancelled);
```

Dispatch keeps pending questions and records `ResumeSubmission { run_id, entries,
status: InFlight }`. Only a validated matching `RunFinished` clears the submission and
either clears or replaces pending questions. Transport failure, abort, `RunError` or invalid
terminal retains it as `Unconfirmed`. Further ordinary send/resume calls are blocked.
The application must query its server's actual state and reconstruct a reconciled snapshot.
There is no generic AG-UI reconciliation endpoint or automatic decision retry.

## Snapshots and cancellation

`thread.snapshot()` is serde-compatible and versioned. `agent.restore_thread(snapshot)?`
restores JSON; `restore_thread_with_state::<S>` validates a typed view. Custom transports
use `Thread::restore`. Restoration checks duplicate IDs, snapshot version and stored
references, and does not fetch server state. Snapshots retain history/raw state/reasoning,
subagents, pending questions, submission attempts, used run IDs and observed local status.
An in-flight snapshot becomes locally aborted and its submission becomes Unconfirmed.
Transport credentials, observers, futures, tools/context/forwarded properties are excluded;
reconfigure request settings explicitly after restoring.

`run.abort_handle()` returns a cloneable per-run handle that wakes pending polls and drops
connection/response futures. If a terminal was already applied, later abort keeps it.
Otherwise consuming the run yields `Aborted` once. Dropping a dispatched run records
local abortion but cannot deliver Done to a departed consumer. This does not prove remote
cancellation. `decline` answers a question; it is distinct from aborting a run.

## Rendering subagents and incomplete messages

Group ordinary messages by `Message::subagent_run_id()`. `Update::Subagent` gives the
name, parent references and status, not the child's text. Suspended invocations can resume
under the same ID. Attribution without lifecycle events is valid; provide a fallback label.
A parent error or interrupted connection marks open children `SubagentStatus::Aborted`
without inventing a business failure or success. Open text/reasoning uses local `Aborted`
changes so a renderer can stop indicators without claiming successful `Ended`.

State from every source is shared run state. Key interleaved tool fragments by tool call
ID. Drawing in arrival order preserves sequence; buffering until each call ends moves its
line after events received during that call. [Rendering details](references/rendering.md).

## Testing and raw transports

```rust
use ag_ui::client::{Thread, RunEnd, transport::ReplayTransport};
use ag_ui::Event;

#[tokio::main]
async fn main() -> Result<(), ag_ui::client::Error> {
    let replay = ReplayTransport::new([
        Event::run_started("t", "fixture-run"),
        Event::run_finished_success("t", "fixture-run"),
    ]).matching_requests();
    let mut thread = Thread::new(replay.clone(), "t");
    let report = thread.send("Hello")?.collect_report().await;
    assert!(matches!(report.end, RunEnd::Success { .. }));
    assert_eq!(replay.requests().len(), 1);
    Ok(())
}
```

Replay is literal by default. `.matching_requests()` rewrites only a matching scripted
lifecycle pair to the actual request IDs. Use literal replay to test mismatches, or
`set_next_run_id` to choose a known request ID. `with_runs` supplies multiple fixtures;
clones share fixtures and recorded requests.

`Transport::run(&self, RunAgentInput) -> TransportFuture` returns a `'static` event future;
clone what it needs instead of borrowing self. `&T`, `Box<T>` and `Arc<T>` implement Transport
when `T` does. Native event futures/streams are Send, wasm aliases are local.
`RemoteAgent::run_events(params)` and `HttpAgent::run_events(params)` expose raw events
without state handling. `decode_events` and `boxed_stream` adapt byte streams. Use
`Error::transport(error)` for custom transport errors.

HTTP `.connect_timeout` bounds setup; `.timeout` bounds the entire streamed run.
`HttpAgent::builder` exposes both alongside headers and a configured reqwest Client.
Neither the client nor the transport retries submitted decisions automatically.

Activity messages remain in the local thread and its snapshot. High-level Thread requests
omit them from the conversation sent to the server, matching the reference client.
Raw `RemoteAgent::run_events` requests are transmitted as supplied.
