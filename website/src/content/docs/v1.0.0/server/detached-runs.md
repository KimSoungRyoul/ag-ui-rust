---
title: Runs that outlive a response
description: Keep a driver running across disconnects, catch up a new subscriber, or continue from stored events.
---

With `Runner::run`, polling the response stream drives the agent. Dropping
that stream drops the driver's future. Use `Runner::into_parts` when the
application needs to run independently of any one response.

## Drive and store a run

1. Schedule the driver future on your application's executor.
2. Keep one event receiver for the run. Drain it promptly, store the events
   in order, and distribute them to subscribers.
3. To stop the run deliberately, cancel via the runner's cancellation token
   and keep draining for its terminal event. If the receiver closes while
   the driver is still pending, stop the driver. A bounded queue overflow
   can close it after a `RUN_ERROR`.

```rust
use ag_ui::{Event, EventType, RunAgentInput, RunOutcome};
use ag_ui::server::{Agent, Result, RunContext, Runner};
use futures_util::StreamExt;

# struct Greeter;
# impl Agent for Greeter {
#     type State = ();
#     async fn run(&self, ctx: &mut RunContext<()>) -> Result<RunOutcome> {
#         ctx.say("Hello")?;
#         Ok(RunOutcome::Success)
#     }
# }
# #[tokio::main]
# async fn main() {
let (driver, mut events) =
    Runner::new(Greeter).into_parts(RunAgentInput::new("thread-1", "run-1"));
let task = tokio::spawn(driver);
let mut stored = Vec::new();
while let Some(event) = events.next().await {
    stored.push(event);
}
task.abort();

assert_eq!(stored.last().map(Event::event_type), Some(EventType::RunFinished));
# }
```

The vector stands in for your event store. Admission, checkpoints, storage and
subscriber delivery belong to your host. The standard Axum endpoint ties the
driver to the response body. Serve detached runs from an application endpoint
that subscribes to the event store.

## Attach another client

Feed the stored prefix to `RunProgress::replay`, or update progress with
`observe` as events are sent. Use `prelude()` to produce a `RUN_STARTED`
followed by the opened entities and their accumulated content. Supply the
application's conversation and state with `Prelude::messages` and
`Prelude::state` when the subscriber needs a baseline. `RunProgress` tracks
open entities and can describe them as `OpenEntity` values.

```rust
use ag_ui::{Event, EventType, TextMessageRole};
use ag_ui::server::RunProgress;

let sent = [
    Event::run_started("thread-1", "run-1"),
    Event::text_message_start("msg-1", TextMessageRole::Assistant),
    Event::text_message_content("msg-1", "Hello"),
];
let mut progress = RunProgress::new();
progress.replay(&sent)?;
let prelude = progress.prelude().into_events();
assert_eq!(prelude[0].event_type(), EventType::RunStarted);
# Ok::<(), ag_ui::server::VerificationError>(())
```

Choose one stream prefix for that subscriber. Replay the whole stored stream,
or send the prelude and then only the events after its cut in the log. Applying
both prefixes can append the same content twice. The host coordinates the cut
and live subscription so no event is lost between them.

## Continue the same run from another driver

Use `Runner::continue_run(input, &sent)` with the same thread ID and run ID,
the messages the run started with, the current state, and every event already
sent in order. Drive the returned future and event receiver as above. The
continuation emits only the suffix and reuses the history for ordering,
state baselines and ID allocation.

There are no typed handles for entities opened by the earlier driver. Use
`RunContext::emit` with their existing IDs to continue and close them
before finishing. On cancellation, the driver closes what it inherited
before sending the cancelled outcome. When building events yourself,
initialize `EventVerifier` with `seed` for request history and
`replay` for events already sent, then check the suffix with that verifier.

The host supplies the continuation's application state and execution logic.
An approval response is a separate [interrupt and resume flow](/ag-ui-rust/v1.0.0/server/interrupts/)
that opens another run.

## Transform events across a continuation

A fresh transformer gets the request's messages via `StreamTransformer::seed`,
then the events that reached subscribers via `StreamTransformer::replay`, then
new events. Store the published events after filtering so a new driver can
reconstruct what subscribers saw.

`FilterToolCalls` applies its allowlist or denylist to calls and their results,
including calls in a `MESSAGES_SNAPSHOT` or `RUN_STARTED` input echo.
An event for a call whose tool name the filter never learned is dropped.
If the new driver continues a tool call with chunks, put its ID on the first
chunk. Later chunks can use that stream's remembered ID.
The filter only covers recognized tool call fields. The host defines what
may appear in raw and custom events, `rawEvent`, and interrupts.
