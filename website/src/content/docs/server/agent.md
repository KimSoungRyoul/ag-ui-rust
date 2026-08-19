---
title: The Agent trait
description: Implementing the trait that is this SDK's only boundary, and what the run context hands an implementation.
---

`Agent` is the whole extension point of `ag_ui::server`. There is one trait, it has one
associated type and one method, and everything else on these pages is something the run
context hands that method.

```rust
// crates/ag-ui/src/server/agent.rs — the declaration, with the docs stripped.
use ag_ui::RunOutcome;
use ag_ui::server::{AgentState, Result, RunContext};
use std::future::Future;

pub trait Agent: Send + Sync {
    type State: AgentState;

    fn run(
        &self,
        ctx: &mut RunContext<Self::State>,
    ) -> impl Future<Output = Result<RunOutcome>> + Send;
}
```

The trait deliberately says nothing about models, prompts or providers. The .NET SDK builds
on `Microsoft.Extensions.AI` because .NET has one blessed chat abstraction; Rust does not —
the ecosystem is split across `async-openai`, `rig-core` and `genai` with no winner — so
binding to any of them would make this crate useless to most of it. `ag_ui::server` therefore
depends on no LLM crate at all. Bring your own client, call it inside `run`, and emit what it
gives you. A framework integration is an `impl Agent for …` in its own crate.

## A complete agent

This compiles, runs, and is the whole program:

```rust
// src/main.rs
use ag_ui::{Event, EventType, RunAgentInput, RunOutcome};
use ag_ui::server::{Agent, Result, RunContext, run};
use futures_util::StreamExt;

struct Greeter;

impl Agent for Greeter {
    type State = ();

    async fn run(&self, ctx: &mut RunContext<()>) -> Result<RunOutcome> {
        let mut message = ctx.assistant_message()?;
        message.delta("Hello from Rust.")?;
        message.end()?;

        Ok(RunOutcome::Success)
    }
}

#[tokio::main]
async fn main() {
    let input = RunAgentInput::new("thread-1", "run-1");

    let events: Vec<Event> = run(Greeter, input)
        .map(|event| event.expect("the stream should not break"))
        .collect()
        .await;

    let types: Vec<EventType> = events.iter().map(Event::event_type).collect();
    assert_eq!(
        types,
        [
            EventType::RunStarted,
            EventType::TextMessageStart,
            EventType::TextMessageContent,
            EventType::TextMessageEnd,
            EventType::RunFinished,
        ]
    );
}
```

Two of those five events are not in the agent's code. `run()` is the driver: it emits
`RUN_STARTED` before calling the agent and exactly one of `RUN_FINISHED` / `RUN_ERROR` after
it returns, including when the body does nothing at all and when it returns `Err` through a
`?`.

The driver has no executor of its own. It owns the agent's future and polls it from the
stream, so draining the stream *is* running the agent — there is no `spawn` anywhere in the
crate, and nothing to configure. [Serving over HTTP](/ag-ui-rust/server/axum/) is what turns
that stream into a response body.

## `type State`

`State` is the shared state the client mirrors. It is deserialized from
`RunAgentInput.state` on the way in and published as `STATE_SNAPSHOT` / `STATE_DELTA` events
on the way out.

The bound is `AgentState`, and you never implement it — a blanket impl covers every type
that qualifies:

```rust
use ag_ui::RunOutcome;
use ag_ui::server::{Agent, Result, RunContext};
use serde::{Deserialize, Serialize};

/// `Serialize + DeserializeOwned + Default + Send` is the whole bound, and
/// `#[derive(Default, Serialize, Deserialize)]` satisfies all of it.
#[derive(Default, Serialize, Deserialize)]
struct Draft {
    revision: u32,
    title: String,
}

struct Editor;

impl Agent for Editor {
    type State = Draft;

