# Interrupts, errors, verification, cancellation

Read this when a run has to pause for a person, when you need to know what a failure looks
like on the wire, or when an emit is being rejected.

## The human-in-the-loop round trip

A pause is a **finished run**: `RUN_FINISHED` whose outcome says `interrupt` and lists what
is pending. The connection closes, nothing is held open, and the next request is an ordinary
request in the same thread that happens to carry answers.

```rust
use ag_ui::{Event, Interrupt, ResumeEntry, ResumeStatus, RunAgentInput, RunOutcome};
use ag_ui::server::{Agent, Error, Result, RunContext, run};
use futures_util::StreamExt;
use serde_json::{Value, json};

const APPROVE_DEPLOY: &str = "approve-deploy";

struct Deployer;

impl Agent for Deployer {
    type State = ();

    async fn run(&self, ctx: &mut RunContext<()>) -> Result<RunOutcome> {
        let Some(answer) = ctx.resume_for(APPROVE_DEPLOY) else {
            ctx.say("Deploying to production needs a human.")?;
            return Ok(RunOutcome::interrupt(vec![Interrupt {
                id: APPROVE_DEPLOY.to_owned(),
                reason: "tool_approval".to_owned(),
                message: Some("Deploy build 42 to production?".to_owned()),
                ..Default::default()
            }]));
        };

        match answer.status {
            ResumeStatus::Resolved => {
                let build = answer
                    .payload
                    .as_ref()
                    .and_then(|payload| payload.get("build"))
                    .and_then(Value::as_u64)
                    .ok_or_else(|| Error::agent("the approval carried no build number"))?;
                ctx.say(format!("Deployed build {build}."))?;
            }
            // A decline is an answer, not a failure: carry on down the other
            // branch and finish successfully.
            ResumeStatus::Cancelled => {
                ctx.say("Left production alone.")?;
            }
        }

        Ok(RunOutcome::Success)
    }
}

#[tokio::main]
async fn main() {
    let mut resumed = RunAgentInput::new("deploy", "run-2");
    resumed.resume = Some(vec![ResumeEntry::resolved(
        APPROVE_DEPLOY,
        json!({"build": 42}),
    )]);

    let events: Vec<Event> = run(Deployer, resumed)
        .map(|event| event.expect("the stream should not break"))
        .collect()
        .await;

    assert!(events.iter().any(|event| matches!(
        event,
        Event::TextMessageContent(content) if content.delta == "Deployed build 42."
    )));
}
```

`Interrupt` fields: `id` (correlation, echoed back as `ResumeEntry::interrupt_id`), `reason`
(machine-readable, e.g. `"tool_approval"`), `message`, `tool_call_id`, `response_schema`
(JSON Schema, lets a client render a form), `expires_at`, `metadata`.
`Interrupt::new(id, reason)` sets the two required ones.

Reading the answers: `ctx.is_resume()`, `ctx.resume()`, `ctx.resume_for(id)`.

### The mistake that never terminates

The paused run is gone — its future was dropped when the stream ended. Only the answers
*this request* carries exist. So an agent pausing on more than one decision must recompute
what is still outstanding and report all of it:

