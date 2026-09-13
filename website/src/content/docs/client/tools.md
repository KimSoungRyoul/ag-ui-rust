---
title: Client tools and results
description: Describe client capabilities and return application-executed results.
---

Executable client functions must already be registered in your application.
`Tool` describes those capabilities to the server. The following code neither registers nor executes a function.

## Advertise capabilities

```rust
use ag_ui::{RunAgentInput, Tool};
use serde_json::json;

let tool = Tool::new("open_settings_panel", "Open application settings", json!({
    "type": "object",
    "properties": { "tab": { "type": "string" } },
    "required": ["tab"],
    "additionalProperties": false
}));
let mut input = RunAgentInput::new("conversation-1", "run-1");
input.tools = vec![tool];
assert_eq!(input.tools[0].name, "open_settings_panel");
```

With the high-level API, use `thread.set_tools(vec![tool])` or `ThreadBuilder::tools`.
The server receives names, descriptions and argument schemas, not client function implementations.
Server-executed tools do not need to be offered by the client.

## Receive a call and send its result

1. Collect the name, ID and arguments from `Update::Message` in the `RunStream`. Wait for `ToolCallEnded` before parsing arguments.
2. Check that the call is intended for client execution and has no server-supplied result. Do not automatically execute display-only calls.
3. Resolve the application's registered function, validate arguments and authorization, then execute it.
4. Consume and drop the current run, then append a result message with the same tool call ID to the `Thread`.
5. With no pending interrupts, use `thread.run()` to send the next request. Otherwise follow the [approval response](/ag-ui-rust/client/interrupts/) flow.

```rust
use ag_ui::Message;
use ag_ui::client::{Result, Thread, transport::Transport};
use serde_json::Value;

fn record_result<T: Transport>(
    thread: &mut Thread<T>,
    message_id: &str,
    tool_call_id: &str,
    output: &Value,
) -> Result<()> {
    thread.push_message(Message::tool(
        message_id, tool_call_id, serde_json::to_string(output)?,
    ))
}
```

Supply a new unique `message_id` and the received `tool_call_id`.
This function only records an existing result. Function selection, execution, retry policy and success/error result formats belong to the application.
`Thread` supplies neither a tool executor nor an automatic model loop.

[Send calls from the server](/ag-ui-rust/server/tools/) · [Manage conversations](/ag-ui-rust/client/thread/)