    async fn run(&self, ctx: &mut RunContext<Draft>) -> Result<RunOutcome> {
        ctx.update_state(|draft| {
            draft.revision += 1;
            draft.title = "Q3 plan".into();
        })?;

        Ok(RunOutcome::Success)
    }
}
```

Use `()` when the agent keeps no state; that is what the `Greeter` above does. A request
whose `state` is `null` or an empty object decodes to `S::default()` rather than failing, so
a stateless agent works against every client. [Shared state](/ag-ui-rust/server/state/)
covers the publishing side.

## Why `async fn` and not `#[async_trait]`

`Agent::run` is written as a native `-> impl Future + Send` — an RPITIT — so an
implementation is a plain `async fn`: no macro, no `Box::pin` per call, no allocation per
run.

The cost is real and worth stating: a trait with an RPITIT method is not `dyn`-compatible,
so `Box<dyn Agent>` does not exist. When you need one — a registry of agents behind a single
endpoint, say — `DynAgent` is the boxed form, it is implemented for every `Agent`, and
`BoxAgent<S>` implements `Agent` again so the driver takes it like any other:

```rust
use ag_ui::RunOutcome;
use ag_ui::server::{Agent, BoxAgent, Result, RunContext};

struct Fixed(&'static str);

impl Agent for Fixed {
    type State = ();

    async fn run(&self, ctx: &mut RunContext<()>) -> Result<RunOutcome> {
        ctx.say(self.0)?;
        Ok(RunOutcome::Success)
    }
}

let agents: Vec<BoxAgent<()>> = vec![Box::new(Fixed("a")), Box::new(Fixed("b"))];
assert_eq!(agents.len(), 2);
```

The difference is one boxed future per run. `Agent` is also implemented for `&A` and
`Arc<A>`, which is how one agent value serves many concurrent requests without being cloned.

## Why `&mut RunContext` and not `RunContext`

The agent borrows the context; it never owns it. That is what lets the driver emit the
terminal event *after* `run` returns, through the same transformer chain and the same
ordering verifier the agent was using. Handing the context over by value would drop both
with the agent's last statement, and the terminal event would go out unverified.

## What the context hands you

`RunContext<S>` is the request, the state, the event sink and the cancellation flag in one
value. The reading half needs no `&mut`:

```rust
use ag_ui::{Message, RunAgentInput, Tool};
use ag_ui::server::RunContext;
use serde_json::json;

fn main() -> ag_ui::server::Result<()> {
    let mut input = RunAgentInput::new("thread-1", "run-1");
    input.messages = vec![Message::user("msg-1", "what is the weather in Seoul?")];
    input.tools = vec![Tool::new(
        "get_weather",
        "Look up the current weather for a city.",
        json!({"type": "object", "properties": {"city": {"type": "string"}}}),
    )];

    // `RunContext::new` is the unit-test harness: a context plus the receiving
    // end of its event stream, with no driver and so no RUN_STARTED.
    let (ctx, _events) = RunContext::<()>::new(input)?;

    assert_eq!(ctx.thread_id().as_str(), "thread-1");
    assert_eq!(ctx.run_id().as_str(), "run-1");
    assert_eq!(ctx.messages().len(), 1);
    assert_eq!(
        ctx.last_user_text().as_deref(),
        Some("what is the weather in Seoul?")
    );
    assert!(ctx.tool("get_weather").is_some());
    assert!(ctx.tool("send_email").is_none());
    assert!(!ctx.is_resume());

    Ok(())
}
```

| Accessor | What it answers |
| --- | --- |
| `thread_id`, `run_id`, `parent_run_id` | which conversation, which run, which run spawned it |
| `messages` | the conversation history, oldest first |
| `last_user_text` | the turn you are almost always answering |
| `tools`, `tool(name)` | what the client offered — see [Tool calls](/ag-ui-rust/server/tools/) |
| `context`, `forwarded_props` | ambient entries and opaque passthrough |
| `resume`, `resume_for`, `is_resume` | answers to a previous pause — see [Human in the loop](/ag-ui-rust/server/interrupts/) |
| `input` | the whole `RunAgentInput`, for anything the rest does not cover |

