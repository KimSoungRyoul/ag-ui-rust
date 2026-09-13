---
title: Tool calls and results
description: Carry descriptions, calls and results for tools your application already implements.
---

Assume executable tools already exist in your application or agent framework.
The SDK does not choose or execute them. `ctx.tool_call(name)` opens protocol events for a call;
`args_json()` sends its arguments, and `result_json()` sends a result the application obtained.

## Tool descriptions and executable functions

`ag_ui::Tool` contains a name, description and argument JSON Schema. It contains no function or credentials.
Clients advertise their capabilities through `RunAgentInput.tools`. The server reads those descriptions with
`ctx.tools()` or `ctx.tool(name)` and can adapt them to its model's tool format.
The executing application owns function registration, argument validation, authorization and execution.

For an existing weather tool, the framework decides to call it and executes the weather API.
AG-UI carries the name, arguments and result to the UI. The weather result below is fixed example data.

## Send a server-executed result

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

The sequence is `TOOL_CALL_START` → `TOOL_CALL_ARGS` → `TOOL_CALL_END` → `TOOL_CALL_RESULT`.
`TOOL_CALL_END` ends argument emission; it does not report successful business execution.
`result_json` emits the end and result events and returns the result message ID.
`result()` accepts an already serialized string.

## Send a client-executed call

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

Finish argument emission with `end()`. The client application checks that it offered the tool,
executes it, and includes a result message with the same tool call ID in its next request.
The SDK does not invoke UI functions or external APIs. A call whose result the server already supplied
may be display-only; the client must not execute it again.

`RunAgentInput.tools` is neither proof of authorization nor an allow-list of all server tools.
A server may report calls and results for its own tools even when the client did not advertise them.
The `task-board` example checks some names against the offered list as application policy;
the example's application code also performs their execution.

## Stream argument fragments

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

`args()` accepts fragments of JSON text. Call `parse_args()` once all fragments have arrived.
Do not parse an individual fragment or execute a tool from incomplete arguments.

## Publish state while work is in progress

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

`state_mut()` and `publish_state()` expose progress; they do not perform database writes or tool execution.
A state event's position alone does not associate it with a particular call.

## Concurrent call output

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

A single context cannot hold two tool handles at once. This example buffers by call ID and emits calls sequentially.
For live interleaving, use `ctx.emit` with explicit IDs. Different call IDs may overlap.
The SDK does not schedule parallel tool execution.

## Next

- [Send tool results from a client](/ag-ui-rust/client/tools/)
- [Shared state](/ag-ui-rust/server/state/) and [AG-UI integration overview](/ag-ui-rust/server/)
- [`Tool`](/ag-ui-rust/api/ag_ui/tool/struct.Tool.html), [`ToolCallHandle`](/ag-ui-rust/api/ag_ui/server/emit/struct.ToolCallHandle.html)
