---
name: ag-ui-rust-server
description: "MUST USE when writing Rust against ag-ui-rust to host an agent — the crate ag-ui with its `serve` and `axum` features, plus ag-ui-a2ui. UNCONVENTIONAL, and wrong from memory: this is ONE crate named ag-ui, not ag-ui-core / ag-ui-server / ag-ui-axum — those registry names belong to an unrelated community SDK — and the server lives under ag_ui::serve, the axum binding under ag_ui::axum, both behind features. Events are emitted through RAII typestate handles — ctx.assistant_message() then delta() then end() — never as raw start/content/end calls; the emit path is SYNCHRONOUS with no .await, because the handle emits its terminator on Drop; two open handles at once is a borrow-check error, not a runtime one; Agent::run is a native async fn (RPITIT), so #[async_trait] does not apply and Box<dyn Agent> does not exist (use BoxAgent). Covers RunContext accessors, RunOutcome, tool calls the agent answers vs. the client runs, shared state as automatic snapshot/delta, human-in-the-loop interrupts, the ordering verifier and its seven rules, cancellation, and mounting with route_agui. Triggers on: ag-ui-rust, ag_ui::serve, ag_ui::axum, ag_ui, impl Agent for, RunContext, RunOutcome, route_agui, AG-UI agent in Rust, Rust backend for CopilotKit or AG-UI, A2UI surface from a Rust agent."
---

# Serving an AG-UI agent in Rust

Docs: <https://kimsoungryoul.github.io/ag-ui-rust/> · this skill is written against
workspace version **0.1.0**. If the API here disagrees with the compiler, the compiler is
right and the skill is stale — see `ag-ui-rust-update`.

## Adding the crates

**One crate, `ag-ui`.** Not `ag-ui-core` / `ag-ui-server` / `ag-ui-client` — those names on
crates.io are a different, unrelated project. Which half of the protocol you get is a
feature; `axum` implies `serve`:

```toml
# Cargo.toml
[dependencies]
ag-ui = { version = "0.1", features = ["axum"] }
axum = "0.8"
tokio = { version = "1", features = ["rt-multi-thread", "macros", "net"] }
```

There are no release tags yet, so pin `rev = "<sha>"` for anything you rely on. Rust
**1.85+**, edition 2024. No LLM crate is involved anywhere: bring your own model client and
call it inside `run`.

## The whole extension point

One trait, one associated type, one method. `ctx` is the request, the state, the event sink
and the cancellation flag in one value.

```rust,no_run
// src/main.rs
use ag_ui::axum::RouterExt;
use ag_ui::RunOutcome;
use ag_ui::serve::{Agent, Result, RunContext};
use axum::Router;

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
    let app: Router = Router::new().route_agui("/agent", Greeter);

    let listener = tokio::net::TcpListener::bind("127.0.0.1:3000").await.unwrap();
    axum::serve(listener, app).await.unwrap();
}
```

`RUN_STARTED` and the terminal `RUN_FINISHED` / `RUN_ERROR` are the driver's, not yours.
`run(agent, input)` returns a `Stream` of events and owns the agent's future — draining the
stream *is* running the agent, so there is no `spawn` and nothing to configure.

## Emitting

| Want | Call |
| --- | --- |
| A whole message at once | `ctx.say(text)?` → `MessageId` |
| Stream one | `ctx.assistant_message()?` → `delta(..)`, `end()` |
| Reasoning | `ctx.think(text)?`, or `ctx.reasoning()?` |
| A tool call | `ctx.tool_call(name)?` → `args`/`args_json`, then `result_json` or `end` |
| A scope | `ctx.step(name)?` — guard emits `STEP_FINISHED` on drop, derefs to the context |
| Anything untyped | `ctx.emit(event)?` |

Handles are RAII: the terminator goes out on `Drop`, including on the early return a `?`
produces. `end()` is worth calling anyway, because `Drop` has nowhere to report an error and
swallows it.

```rust
use ag_ui::{Event, RunAgentInput, TextMessageRole};
use ag_ui::serve::RunContext;

fn main() -> ag_ui::serve::Result<()> {
    // `RunContext::new` is the unit-test harness: a context plus the receiving
    // end of its event stream. Inside an agent this is just `ctx`.
    let (mut ctx, mut events) = RunContext::<()>::new(RunAgentInput::new("t", "r"))?;

    let mut message = ctx.assistant_message()?;
    for word in ["Hello", ", ", "world"] {
        message.delta(word)?;   // one delta per provider chunk; no buffering
    }
    message.end()?;

    assert_eq!(
        events.drain(),
        vec![
            Event::text_message_start("r-msg-1", TextMessageRole::Assistant),
            Event::text_message_content("r-msg-1", "Hello"),
            Event::text_message_content("r-msg-1", ", "),
            Event::text_message_content("r-msg-1", "world"),
            Event::text_message_end("r-msg-1"),
        ]
    );
    Ok(())
}
```

