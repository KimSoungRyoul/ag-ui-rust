---
title: Human in the loop
description: Ending a run to wait for a person, and picking the work back up on the request that follows.
---

Some runs cannot finish on their own. A deployment needs approval, a destructive command
needs confirming, a form needs filling in. AG-UI models this by letting a run *pause*: the
agent returns `RunOutcome::Interrupt` instead of `Success`, the client collects the answers,
and a second request carries them back.

The important part is what a pause is on the wire. It is a **finished run** — a normal
`RUN_FINISHED` event whose `outcome` says `interrupt` and lists what is pending. The
connection closes, nothing is held open, and no server-side session survives the pause. The
next request is an ordinary request in the same thread that happens to carry answers.

## The round trip

```rust
// src/agent.rs
use ag_ui::{Event, Interrupt, ResumeEntry, ResumeStatus, RunAgentInput, RunOutcome};
use ag_ui::server::{Agent, Error, Result, RunContext, run};
use futures_util::StreamExt;
use serde_json::{Value, json};

const APPROVE_DEPLOY: &str = "approve-deploy";

struct Deployer;

impl Agent for Deployer {
    type State = ();

    async fn run(&self, ctx: &mut RunContext<()>) -> Result<RunOutcome> {
        // No answer yet: say why, and pause.
        let Some(answer) = ctx.resume_for(APPROVE_DEPLOY) else {
            ctx.say("Deploying to production needs a human.")?;
            return Ok(RunOutcome::interrupt(vec![request()]));
        };

        match answer.status {
            ResumeStatus::Resolved => {
                // Reading the payload is what proves it survived the trip. An
                // answer that arrives empty fails the run rather than quietly
                // succeeding.
                let build = answer
                    .payload
                    .as_ref()
                    .and_then(|payload| payload.get("build"))
                    .and_then(Value::as_u64)
                    .ok_or_else(|| Error::agent("the approval carried no build number"))?;
                ctx.say(format!("Deployed build {build}."))?;
            }
            ResumeStatus::Cancelled => {
                ctx.say("Left production alone.")?;
            }
        }

        Ok(RunOutcome::Success)
    }
}

/// What the client is asked, and the shape its answer must take.
fn request() -> Interrupt {
    Interrupt {
        id: APPROVE_DEPLOY.to_owned(),
        reason: "tool_approval".to_owned(),
        message: Some("Deploy build 42 to production?".to_owned()),
        ..Default::default()
    }
}

#[tokio::main]
async fn main() {
    // Turn one: the agent pauses.
    let first: Vec<Event> = run(Deployer, RunAgentInput::new("deploy", "run-1"))
        .map(|event| event.expect("the stream should not break"))
        .collect()
        .await;

    let Some(Event::RunFinished(finished)) = first.last() else {
        panic!("a paused run still finishes: {first:?}");
    };
    assert_eq!(
        finished.outcome.as_ref().map(RunOutcome::interrupts),
        Some(&[request()][..])
    );

    // Turn two: same thread, a new run id, carrying the answer.
    let mut resumed = RunAgentInput::new("deploy", "run-2");
    resumed.resume = Some(vec![ResumeEntry::resolved(
        APPROVE_DEPLOY,
        json!({"build": 42}),
    )]);

    let second: Vec<Event> = run(Deployer, resumed)
        .map(|event| event.expect("the stream should not break"))
        .collect()
        .await;

    assert!(second.iter().any(|event| matches!(
        event,
        Event::TextMessageContent(content) if content.delta == "Deployed build 42."
    )));
}
```

## What to put in an interrupt

`Interrupt::new(id, reason)` sets the two required fields; the rest are optional and exist so
a client can render the question without knowing anything about your agent.

| Field | What it is for |
| --- | --- |
| `id` | correlation. Echoed back as `ResumeEntry::interrupt_id` |
| `reason` | machine-readable, for example `"tool_approval"` |
| `message` | the prompt to show the user |
| `tool_call_id` | the call awaiting approval, when the interrupt is about one |
| `response_schema` | JSON Schema the answer must satisfy — lets a client render a form |
| `expires_at` | when the question stops being answerable, ISO-8601 |
| `metadata` | integration-specific extras |

A `response_schema` is worth setting whenever the answer is more than yes or no:

```rust
use ag_ui::{Interrupt, JsonObject};
use serde_json::json;

fn confirm_clear(count: usize) -> Interrupt {
    let mut schema = JsonObject::new();
    schema.insert("type".to_owned(), json!("object"));
    schema.insert(
        "properties".to_owned(),
        json!({"confirm": {"type": "boolean"}}),
    );
    schema.insert("required".to_owned(), json!(["confirm"]));

    Interrupt {
        id: "confirm-clear".to_owned(),
        reason: "tool_approval".to_owned(),
        message: Some(format!("Clear the board? {count} task(s) will be removed.")),
        response_schema: Some(schema),
        ..Default::default()
    }
}

fn main() {
    let interrupt = confirm_clear(3);
    assert_eq!(interrupt.id, "confirm-clear");
    assert!(interrupt.response_schema.is_some());
}
```

