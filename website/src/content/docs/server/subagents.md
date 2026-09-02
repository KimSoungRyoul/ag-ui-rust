---
title: Subagents
description: Delegating part of a run to a child agent, attributing what it emits, and choosing what an older client gets to see of it.
---

Many agents delegate. A supervisor dispatches research to a child, a planner farms out
subtasks, a tool call *is* a nested agent. To a frontend all of that arrives as one event
stream, and without more information three concurrent researchers render as one wall of
text.

The protocol's answer is deliberately small. It **attributes** each event to the subagent
that produced it — an optional `subagentRunId` on 24 of the 36 event types — and it reports
when subagents start and stop, with `SUBAGENT_STARTED`, `SUBAGENT_FINISHED` and
`SUBAGENT_ERROR`. It does not orchestrate, schedule or define subagents. That stays with
you.

## An id names one invocation

`subagentRunId` is an opaque handle for **one invocation**. Run the same subagent twice
and you get two ids; the reusable half is `SubagentStartedEvent::name`, which is what a UI
displays. The symmetry with the top-level run is the way to remember it: `agentId` is to
`runId` as `name` is to `subagentRunId`.

The one exception is suspension, covered [below](#suspension-and-continuation): a subagent
that paused on an interrupt may reuse its id on the run that resumes it.

An event with no `subagentRunId` belongs to the parent agent, so a stream that never sets
the field behaves exactly as it did before subagents existed. `RUN_STARTED`, `RUN_FINISHED`
and `RUN_ERROR` cannot carry it — they describe the run as a whole — and neither can
`MESSAGES_SNAPSHOT`, whose messages carry their own. `EventType::is_attributable` answers
the question per type.

## A subagent is a scope

`ctx.subagent(name)` emits `SUBAGENT_STARTED` under a fresh id and returns a handle. Like a
[step](/ag-ui-rust/server/agent/#bracketing-a-run-with-steps), the handle dereferences to
the run context, so messages, tool calls, reasoning, steps and further subagents all open
through it — and everything they emit comes out carrying the subagent's id. When the handle
drops, `SUBAGENT_FINISHED` goes out with a success outcome, on the early return a `?`
produces as much as on the happy path.

```rust
use ag_ui::{Event, EventType, RunAgentInput, RunOutcome};
use ag_ui::server::{Agent, Result, RunContext, run};
use futures_util::StreamExt;
use serde_json::json;

struct Supervisor;

impl Agent for Supervisor {
    type State = ();

    async fn run(&self, ctx: &mut RunContext<()>) -> Result<RunOutcome> {
        let mut planner = ctx.subagent("planner")?;
        planner.say("Two tasks: scope, then risks.")?;   // attributed, through Deref
        {
            let mut estimator = planner.subagent("estimator")?;   // nested
            estimator.say("About a day each.")?;
        }                                                          // SUBAGENT_FINISHED
        planner.finish_with(json!({ "tasks": 2 }))?;

        ctx.say("Plan ready.")?;                                   // the parent's own
        Ok(RunOutcome::Success)
    }
}

#[tokio::main]
async fn main() {
    let events: Vec<Event> = run(Supervisor, RunAgentInput::new("t", "run-1"))
        .map(|event| event.expect("the stream should not break"))
        .collect()
        .await;

    let types: Vec<EventType> = events.iter().map(Event::event_type).collect();
    assert_eq!(
        types,
        [
            EventType::RunStarted,
            EventType::SubagentStarted,      // planner
            EventType::TextMessageStart,
            EventType::TextMessageContent,
            EventType::TextMessageEnd,
            EventType::SubagentStarted,      // estimator, inside planner
            EventType::TextMessageStart,
            EventType::TextMessageContent,
            EventType::TextMessageEnd,
            EventType::SubagentFinished,     // estimator
            EventType::SubagentFinished,     // planner
            EventType::TextMessageStart,
            EventType::TextMessageContent,
            EventType::TextMessageEnd,
            EventType::RunFinished,
        ]
    );

    // Ids are derived, like every other id: the run id plus a counter.
    let tag = |i: usize| events[i].subagent_run_id().map(|id| id.as_str());
    assert_eq!(tag(2), Some("run-1-sub-1"));
    assert_eq!(tag(6), Some("run-1-sub-2"));
    assert_eq!(tag(11), None);

    // The nested announcement names its parent without the agent saying so.
    let Event::SubagentStarted(estimator) = &events[5] else { unreachable!() };
    assert_eq!(estimator.parent_subagent_run_id.as_deref(), Some("run-1-sub-1"));
}
```

The handle's own methods are its endings. `finish()` and `finish_with(result)` emit
`SUBAGENT_FINISHED` with a success outcome — the second carries a completion payload, the
subagent's counterpart of `RUN_FINISHED.result`. `fail(message)` and
`fail_with_code(message, code)` emit `SUBAGENT_ERROR`. `suspend(interrupt_ids)` is the
paused case below. Every one of them names the subagent it closes and is not itself
attributed to it: the terminator belongs to the enclosing scope, which is back in force the
moment it goes out.

`Drop` cannot tell success from failure, so on the error path you care about, call `fail`
yourself. A handle that is simply dropped by a `?` unwinding through it still closes the
subagent — as a success, followed by the `RUN_ERROR` the driver emits for the run.

### Where the tag comes from

The attribution lives in the event sink, not in the handles. While a subagent scope is
open, the sink tags every attributable event that arrives untagged; a `MessageHandle`
opened inside the scope needs no idea that it was. An event the agent tagged explicitly is
left alone, and an event type that cannot carry the field stays bare:

```rust
use ag_ui::{Event, RunAgentInput};
use ag_ui::server::RunContext;
use serde_json::json;

fn main() -> ag_ui::server::Result<()> {
    let (mut ctx, mut events) = RunContext::<()>::new(RunAgentInput::new("t", "r"))?;
    {
        let mut sub = ctx.subagent("researcher")?;
        sub.emit(Event::custom("mine", json!(1)))?;
        sub.emit(Event::custom("theirs", json!(2)).with_subagent_run_id("other"))?;
        sub.emit(Event::messages_snapshot(Vec::new()))?;
    }
    let events = events.drain();
    let tag = |i: usize| events[i].subagent_run_id().map(|id| id.as_str());

    assert_eq!(tag(1), Some("r-sub-1"));   // tagged by the sink
    assert_eq!(tag(2), Some("other"));     // an explicit tag is respected
    assert_eq!(tag(3), None);              // MESSAGES_SNAPSHOT cannot carry one
    Ok(())
}
```

`ctx.subagent_run_id()` reports the scope in force, and `ctx.new_subagent_run_id()` mints
an id without opening one — for the concurrent case below.

## Agents as tools

When a tool call *is* the child agent, announce the subagent with the links a UI needs to
draw it inside the tool-call card: `parent_tool_call_id` and, when the call sits in a
message, `parent_message_id`. `ctx.subagent_with` takes the announcement you built, and
still fills an absent `parent_subagent_run_id` from the enclosing scope.

The order matters. The client sees the call close, then the subagent it spawned, then the
call's result — so end the call before opening the subagent, and emit the result after
the subagent has finished:

```rust
use ag_ui::{Event, EventType, RunAgentInput, SubagentStartedEvent};
use ag_ui::server::RunContext;

fn main() -> ag_ui::server::Result<()> {
    let (mut ctx, mut events) = RunContext::<()>::new(RunAgentInput::new("t", "r"))?;

    let mut call = ctx.tool_call("task")?;
    call.args(r#"{"brief":"find sources"}"#)?;
    let (call_id, result_id) = (call.id().clone(), call.result_message_id().clone());
    call.end()?;

    let announce = SubagentStartedEvent::new("researcher-7", "researcher")
        .with_description("Finds and ranks sources")
        .with_parent_tool_call(call_id.clone());
    let mut researcher = ctx.subagent_with(announce)?;
    researcher.say("Three sources found.")?;
    researcher.finish_with(serde_json::json!({ "sources": 3 }))?;

    ctx.emit(Event::tool_call_result(result_id, call_id, "3 sources"))?;

    let types: Vec<EventType> = events.drain().iter().map(Event::event_type).collect();
    assert_eq!(types[2], EventType::ToolCallEnd);
    assert_eq!(types[3], EventType::SubagentStarted);
    assert_eq!(types[7], EventType::SubagentFinished);
    assert_eq!(types[8], EventType::ToolCallResult);
    Ok(())
}
```

An explicit id, as here, is also how a resuming run continues a subagent — which is the
next section.

## Suspension and continuation

A subagent can be the one that needs a human. The run still ends the way every paused run
does — `RUN_FINISHED` with an interrupt outcome, connection closed, nothing held open — and
because every started subagent must close before `RUN_FINISHED`, the paused subagent closes
too. It closes with a **suspended** outcome rather than a success, naming the interrupts it
owns, so a client can show "waiting" rather than "done". Build each interrupt with
`Interrupt::with_subagent_run_id` so it renders inside the subagent's group.

On the resuming run, announce the **same id** again. A consumer treats that as a
continuation — the group moves from waiting back to running — rather than as a second
subagent:

```rust
use ag_ui::{Event, EventType, Interrupt, ResumeEntry, RunAgentInput, RunOutcome, SubagentStartedEvent};
use ag_ui::server::{Agent, Result, RunContext, run};
use futures_util::StreamExt;
use serde_json::json;

const APPROVE: &str = "approve-delete";
const DELETER: &str = "deleter-1";

struct Janitor;

impl Agent for Janitor {
    type State = ();

    async fn run(&self, ctx: &mut RunContext<()>) -> Result<RunOutcome> {
        let approved = ctx.resume_for(APPROVE).is_some();

        // The same id on both runs: a fresh one would draw a second subagent.
        let mut deleter = ctx.subagent_with(SubagentStartedEvent::new(DELETER, "deleter"))?;
        if approved {
            deleter.say("Deleted.")?;
            deleter.finish()?;
            return Ok(RunOutcome::Success);
        }

        deleter.say("This cannot be undone. May I?")?;
        let interrupt = Interrupt::new(APPROVE, "tool_approval").with_subagent_run_id(DELETER);
        deleter.suspend(vec![interrupt.id.clone()])?;
        Ok(RunOutcome::interrupt(vec![interrupt]))
    }
}

#[tokio::main]
async fn main() {
    let paused: Vec<Event> = run(Janitor, RunAgentInput::new("t", "run-1"))
        .map(|event| event.expect("the stream should not break"))
        .collect()
        .await;

    let Event::SubagentFinished(finished) = &paused[5] else { unreachable!() };
    let outcome = finished.outcome.as_ref().expect("an outcome");
    assert!(outcome.is_suspended());
    assert_eq!(outcome.interrupt_ids(), [APPROVE]);

    let Event::RunFinished(end) = &paused[6] else { unreachable!() };
    let interrupts = end.outcome.as_ref().expect("an outcome").interrupts();
    assert_eq!(interrupts[0].subagent_run_id.as_deref(), Some(DELETER));

    let mut input = RunAgentInput::new("t", "run-2");
    input.resume = Some(vec![ResumeEntry::resolved(APPROVE, json!(true))]);
    let resumed: Vec<Event> = run(Janitor, input)
        .map(|event| event.expect("the stream should not break"))
        .collect()
        .await;

    let Event::SubagentStarted(again) = &resumed[1] else { unreachable!() };
    assert_eq!(again.subagent_run_id.as_str(), DELETER);
    assert_eq!(resumed.last().map(Event::event_type), Some(EventType::RunFinished));
}
```

An ancestor of the paused subagent is suspended too, with an empty interrupt list: it owns
no interrupt itself, it is only waiting on a descendant that does. `SubagentOutcome` is the
type both cases read through, and `interrupt_ids()` is empty for the ancestor.

## Concurrency by hand

Two handles cannot be open at once — the second `subagent()` is a borrow-check error, as
everything overlapping is in this SDK. Subagents that genuinely stream concurrently are the
[parallel tool call](/ag-ui-rust/server/tools/) situation again: tag each subagent's events
yourself with `Event::with_subagent_run_id` and emit them interleaved, bracketed by the
lifecycle events.

```rust
use ag_ui::{Event, RunAgentInput, TextMessageRole};
use ag_ui::server::RunContext;

fn main() -> ag_ui::server::Result<()> {
    let (mut ctx, _events) = RunContext::<()>::new(RunAgentInput::new("t", "r"))?;
    let (a, b) = (ctx.new_subagent_run_id(), ctx.new_subagent_run_id());
    let role = TextMessageRole::Assistant;

    for event in [
        Event::subagent_started(a.clone(), "researcher"),
        Event::subagent_started(b.clone(), "researcher"),
        Event::text_message_start("m1", role).with_subagent_run_id(a.clone()),
        Event::text_message_start("m2", role).with_subagent_run_id(b.clone()),
        Event::text_message_content("m1", "GDP is ").with_subagent_run_id(a.clone()),
        Event::text_message_content("m2", "Population is ").with_subagent_run_id(b.clone()),
        Event::text_message_end("m1").with_subagent_run_id(a.clone()),
        Event::subagent_finished_success(a),
        Event::text_message_end("m2").with_subagent_run_id(b.clone()),
        Event::subagent_error(b, "rate limited"),
    ] {
        ctx.emit(event)?;   // the verifier accepts the interleaving
    }
    Ok(())
}
```

The verifier keys every entity by id and remembers who opened it, so the interleaving
passes. What it rejects is a continuation, terminator or re-open that *names* a different
subagent than the one that opened the entity — `Rule::OwnerMismatch`, the eighth rule, and
the only one subagents added on the server. A continuation that names nobody is accepted:
attribution is optional per event, and a bare continuation is what a pre-subagent producer
sends — and it does not hand the entity to the parent either; the first writer stays the
owner, as upstream records it. Steps are keyed by owner as well as name, so a subagent
cannot close the parent's step, and two agents may run a step of the same name at once.

Attribute every chunk when several subagents stream at once. A `*_CHUNK` event that names
neither a message nor a subagent continues the parent's open stream when there is one,
and otherwise the only open stream; with several subagents' streams open and the parent's
closed there is nothing to resolve it against, and the consuming side rejects it.

## What an older client sees

Attribution is additive and safe: a client that predates it sees an unknown *field* and
ignores it. The three lifecycle events are unknown *event types* to that client, and an
unknown event type fails while decoding, before any application code runs — which is
[by design](/ag-ui-rust/client/updates/#an-event-this-build-does-not-know) on the consuming
side and is nothing a producer can fix after the fact. A producer with consumers older than
`@ag-ui/client` 0.0.59 must not send them, and `SubagentVisibility` is how it does not.

| Mode | On the wire |
| --- | --- |
| `Attributed` | **The default, and no transformer at all.** The lifecycle events, and `subagentRunId` on everything a subagent produced. |
| `Inline` | The pre-subagent shape: no lifecycle events and no `subagentRunId` anywhere — not on events, not on the messages inside `MESSAGES_SNAPSHOT` or the `RUN_STARTED` input echo, not on the interrupts a paused run reports. A subagent's text arrives as the parent's work; its steps are dropped, like the lifecycle events — a step brackets its own agent's graph, and flattened it would collide with the parent's open step of the same name. |
| `Hidden` | Only the parent's own events. Everything a subagent produced is dropped, including the result of a call it requested even when the parent executed it — a result for a call the consumer never saw is a protocol error. The converse holds too: a result answering the parent's call is kept, untagged, whoever executed it. The other exception is the run's shared state: a `STATE_*` event a subagent published is kept, untagged, because a client that missed it would send a stale state back on its next request. |

Both are ordinary [transformers](/ag-ui-rust/server/axum/), so they compose with the
rest of the chain and are set per endpoint:

```rust
use ag_ui::{Event, EventType, RunAgentInput, RunOutcome};
use ag_ui::server::{Agent, Result, RunContext, Runner, SubagentVisibility};
use futures_util::StreamExt;

struct Delegating;

impl Agent for Delegating {
    type State = ();

    async fn run(&self, ctx: &mut RunContext<()>) -> Result<RunOutcome> {
        ctx.say("parent first")?;
        {
            let mut researcher = ctx.subagent("researcher")?;
            researcher.say("child")?;
        }
        ctx.say("parent last")?;
        Ok(RunOutcome::Success)
    }
}

#[tokio::main]
async fn main() {
    let inline: Vec<Event> = Runner::new(Delegating)
        .transformer(SubagentVisibility::inline())
        .run(RunAgentInput::new("t", "r"))
        .map(|event| event.expect("the stream should not break"))
        .collect()
        .await;

    // Three messages, nothing on the wire says subagent.
    assert!(inline.iter().all(|e| !matches!(
        e.event_type(),
        EventType::SubagentStarted | EventType::SubagentFinished | EventType::SubagentError
    )));
    assert!(inline.iter().all(|e| e.subagent_run_id().is_none()));
    assert_eq!(inline.iter().filter(|e| e.event_type() == EventType::TextMessageEnd).count(), 3);

    let hidden: Vec<Event> = Runner::new(Delegating)
        .transformer(SubagentVisibility::hidden())
        .run(RunAgentInput::new("t", "r"))
        .map(|event| event.expect("the stream should not break"))
        .collect()
        .await;

    // Two: the parent's own.
    assert_eq!(hidden.iter().filter(|e| e.event_type() == EventType::TextMessageEnd).count(), 2);
}
```

Over HTTP the same choice is `AgentEndpoint::transformer(|| SubagentVisibility::inline())`
— a closure, because transformers are state machines and the endpoint builds a fresh chain
per run.

:::note[Why the default is not inline]
Upstream's integrations default to the inline shape and make the full surface opt-in. This
crate defaults the other way. A transformer that rewrites the stream is opt-in here like
every other: an agent that wrote `ctx.subagent(..)` meant it, and silently flattening what
it said is the kind of surprise [the design notes](/ag-ui-rust/design/commitments/) argue
against. Flip it per endpoint when your consumers are older.
:::

## API

- [`RunContext::subagent`](/ag-ui-rust/api/ag_ui/server/struct.RunContext.html#method.subagent),
  [`subagent_with`](/ag-ui-rust/api/ag_ui/server/struct.RunContext.html#method.subagent_with)
  and [`new_subagent_run_id`](/ag-ui-rust/api/ag_ui/server/struct.RunContext.html#method.new_subagent_run_id)
- [`ag_ui::server::SubagentHandle`](/ag-ui-rust/api/ag_ui/server/struct.SubagentHandle.html)
- [`ag_ui::server::SubagentVisibility`](/ag-ui-rust/api/ag_ui/server/enum.SubagentVisibility.html)
  and [`SubagentFilter`](/ag-ui-rust/api/ag_ui/server/struct.SubagentFilter.html)
- [`ag_ui::SubagentStartedEvent`](/ag-ui-rust/api/ag_ui/struct.SubagentStartedEvent.html),
  [`SubagentFinishedEvent`](/ag-ui-rust/api/ag_ui/struct.SubagentFinishedEvent.html),
  [`SubagentErrorEvent`](/ag-ui-rust/api/ag_ui/struct.SubagentErrorEvent.html) and
  [`SubagentOutcome`](/ag-ui-rust/api/ag_ui/enum.SubagentOutcome.html)
- [`Event::subagent_run_id`](/ag-ui-rust/api/ag_ui/event/enum.Event.html#method.subagent_run_id)
  and [`EventType::is_attributable`](/ag-ui-rust/api/ag_ui/event/enum.EventType.html#method.is_attributable)
- The consuming half: [The update stream](/ag-ui-rust/client/updates/#subagents)
