---
title: Tool calls
description: Emitting tool calls from an agent, answering them within the run, and reading the tool list a client offered.
---

A tool call is bracketed like a message: `TOOL_CALL_START`, some number of `TOOL_CALL_ARGS`,
then `TOOL_CALL_END`, all carrying the same call id. `ctx.tool_call(name)` emits the start
and returns a handle that emits the end on `Drop`, exactly as
[a message handle](/ag-ui-rust/server/text/) does.

What is different is the ending. A call can be *answered* — the agent runs the tool itself
and reports what it got — or *left open for the client*, which runs it and sends the result
back on the next request.

## A call the agent answers itself

```rust
use ag_ui::{Event, EventType, RunAgentInput};
use ag_ui::server::RunContext;
use serde_json::json;

fn main() -> ag_ui::server::Result<()> {
    let (mut ctx, mut events) = RunContext::<()>::new(RunAgentInput::new("t", "r"))?;

    let mut call = ctx.tool_call("get_weather")?;
    call.args_json(&json!({"city": "Seoul"}))?;
    // The tool's own work goes here.
    let result_id = call.result_json(&json!({"tempC": 21}))?;

    assert_eq!(result_id.as_str(), "r-msg-1");
    let types: Vec<EventType> = events.drain().iter().map(Event::event_type).collect();
    assert_eq!(
        types,
        [
            EventType::ToolCallStart,
            EventType::ToolCallArgs,
            EventType::ToolCallEnd,
            EventType::ToolCallResult,
        ]
    );
    Ok(())
}
```

`result` emits `TOOL_CALL_END` and then `TOOL_CALL_RESULT`, and returns the id of the tool
message carrying the result — the id the conversation history will use for it. That id is
allocated when the handle is created, not when the result is emitted, which is what lets the
handle finish the call without reaching back into the run context. `result_message_id()`
reads it early if you need it.

`result_json` serializes for you; `result` takes a `String` you already have.

## A call the client executes

Front-end tools — the ones the client offered because the client is what can run them — are
closed with `end()` and nothing else. There is no result to report from here; it arrives as
a tool message on the next request:

```rust
use ag_ui::{Event, EventType, RunAgentInput};
use ag_ui::server::RunContext;
use serde_json::json;

fn main() -> ag_ui::server::Result<()> {
    let (mut ctx, mut events) = RunContext::<()>::new(RunAgentInput::new("t", "r"))?;

    let mut call = ctx.tool_call("open_settings_panel")?;
    call.args_json(&json!({"tab": "billing"}))?;
    call.end()?;

    let types: Vec<EventType> = events.drain().iter().map(Event::event_type).collect();
    assert_eq!(
        types,
        [
            EventType::ToolCallStart,
            EventType::ToolCallArgs,
            EventType::ToolCallEnd,
        ]
    );
    Ok(())
}
```

## Arguments stream as text

`args` takes a fragment, not a value, because that is how providers emit them: a partial
delta is usually not valid JSON, and the protocol keeps `TOOL_CALL_ARGS` unparsed for
exactly that reason. The handle keeps everything it emitted, so `parse_args` can hand you
the finished struct to execute against once the provider is done:

```rust
use ag_ui::RunAgentInput;
use ag_ui::server::RunContext;
use serde::Deserialize;

#[derive(Deserialize)]
struct Query {
    city: String,
}

fn main() -> ag_ui::server::Result<()> {
    let (mut ctx, _events) = RunContext::<()>::new(RunAgentInput::new("t", "r"))?;

    let mut call = ctx.tool_call("get_weather")?;
    call.args(r#"{"city":"#)?;       // as the provider streams them
    call.args(r#""Seoul"}"#)?;

    assert_eq!(call.raw_args(), r#"{"city":"Seoul"}"#);
    let query: Query = call.parse_args()?;
    assert_eq!(query.city, "Seoul");

    call.result(r#"{"tempC":21}"#)?;
    Ok(())
}
```

`parse_args` fails while the arguments are still partial, which is the point — call it once
the stream has finished, not per fragment. `raw_args` is the same buffer unparsed.

## The offered tool list is a capability list

`RunAgentInput.tools` says what the **client** can execute. It does not say what the agent
may call, and nothing in this SDK treats it as an allow-list: emitting `TOOL_CALL_START` for
a name absent from that list is a well-formed stream, and the
[ordering verifier](/ag-ui-rust/server/errors/) says nothing about it.

The case that settles it is a tool the agent answers itself. An A2UI agent emits
`render_a2ui` to carry a surface to the frontend — the frontend draws it, and no client ever
"offered" it because there is nothing for a client to execute. The same shape covers a
server-side tool whose result the agent computes within the run, and a call emitted purely
so the transcript shows what the agent did.

What a client does with a call it does not recognise is the client's decision: ignore it,
render it as an activity, or report it. What the protocol constrains is the *ordering* —
args with no start, a result before the end — and that is what gets checked.

An agent that wants the stricter rule can have it, because `ctx.tool(name)` returns `None`
for anything unoffered:

