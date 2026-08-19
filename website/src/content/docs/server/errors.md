---
title: Errors and cancellation
description: How a run reports failure to the client, and what happens to an agent when the caller goes away mid-stream.
---

A run that fails is still a run. The driver turns whatever escapes `Agent::run` into a
`RUN_ERROR` event, so the client gets a well-formed stream that ends by saying what went
wrong — never a panic, and never a body that simply stops.

```rust
// src/agent.rs
use ag_ui::{Event, RunAgentInput, RunOutcome};
use ag_ui::server::{Agent, Error, Result, RunContext, run};
use futures_util::StreamExt;

struct Flaky;

impl Agent for Flaky {
    type State = ();

    async fn run(&self, ctx: &mut RunContext<()>) -> Result<RunOutcome> {
        ctx.say("Looking that up.")?;
        Err(Error::agent("the weather service is down"))
    }
}

#[tokio::main]
async fn main() {
    let events: Vec<Event> = run(Flaky, RunAgentInput::new("t", "r"))
        .map(|event| event.expect("the stream should not break"))
        .collect()
        .await;

    let Some(Event::RunError(error)) = events.last() else {
        panic!("a failed run ends in RUN_ERROR: {events:?}");
    };
    assert_eq!(error.message, "agent error: the weather service is down");
    assert_eq!(error.code.as_deref(), Some("AGENT_ERROR"));
}
```

`Error::agent` wraps any `Into<Box<dyn std::error::Error + Send + Sync>>`, which is most
things — a `String`, a `&str`, or your own error type — so `?` on your own failures usually
needs one `map_err(Error::agent)` and nothing else.

## The variants

`ag_ui::server::Error` is what every method in the crate returns, through the
`Result<T, E = Error>` alias. Each variant has a stable code that lands on the `RUN_ERROR`
event:

| Variant | Code | Raised when |
| --- | --- | --- |
| `Protocol` | `PROTOCOL` | a core type rejected a value — an `interrupt` outcome with no interrupts, say |
| `Json` | `SERIALIZATION` | state, tool arguments or a tool result would not convert to or from JSON |
| `Verification` | `PROTOCOL_VIOLATION` | the emitted stream broke an ordering rule |
| `Cancelled` | `CANCELLED` | the run was cancelled, usually because the client disconnected |
| `Disconnected` | `DISCONNECTED` | the consumer dropped the event stream |
| `Agent` | `AGENT_ERROR` | your code failed. Built with `Error::agent` |

`is_cancelled()` and `is_disconnected()` are the two you are likely to branch on — both mean
"stop, nobody is listening" rather than "something is broken".

The enum is `#[non_exhaustive]`, as every error type in this workspace is. `Event` and
`EventType` deliberately are **not**, and the asymmetry is the point: a new protocol event
*should* be a compile error for consumers, because a `_` arm is exactly the construct that
turns "a new event arrived" into no diagnostic at all. Nobody wants an exhaustive match over
failure modes, callers route on a handful of variants and fall through on the rest, and a new
failure mode is not a wire-contract change. The reasoning is written out in `docs/DESIGN.md`.

:::caution
A *panic* is not an error. It is not caught anywhere in this crate; it unwinds through
whoever is polling the stream, as it would through any other future. Over HTTP the status
line has already been sent by then, so the client sees a truncated body. Return
`Err(Error::agent(…))` for failures you expect, and reach for `tower_http::catch_panic` only
for the ones you did not.
:::

## Protocol verification

The borrow checker stops two overlapping messages. What it cannot see is a raw `ctx.emit`,
so an ordering state machine watches every event on its way out — on by default, on the
server, where the bug is caused rather than three network hops downstream:

```rust
use ag_ui::{Event, RunAgentInput};
use ag_ui::server::{Error, RunContext, Rule};

fn main() -> ag_ui::server::Result<()> {
    let (mut ctx, _events) = RunContext::<()>::new(RunAgentInput::new("t", "r"))?;

    // Content for a message that was never started.
    let error = ctx
        .emit(Event::text_message_content("msg-1", "hello"))
        .expect_err("the verifier should reject this");

    let Error::Verification(failure) = &error else {
        panic!("expected a verification failure, got {error}");
    };
    assert_eq!(failure.rule, Rule::NotOpen);
    assert_eq!(error.code(), "PROTOCOL_VIOLATION");
    Ok(())
}
```

There are seven rules, and `Rule::describe()` states each in one sentence:

| Rule | What it forbids |
| --- | --- |
| `RunEnded` | anything after `RUN_FINISHED` or `RUN_ERROR` |
| `DuplicateRunStarted` | a second `RUN_STARTED` |
| `DuplicateStart` | opening a message, reasoning block, tool call or step whose id is already open |
| `NotOpen` | content or a terminator for something that was never opened |
| `UnknownId` | a tool result for a call id that was never introduced |
| `OutOfOrder` | a tool result before the call's `TOOL_CALL_END` |
| `OpenAtFinish` | `RUN_FINISHED` while anything is still open |

`RUN_ERROR` is exempt from `OpenAtFinish` — a run that blew up mid-message could not have
closed it. That exemption is also what keeps a rejected `RUN_FINISHED` from leaving a run
with no terminal event at all: the driver reports the rejection as a `RUN_ERROR` instead.