## Reading the answers

Everything about a resumed request is on the context:

| Method | Answers |
| --- | --- |
| `is_resume()` | is this request resuming a paused run at all |
| `resume()` | every `ResumeEntry` the request carried |
| `resume_for(id)` | the answer to one interrupt, or `None` |

A `ResumeEntry` has a `status` — `Resolved` or `Cancelled` — and an optional `payload`. The
two statuses are the *user's* two answers, not success and failure: a cancelled interrupt is
a person declining, and the run that reads it should carry on down the other branch and
finish successfully. A run that fails because the answer was malformed is an
[error](/ag-ui-rust/server/errors/), and reaches the client as `RUN_ERROR`.

```rust
use ag_ui::{ResumeEntry, ResumeStatus, RunAgentInput};
use ag_ui::server::RunContext;
use serde_json::json;

fn main() -> ag_ui::server::Result<()> {
    let mut input = RunAgentInput::new("t", "r");
    input.resume = Some(vec![
        ResumeEntry::resolved("approve-deploy", json!({"build": 42})),
        ResumeEntry::cancelled("confirm-clear"),
    ]);
    let (ctx, _events) = RunContext::<()>::new(input)?;

    assert!(ctx.is_resume());
    assert_eq!(ctx.resume().len(), 2);
    assert_eq!(
        ctx.resume_for("approve-deploy").map(|entry| entry.status),
        Some(ResumeStatus::Resolved)
    );
    assert_eq!(
        ctx.resume_for("confirm-clear").map(|entry| entry.status),
        Some(ResumeStatus::Cancelled)
    );
    assert!(ctx.resume_for("something-else").is_none());
    Ok(())
}
```

## The agent remembers nothing

This is the part that catches people out. The paused run is gone — its future was dropped
when the stream ended — so the resumed run starts from scratch and rebuilds its position
from `messages`, `state` and `resume`. Only the answers *this request* carries exist.

The consequence shows up as soon as a run pauses on more than one decision. If the client
answers one interrupt per request, every request tells the agent about exactly one answer,
and the agent pauses again on whatever that request did not mention. It never terminates.
`e2e/tests/human_in_the_loop.rs` pins this: answering the budget, then the date, leaves the
budget unanswered again, and only sending both in one request finishes the run.

So an agent that pauses on several things at once should report the ones still outstanding,
and a client should answer all of them together:

```rust
use ag_ui::{Interrupt, RunOutcome};
use ag_ui::server::{Agent, Result, RunContext};

const BUDGET: &str = "approve-budget";
const DATE: &str = "confirm-date";

struct Planner;

impl Agent for Planner {
    type State = ();

    async fn run(&self, ctx: &mut RunContext<()>) -> Result<RunOutcome> {
        let pending: Vec<Interrupt> = [BUDGET, DATE]
            .into_iter()
            .filter(|id| ctx.resume_for(id).is_none())
            .map(|id| Interrupt::new(id, "tool_approval"))
            .collect();

        if pending.is_empty() {
            ctx.say("Booked.")?;
            return Ok(RunOutcome::Success);
        }

        Ok(RunOutcome::interrupt(pending))
    }
}
```

Note the `filter` runs before anything is emitted. `resume_for` borrows the context
immutably and the emitters want it mutably, so reading the pending decisions first is not
just tidier — it is what makes the borrows line up.

## One rule the type system does not catch

`RunOutcome::Interrupt` can be constructed with an empty list, and the protocol forbids it.
The driver validates the outcome before emitting, so an empty interrupt list becomes a
`RUN_ERROR` with the `PROTOCOL` code rather than a `RUN_FINISHED` no client can act on:

```rust
use ag_ui::RunOutcome;

fn main() {
    assert!(RunOutcome::Success.validate().is_ok());
    assert!(RunOutcome::interrupt(vec![]).validate().is_err());
}
```

Deserializing does not enforce it either, deliberately: a stray empty array from a buggy
producer surfaces as a protocol error you can log rather than as an unparseable event that
kills the stream.

## API

- [`ag_ui::RunOutcome`](/ag-ui-rust/api/ag_ui/enum.RunOutcome.html)
- [`ag_ui::Interrupt`](/ag-ui-rust/api/ag_ui/struct.Interrupt.html)
- [`ag_ui::ResumeEntry`](/ag-ui-rust/api/ag_ui/struct.ResumeEntry.html) and
  [`ResumeStatus`](/ag-ui-rust/api/ag_ui/enum.ResumeStatus.html)
- [`RunContext::resume_for`](/ag-ui-rust/api/ag_ui/server/struct.RunContext.html#method.resume_for)
- The client half of the round trip: [The update stream](/ag-ui-rust/client/updates/)