```rust
use ag_ui::{RunAgentInput, RunOutcome, Tool};
use ag_ui::server::{Agent, Error, Result, RunContext, ToolCallHandle};
use serde_json::json;

/// Opens a call only when the client offered the tool. A rule this agent
/// adopts for the tools it expects the client to run — not one the protocol
/// imposes.
fn offered<'a>(ctx: &'a mut RunContext<()>, name: &str) -> Result<ToolCallHandle<'a, ()>> {
    if ctx.tool(name).is_none() {
        return Err(Error::agent(format!("the client offered no {name} tool")));
    }
    ctx.tool_call(name)
}

struct Board;

impl Agent for Board {
    type State = ();

    async fn run(&self, ctx: &mut RunContext<()>) -> Result<RunOutcome> {
        let mut call = offered(ctx, "add_task")?;
        call.args_json(&json!({"title": "ship it"}))?;
        call.result_json(&json!({"ok": true}))?;

        Ok(RunOutcome::Success)
    }
}

fn main() -> ag_ui::server::Result<()> {
    let mut input = RunAgentInput::new("t", "r");
    input.tools = vec![Tool::new("add_task", "Add a task to the board.", json!({}))];
    let (mut ctx, _events) = RunContext::<()>::new(input)?;

    assert!(offered(&mut ctx, "add_task").is_ok());
    assert!(offered(&mut ctx, "delete_everything").is_err());
    Ok(())
}
```

`examples/task-board` does exactly this for the four tools that move its board on the
client's behalf, and does *not* do it for `render_a2ui`.

## Doing the work while the call is open

A handle borrows the run's event sink and its state, not the run context, so a tool's own
work belongs *between* the arguments and the result:

```rust
use ag_ui::RunAgentInput;
use ag_ui::server::RunContext;
use serde::{Deserialize, Serialize};
use serde_json::json;

#[derive(Default, Serialize, Deserialize)]
struct Board {
    tasks: Vec<String>,
}

fn main() -> ag_ui::server::Result<()> {
    let (mut ctx, mut events) = RunContext::<Board>::new(RunAgentInput::new("t", "r"))?;

    let mut call = ctx.tool_call("add_task")?;
    call.args_json(&json!({"title": "ship it"}))?;

    call.state_mut().tasks.push("ship it".to_owned());
    call.publish_state()?;               // STATE_SNAPSHOT, with the call open

    call.result_json(&json!({"ok": true}))?;

    assert_eq!(ctx.state().tasks, ["ship it"]);
    // START, ARGS, STATE_SNAPSHOT, END, RESULT.
    assert_eq!(events.drain().len(), 5);
    Ok(())
}
```

That order — the call in flight, the state changing, the result closing it — is the reason
to stream a call at all rather than announce it once it is already done.
[Shared state](/ag-ui-rust/server/state/) covers why the protocol allows it and why the
ordering is worth caring about.

## Parallel calls

Two open `ToolCallHandle`s at once is a borrow-check error, by design and for the same
reason two messages are. A provider that streams `args(a) args(b) args(a) end(a) end(b)`
therefore cannot be mirrored handle-for-call.

The mapping that works is to accumulate each call and emit it whole once its arguments are
complete. It is also the only one that cannot splice two calls' arguments into each other:

```rust
use ag_ui::{Event, EventType, RunAgentInput};
use ag_ui::server::RunContext;
use std::collections::BTreeMap;

fn main() -> ag_ui::server::Result<()> {
    let (mut ctx, mut events) = RunContext::<()>::new(RunAgentInput::new("t", "r"))?;

    let names = ["get_weather", "roll_dice"];
    // What the provider streamed: two calls, interleaved.
    let streamed = [
        (0, r#"{"city":"#),
        (1, r#"{"sides":"#),
        (0, r#""Seoul"}"#),
        (1, "20}"),
    ];

    let mut buffered: BTreeMap<usize, String> = BTreeMap::new();
    for (call, fragment) in streamed {
        buffered.entry(call).or_default().push_str(fragment);
    }

    for (call, args) in buffered {
        let mut handle = ctx.tool_call(names[call])?;
        handle.args(&args)?;
        handle.end()?;
    }

    let types: Vec<EventType> = events.drain().iter().map(Event::event_type).collect();
    assert_eq!(types.len(), 6);
    assert_eq!(types[0], EventType::ToolCallStart);
    Ok(())
}
```

`e2e/src/llm.rs` maps a real provider's stream this way. If you genuinely need the
interleaving on the wire — because a client is rendering both calls as they arrive — emit it
yourself with `ctx.emit`: the verifier keys everything by id, so it accepts an interleaved
stream. What it will not let you do is close a call you never opened.

## API

- [`RunContext::tool_call`](/ag-ui-rust/api/ag_ui/server/struct.RunContext.html#method.tool_call)
  and [`tool_call_with_id`](/ag-ui-rust/api/ag_ui/server/struct.RunContext.html#method.tool_call_with_id)
- [`RunContext::tools`](/ag-ui-rust/api/ag_ui/server/struct.RunContext.html#method.tools) and
  [`tool`](/ag-ui-rust/api/ag_ui/server/struct.RunContext.html#method.tool)
- [`ag_ui::server::ToolCallHandle`](/ag-ui-rust/api/ag_ui/server/struct.ToolCallHandle.html)
- [`ag_ui::Tool`](/ag-ui-rust/api/ag_ui/struct.Tool.html) and
  [`ToolCall`](/ag-ui-rust/api/ag_ui/struct.ToolCall.html)