A `VerificationError` names the event, the rule and the id involved, and in debug builds it
appends a dump of everything still open, which is usually enough to spot the missing
terminator:

```text
TEXT_MESSAGE_CONTENT breaks rule `not-open` (content and terminators require
a matching start): message "msg-2" is not open [open: messages={"msg-1"}]
```

The cost is a handful of `HashSet`s and one lookup per event. Turning off the `verify`
feature — the crate's only feature, on by default — replaces the whole state machine with a
zero-sized type whose `observe` is an inlined `Ok(())`, and removes the `Verification`
variant's only source. The debug-only dump is the expensive part, which is why it is
debug-only.

## Cancellation

`CancellationToken` is a shared "stop now" flag: an `AtomicBool` and a waker list, cloned
cheaply, every clone referring to the same flag. It is deliberately not
`tokio_util::sync::CancellationToken` — this crate builds for wasm and for non-tokio
executors, and taking `tokio_util` would end that.

A transport trips the token when the client disconnects or a deadline passes. What makes
that work without any cooperation from the agent is that **every emit after cancellation
fails**, so the next `?` unwinds the run:

```rust
use ag_ui::{Event, EventType, RunAgentInput, RunOutcome};
use ag_ui::server::{Agent, Result, RunContext, run};
use futures_util::StreamExt;

struct Chatty;

impl Agent for Chatty {
    type State = ();

    async fn run(&self, ctx: &mut RunContext<()>) -> Result<RunOutcome> {
        ctx.say("one")?;
        // Standing in for the transport noticing the client hung up.
        ctx.cancel_token().cancel();
        ctx.say("two")?;   // fails, and `?` returns
        ctx.say("three")?;
        Ok(RunOutcome::Success)
    }
}

#[tokio::main]
async fn main() {
    let events: Vec<Event> = run(Chatty, RunAgentInput::new("t", "r"))
        .map(|event| event.expect("the stream should not break"))
        .collect()
        .await;

    let said: Vec<&str> = events
        .iter()
        .filter_map(|event| match event {
            Event::TextMessageContent(content) => Some(content.delta.as_str()),
            _ => None,
        })
        .collect();
    assert_eq!(said, ["one"]);

    // The terminal event goes out regardless of the cancellation.
    assert_eq!(events.last().map(Event::event_type), Some(EventType::RunError));
    let Some(Event::RunError(error)) = events.last() else {
        panic!("{events:?}");
    };
    assert_eq!(error.code.as_deref(), Some("CANCELLED"));
}
```

An agent that wants to notice sooner has four ways to ask:

| Method | Shape |
| --- | --- |
| `is_cancelled()` | a `bool`, for a loop condition |
| `check_cancelled()` | `Result<()>`, for a bare `?` between steps |
| `cancelled()` | a `'static` future that resolves when the token trips |
| `until_cancelled(f)` | races `f` against cancellation, `None` if cancellation won |

`until_cancelled` is the one that matters for a long model call, because a request already in
flight is what cancellation is meant to stop paying for:

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

    let answer = ctx.until_cancelled(call_the_model()).await;
    assert_eq!(answer.as_deref(), Some("the model's reply"));

    // A future that never resolves, against a token that has been tripped.
    ctx.cancel_token().cancel();
    let never = futures_util::future::pending::<String>();
    assert!(ctx.until_cancelled(never).await.is_none());
    Ok(())
}
```

`until_cancelled` is deliberately not an `async fn`. That would capture `&self` in the
returned future, and a future holding a borrow of the run context is only `Send` if the
context is `Sync` — which it is not, since a stream transformer only has to be `Send`.

`cancelled()` returns a future that owns a clone of the token rather than borrowing one: one
`Arc` bump, in exchange for a `'static` future an agent can hold across an await without
dragging a borrow of the run context along with it.

## Who trips the token

Over HTTP, [`ag_ui::axum`](/ag-ui-rust/server/axum/) does it. The response body owns the run,
so when the client goes away hyper drops the body and the run goes with it; the body also
holds a guard that trips the token on drop, and disarms itself if the run got to finish. That
second part is what reaches everything the run touched *outside* itself — a spawned tool
call, an in-flight model request, a lock it holds.

If you are writing your own transport, take the token from
`Runner::cancellation_token()` before `run` consumes the runner, and trip it when your
connection ends.

## API

- [`ag_ui::server::Error`](/ag-ui-rust/api/ag_ui/server/enum.Error.html) and
  [`Result`](/ag-ui-rust/api/ag_ui/server/type.Result.html)
- [`ag_ui::server::Rule`](/ag-ui-rust/api/ag_ui/server/enum.Rule.html) and
  [`VerificationError`](/ag-ui-rust/api/ag_ui/server/struct.VerificationError.html)
- [`ag_ui::server::verify`](/ag-ui-rust/api/ag_ui/server/verify/index.html) — the state
  machine, rule by rule
- [`ag_ui::server::CancellationToken`](/ag-ui-rust/api/ag_ui/server/struct.CancellationToken.html)
  and [`Cancelled`](/ag-ui-rust/api/ag_ui/server/struct.Cancelled.html)
- [Feature flags](/ag-ui-rust/reference/features/) for what `verify` costs and removes