**`delta` is not async.** `Drop` cannot be async in Rust, so a handle that emits its own
terminator cannot `await`. Emitters push into an unbounded channel and the transport drains
it; nothing blocks. Do not write `.await` on an emitter, and do not reach for `async_drop`.

**Ids are strings, and derived.** `r-msg-1` is the run id plus a counter — no `uuid`
dependency anywhere. `ThreadId`, `RunId`, `MessageId` are newtypes over `String`.
`message_with_id` / `tool_call_with_id` take your own.

### Two open handles do not compile

A handle borrows the run context mutably for as long as it lives:

```rust,compile_fail
use ag_ui::serve::RunContext;

fn interleave(ctx: &mut RunContext<()>) {
    let mut first = ctx.assistant_message().unwrap();
    // error[E0499]: cannot borrow `*ctx` as mutable more than once at a time
    let mut second = ctx.assistant_message().unwrap();
    first.delta("a").unwrap();
    second.delta("b").unwrap();
}
```

A handle still reaches the **state** (`handle.state_mut()`, `handle.publish_state()`) and can
`handle.emit(..)` the unordered families — `STATE_*`, `ACTIVITY_*`, `CUSTOM`, `RAW`. What it
cannot do is open a second block.

For parallel tool calls from a provider that interleaves argument fragments, accumulate per
call and emit each one whole. Emitting interleaved calls by hand with `ctx.emit` is legal —
the verifier keys by id — but the handles cannot express it.

## Tool calls

```rust
use ag_ui::{Event, EventType, RunAgentInput};
use ag_ui::serve::RunContext;
use serde_json::json;

fn main() -> ag_ui::serve::Result<()> {
    let (mut ctx, mut events) = RunContext::<()>::new(RunAgentInput::new("t", "r"))?;

    // Answered by the agent: `result` emits TOOL_CALL_END then TOOL_CALL_RESULT.
    let mut call = ctx.tool_call("get_weather")?;
    call.args_json(&json!({"city": "Seoul"}))?;
    let _tool_message_id = call.result_json(&json!({"tempC": 21}))?;

    // Left for the client to run: `end()` and nothing else. The result arrives
    // as a tool message on the *next* request.
    let mut front = ctx.tool_call("open_settings_panel")?;
    front.args_json(&json!({"tab": "billing"}))?;
    front.end()?;

    let types: Vec<EventType> = events.drain().iter().map(Event::event_type).collect();
    assert_eq!(types.len(), 7);
    Ok(())
}
```

`args` takes a *fragment*, not a value, because providers stream partial JSON. `raw_args()`
is the buffer; `parse_args()` deserializes once the stream is done.

`RunAgentInput.tools` is a **capability list, not an allow-list**: emitting a call for a name
the client never offered is a well-formed stream and the verifier says nothing. That is what
lets an agent answer its own calls — `render_a2ui` carries an A2UI surface no client could
execute. Gate on `ctx.tool(name).is_some()` yourself when you want the stricter rule.

## Shared state

`type State` is your `serde` struct; the client mirrors it. You never choose between snapshot
and delta — the publisher diffs against the last snapshot with RFC 6902 and sends whichever
is smaller, with the first publish of a run always a snapshot.

```rust
use ag_ui::{Event, EventType, RunAgentInput};
use ag_ui::serve::RunContext;
use serde::{Deserialize, Serialize};

#[derive(Default, Serialize, Deserialize)]
struct Doc {
    step: u32,
    notes: Vec<String>,
}

fn main() -> ag_ui::serve::Result<()> {
    let (mut ctx, mut events) = RunContext::<Doc>::new(RunAgentInput::new("t", "r"))?;

    ctx.update_state(|doc| {
        doc.step = 1;
        doc.notes.push("the document the user is editing".repeat(8));
    })?;

    ctx.state_mut().step = 2;
    ctx.publish_state()?;      // a no-op when nothing changed

    let types: Vec<EventType> = events.drain().iter().map(Event::event_type).collect();
    assert_eq!(types, [EventType::StateSnapshot, EventType::StateDelta]);
    Ok(())
}
```