`last_user_text` drops the non-text parts of a multimodal message, and returns `None` when
the history holds no user message at all — which is not the same as a user who sent an empty
one. Reach into `messages` directly when the images matter.

The writing half all takes `&mut self`, and is covered a page at a time:
[streaming text](/ag-ui-rust/server/text/), [tool calls](/ag-ui-rust/server/tools/),
[state](/ag-ui-rust/server/state/). `ctx.emit(event)` underneath them all is the escape
hatch for anything with no typed emitter.

## Bracketing a run with steps

`ctx.step(name)` emits `STEP_STARTED` and returns a guard that emits `STEP_FINISHED` when it
drops — including on the early return a `?` produces. A step is a *scope* rather than a
stream, so unlike the message and tool-call handles the guard dereferences to the run
context, and everything nests inside it:

```rust
use ag_ui::{Event, EventType, RunAgentInput, RunOutcome};
use ag_ui::server::{Agent, Result, RunContext, run};
use futures_util::StreamExt;

struct Researcher;

impl Agent for Researcher {
    type State = ();

    async fn run(&self, ctx: &mut RunContext<()>) -> Result<RunOutcome> {
        let mut step = ctx.step("research")?;
        step.say("Looking it up.")?;   // through Deref, on the context
        // STEP_FINISHED goes out here, whether or not the `?` above fired.
        drop(step);

        Ok(RunOutcome::Success)
    }
}

#[tokio::main]
async fn main() {
    let events: Vec<Event> = run(Researcher, RunAgentInput::new("t", "r"))
        .map(|event| event.expect("the stream should not break"))
        .collect()
        .await;

    let types: Vec<EventType> = events.iter().map(Event::event_type).collect();
    assert_eq!(
        types,
        [
            EventType::RunStarted,
            EventType::StepStarted,
            EventType::TextMessageStart,
            EventType::TextMessageContent,
            EventType::TextMessageEnd,
            EventType::StepFinished,
            EventType::RunFinished,
        ]
    );
}
```

Steps are optional. Nothing in the protocol requires one, and an agent that emits a single
message is clearer without.

## How a run ends

`run` returns `Result<RunOutcome>`, and the three ways it can return are the three ways a run
can end:

- `Ok(RunOutcome::Success)` — the run completed. The driver emits `RUN_FINISHED` with a
  `success` outcome.
- `Ok(RunOutcome::Interrupt { .. })` — the run is paused, waiting on a person. Still a
  `RUN_FINISHED`, carrying the pending interrupts. See
  [Human in the loop](/ag-ui-rust/server/interrupts/).
- `Err(_)` — the run failed. The driver emits `RUN_ERROR` carrying the message and a code.
  An agent error is never a panic and never a truncated stream. See
  [Errors and cancellation](/ag-ui-rust/server/errors/).

:::caution
A *panic* inside the agent is not caught. It unwinds through whoever is polling the stream,
as it would through any other future, and over HTTP the client sees a truncated body because
the `200` has already been sent. Return `Err(Error::agent(…))` for failures you expect.
:::

## API

- [`ag_ui::server::Agent`](/ag-ui-rust/api/ag_ui/server/trait.Agent.html)
- [`ag_ui::server::AgentState`](/ag-ui-rust/api/ag_ui/server/trait.AgentState.html)
- [`ag_ui::server::RunContext`](/ag-ui-rust/api/ag_ui/server/struct.RunContext.html)
- [`ag_ui::server::run`](/ag-ui-rust/api/ag_ui/server/fn.run.html) and
  [`Runner`](/ag-ui-rust/api/ag_ui/server/struct.Runner.html)
- [`ag_ui::server::DynAgent`](/ag-ui-rust/api/ag_ui/server/trait.DynAgent.html) and
  [`BoxAgent`](/ag-ui-rust/api/ag_ui/server/type.BoxAgent.html)
- [`ag_ui::RunOutcome`](/ag-ui-rust/api/ag_ui/enum.RunOutcome.html)