```rust
use ag_ui::{Interrupt, RunOutcome};
use ag_ui::server::{Agent, Result, RunContext};

struct Planner;

impl Agent for Planner {
    type State = ();

    async fn run(&self, ctx: &mut RunContext<()>) -> Result<RunOutcome> {
        // The filter runs before anything is emitted: `resume_for` borrows the
        // context immutably and the emitters want it mutably.
        let pending: Vec<Interrupt> = ["approve-budget", "confirm-date"]
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

`RunOutcome::interrupt(vec![])` is constructible and the protocol forbids it; the driver
validates before emitting, so it becomes a `RUN_ERROR` with the `PROTOCOL` code.

## Errors

`ag_ui::server::Error` is what every method returns, through `Result<T, E = Error>`. Each
variant has a stable code that lands on `RUN_ERROR`:

| Variant | Code | Raised when |
| --- | --- | --- |
| `Protocol` | `PROTOCOL` | a core type rejected a value |
| `Json` | `SERIALIZATION` | state, tool args or a result would not convert |
| `Verification` | `PROTOCOL_VIOLATION` | the emitted stream broke an ordering rule |
| `Cancelled` | `CANCELLED` | the run was cancelled, usually a client disconnect |
| `Disconnected` | `DISCONNECTED` | the consumer dropped the event stream |
| `Agent` | `AGENT_ERROR` | your code failed — `Error::agent(..)` |

`Error::agent` wraps anything `Into<Box<dyn std::error::Error + Send + Sync>>`, so your own
error type needs one `map_err(Error::agent)`. `is_cancelled()` and `is_disconnected()` both
mean "stop, nobody is listening". The enum is `#[non_exhaustive]`; `Event` and `EventType`
deliberately are not.

## The ordering verifier

On by default, on the server, in release builds too — the `verify` feature compiles it to a
zero-sized type. It catches what the borrow checker cannot see, which is `ctx.emit`:

```rust
use ag_ui::{Event, RunAgentInput};
use ag_ui::server::{Error, RunContext, Rule};

fn main() -> ag_ui::server::Result<()> {
    let (mut ctx, _events) = RunContext::<()>::new(RunAgentInput::new("t", "r"))?;

    let error = ctx
        .emit(Event::text_message_content("msg-1", "hello"))
        .expect_err("content for a message that was never started");

    let Error::Verification(failure) = &error else { panic!("{error}") };
    assert_eq!(failure.rule, Rule::NotOpen);
    assert_eq!(error.code(), "PROTOCOL_VIOLATION");
    Ok(())
}
```

Seven rules: `RunEnded`, `DuplicateRunStarted`, `DuplicateStart`, `NotOpen`, `UnknownId`,
`OutOfOrder`, `OpenAtFinish`. `RUN_ERROR` is exempt from `OpenAtFinish` — a run that blew up
mid-message could not have closed it.

## Cancellation

`ag_ui::server::CancellationToken` — deliberately not `tokio_util`'s, because these crates
build for wasm. A transport trips it when the client hangs up, and **every emit after
cancellation fails**, so the next `?` unwinds the run without any cooperation from the agent.

To notice sooner: `is_cancelled()`, `check_cancelled()?`, `cancelled()` (a `'static` future),
and `until_cancelled(f)` — the one that matters, because an in-flight model request is what
cancellation is meant to stop paying for:

```rust
use ag_ui::{RunAgentInput, RunOutcome};
use ag_ui::server::{Agent, Error, Result, RunContext};

struct Slow;

impl Agent for Slow {
    type State = ();

    async fn run(&self, ctx: &mut RunContext<()>) -> Result<RunOutcome> {
        let reply = ctx
            .until_cancelled(call_the_model())
            .await
            .ok_or(Error::Cancelled)?;
        ctx.say(reply)?;
        Ok(RunOutcome::Success)
    }
}

async fn call_the_model() -> String {
    "the model's reply".to_owned()
}

#[tokio::main]
async fn main() -> ag_ui::server::Result<()> {
    let (ctx, _events) = RunContext::<()>::new(RunAgentInput::new("t", "r"))?;
    assert_eq!(
        ctx.until_cancelled(call_the_model()).await.as_deref(),
        Some("the model's reply")
    );
    Ok(())
}
```

Over HTTP `ag_ui::axum` trips the token: the response body owns the run, and a guard on the
body disarms itself if the run finished. Writing your own transport, take the token from
`Runner::cancellation_token()` *before* `run` consumes the runner.

## Sources

- <https://kimsoungryoul.github.io/ag-ui-rust/server/interrupts/>
- <https://kimsoungryoul.github.io/ag-ui-rust/server/errors/>
- <https://kimsoungryoul.github.io/ag-ui-rust/server/state/>