Publish once per change, not once per run — the point is that the client watches it happen.
`()` for an agent that shares nothing. A `null` or `{}` state decodes to `S::default()`.

## Ending a run

- `Ok(RunOutcome::Success)` — done.
- `Ok(RunOutcome::interrupt(pending))` — paused for a human. Still a `RUN_FINISHED`; the
  connection closes and **no server-side session survives**.
- `Err(Error::agent(..))` — failed. Becomes `RUN_ERROR`, never a truncated stream.

A panic is *not* an error and is not caught: it unwinds through whoever polls the stream, and
over HTTP the client sees a truncated body because the `200` is long sent.

The resumed run is a new run that rebuilds its position from `messages`, `state` and
`resume` — it remembers nothing. An agent paused on several decisions must re-report every
one still unanswered, and the client must answer them all in one request, or the pair never
terminates. `references/state-and-interrupts.md` has the round trip, the verifier's seven
rules, and cancellation.

## Mounting

`route_agui(path, agent)` is `route(path, post(handler))`. The handler reads only the
request, so it is a `Handler<_, S>` for **every** router state — mounting places no bound on
your `AppState`, and an agent that needs values from it should capture them at construction
rather than extracting `State` inside the run.

```rust
use ag_ui::axum::{AgentEndpoint, RouterExt};
use ag_ui::RunOutcome;
use ag_ui::serve::{Agent, FilterToolCalls, Result, RunContext};
use axum::Router;
use std::time::Duration;

struct CartAgent;

impl Agent for CartAgent {
    type State = ();

    async fn run(&self, _ctx: &mut RunContext<()>) -> Result<RunOutcome> {
        Ok(RunOutcome::Success)
    }
}

fn main() {
    let endpoint = AgentEndpoint::new(CartAgent)
        // A *closure*: transformers are state machines, so the endpoint builds
        // a fresh chain per run rather than sharing one across concurrent runs.
        .transformer(|| FilterToolCalls::deny(["internal_debug"]))
        .keep_alive(Duration::from_secs(15))   // off by default
        .echo_input(false);                    // off by default

    let _app: Router = Router::new().route_agui_with("/agent", endpoint);
}
```

A failed run is still `200` — the failure is a `RUN_ERROR` event, which is what lets a client
tell "the agent errored" from "the network died". `400`/`406`/`413`/`415` are the refusals
that happen before a run starts. There is no `AgUiLayer`: by the time a tower layer sees the
response the events are already SSE bytes. `AgUiInput` (extractor) and `SseResponse` are
there for a hand-written handler.

## Do not write

| Instead of | Write |
| --- | --- |
| `ag-ui-server = "0.1"` | the git dependency above — the registry name is someone else's crate |
| `#[async_trait]` on `impl Agent` | a plain `async fn run` |
| `Box<dyn Agent>` | `BoxAgent<S>` (`DynAgent` is the object-safe half) |
| `msg.delta(..).await` | `msg.delta(..)?` — the emit path is synchronous |
| `ctx.emit(Event::text_message_start(..))` | `ctx.assistant_message()?` |
| `tokio::spawn` around the run | nothing; polling the stream runs the agent |
| `Uuid::new_v4()` for ids | nothing; ids are derived strings, or `*_with_id` |
| `Err(..)` when a human declines | `ResumeStatus::Cancelled` — a decline is a successful run |

## Deeper

- [The Agent trait](https://kimsoungryoul.github.io/ag-ui-rust/server/agent/) ·
  [Streaming text](https://kimsoungryoul.github.io/ag-ui-rust/server/text/) ·
  [Tool calls](https://kimsoungryoul.github.io/ag-ui-rust/server/tools/)
- [Shared state](https://kimsoungryoul.github.io/ag-ui-rust/server/state/) ·
  [Human in the loop](https://kimsoungryoul.github.io/ag-ui-rust/server/interrupts/) ·
  [Errors and cancellation](https://kimsoungryoul.github.io/ag-ui-rust/server/errors/)
- [Serving over HTTP](https://kimsoungryoul.github.io/ag-ui-rust/server/axum/) ·
  [A2UI](https://kimsoungryoul.github.io/ag-ui-rust/a2ui/) ·
  [Feature flags](https://kimsoungryoul.github.io/ag-ui-rust/reference/features/)
- rustdoc for all five crates:
  <https://kimsoungryoul.github.io/ag-ui-rust/api/ag_ui/serve/index.html>
- The client half is the `ag-ui-rust-client` skill.
