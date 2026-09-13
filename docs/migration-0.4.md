# Migrating to 0.4

Version 0.4 changes public conversation APIs and fixes A2UI v0.9-family semantics.
Use the repository dependency; no crates.io publication is implied by this version.

## Conversations

Version 0.4.1 uses only `Thread` and `ThreadBuilder` for conversations. The old
conversation type aliases and module are removed; update imports to `ag_ui::client`
or `ag_ui::client::thread`.

| Task | Supported API |
|---|---|
| Connect to an HTTP agent | `HttpAgent::new(url)?` |
| Use a configured HTTP transport | `HttpAgent::from_transport(transport)` |
| Consume literal wire events | `agent.run_events(input)` |
| Create a typed conversation | `agent.thread_with_state(id, initial_state)?` |
| Send a user turn | `thread.send(text)?` |
| Decline a pending decision | `thread.decline(interrupt)?` |
| Inspect a running conversation | `run.thread()` |
| Read typed state | `state() -> Result<&S, StateViewError>` |
| Finish a thread builder | `.build()?` |

`HttpAgent` is a concrete struct. `RemoteAgent::new(transport)` remains the low-level
custom-transport entry point. No compatibility aliases are provided for conversation names.

Thread keeps Activity messages locally but omits them from outgoing run input, as the
official client does. Raw `run_events()` preserves caller-supplied messages verbatim.

```rust,no_run
use ag_ui::client::HttpAgent;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let agent = HttpAgent::new("http://localhost:3000/agent")?;
    let mut thread = agent.thread("conversation-1");
    let report = thread.send("Hello")?.collect_report().await;
    println!("{:?}: {:?}", report.end, report.diagnostics);
    Ok(())
}
```

An agent creates independent local threads and shares its connection settings.
Creating a thread does not load remote history. Requests dispatch on the first poll;
dropping an unpolled run does not add a message or make a request.

`collect_report()` describes the whole run even after partial stream consumption.
Diagnostics remain separate from a server success and carry an omission count if
their configured retention limit was reached. Raw events can be observed through
`thread.on_event(...)` without bypassing the state applier.

## State and restoration

Setters validate before changing state. A valid local `set_state()` updates both
raw and typed views; an invalid one changes neither. A valid remote JSON value
that does not fit `S` advances raw state but clears the typed cache. `state()` then
returns an error until a valid typed update arrives.

Snapshots retain local conversation data, pending decisions, uncertain submissions
and invocation information. Restore with `agent.restore_thread(snapshot)?` or
`restore_thread_with_state::<S>(snapshot)?`. Reconfigure tools, context and forwarded
properties after restoring. Credentials, transports and observers are not saved.
Snapshot restoration does not resume a remote task by itself.

Generated IDs are random strings and are checked for collisions. Tests that depend
on an exact run ID should call `set_next_run_id()` or inject a deterministic generator.
`ReplayTransport` stays literal by default. Add `.matching_requests()` when a scripted
happy path should echo the actual request's lifecycle IDs; leave it off to test bad IDs.

## Approvals and stopping

Resume responses must cover the current pending IDs exactly once. Unknown, duplicate,
expired and partial responses fail before dispatch and keep the existing pending list.
`decline()` answers one pending decision; it does not cancel execution. Response schema
interpretation and execution authorization remain server/application responsibilities.

A response lost after dispatch leaves an `Unconfirmed` submission with the exact answers.
It is not retried automatically. The application must check authoritative server state
before restoring a reconciled snapshot and continuing.

Use the run's `abort_handle()` for a stop button. Local consumption ends as
`RunEnd::Aborted`, distinct from server success or failure. A stale handle cannot stop
a later run. Interrupted message/reasoning/subagent displays have an Aborted status;
they must not be displayed as successful completion. Dropping a run releases its I/O
but cannot deliver a final update to a consumer that no longer exists.

## Subagent events

`ctx.subagent_events(name)` describes application-owned work. It creates no model,
agent executor, scheduler or durable task. `ctx.subagent(name)` remains an equivalent
spelling for migration.

Explicitly call `finish()`, `finish_with(result)`, `fail(message)` or
`suspend(interrupt_ids)`. Drop restores the parent's attribution and emits no invented
outcome. A parent error may end with a child open. A successful/interrupted parent with
an unclosed child becomes a protocol error even when optional verification is off or
the child events are hidden by a visibility transformer.

## A2UI

- v0.9 and v0.9.1 are accepted. Low-level builders retain v0.9 by default; authors
  select their version explicitly. v1.0 RPC is rejected in v0.9-family messages.
- `UpdateDataModel.value` is `DataModelUpdate::Set(Value)` or `Remove`. **Explicit
  null stores null. Omitting value removes the target.** Use
  `AgentMessage::remove_data_model_value(surface_id, path)` for removal.
- `DataModel` retains undefined array slots and an absent root. Exporting such a
  model as ordinary JSON returns an error instead of replacing holes with null.
  `apply_model()` works with the lossless model; `apply()` supports ordinary JSON
  when the result remains representable.
- History and generated surfaces now expose a lossless model. Use `to_json()?` when
  an application needs ordinary JSON. The `try_find_prior_surface` helpers report
  malformed recognized operations rather than quietly returning a partial surface.
- Component updates are applied per surface and replace prior definitions. Invalid
  pointers fail atomically. Duplicate active creates are rejected; delete then
  recreate is permitted.
- Capabilities use their versioned transport representation. Catalog negotiation
  does not merge an unrelated inline catalog under a different catalog's ID or
  silently choose the server default when there is no match.
- `BASIC_CATALOG_ID` retains the toolkit compatibility identifier;
  `OFFICIAL_BASIC_CATALOG_ID` identifies the bundled official catalog used by the
  author. Aliases must be configured explicitly.

New optional features:

| Feature | Provides |
|---|---|
| `schema-validation` | Full Draft 2020-12 validation with a local resource registry |
| `author` | Async `A2uiAuthor` and immutable validated output |
| `ag-ui-server` | Author plus `ctx.send_a2ui(...)`, without HTTP or axum dependencies |

`A2uiAuthor` retries malformed/invalid generated documents. It does not automatically
retry provider transport errors. Create/edit requests enforce their target surface,
catalog and version; stored output must be revalidated. A successful send only means
the event sink accepted the payload, not that a renderer acknowledged it.

## Verification

The [examples catalog](../examples/README.md) lists the three runnable consumers and
their HTTP tests. Review Desk exercises two conversations, restoration, pending
decisions, abort, subagent attribution and async A2UI generation through public APIs.

Run both commands; nextest does not execute documentation examples:

```sh
cargo nextest run --workspace --all-features
cargo test --doc --workspace --all-features
```
