---
title: A2UI function calls
description: Route A2UI 1.0 function calls in both directions and track their replies.
---

A2UI 1.0 defines two pairs of messages. The application runs its handlers and
supplies the clock. The SDK routes messages and tracks outstanding calls.

| Direction of call | Request | Reply |
| --- | --- | --- |
| Renderer to agent | `callAgentFunction` | `agentFunctionResponse` |
| Agent to renderer | `callRendererFunction` | `rendererFunctionResponse` |

The AG-UI adapter rejects a surface batch that contains a function-call message.
For this workflow, use a transport that carries A2UI messages directly, such
as the [A2A data part](/ag-ui-rust/v1.0.0/a2ui/versions/).

## Answer a renderer call

Declare an agent function with `CatalogDocument::with_agent_functions` on
the agent's copy of a v1.0 catalog. The declaration goes into the prompt and
lets the router recognize the call. Obtain a router from
`A2uiRequestProcessor::renderer_messages()`, or construct one from the active
catalogs. With the `schema-validation` feature, `checking_schemas()` validates
arguments and fills defaults before the application sees the call.

```rust
use ag_ui_a2ui::{AgentMessage, A2uiVersion};
use ag_ui_a2ui::agent::{AgentFunction, CatalogDocument, RendererMessageRouter, Routed};
use serde_json::json;

let catalog = CatalogDocument::basic(A2uiVersion::V1_0)?.with_agent_functions([
    AgentFunction::new("suggestTitle", json!({"type": "object"})).returning("string"),
])?;
let router = RendererMessageRouter::new(&[catalog.clone()])?;
let message = json!({"version": "1.0", "callAgentFunction": {
    "surfaceId": "planner",
    "functionCallId": "title-1",
    "callFunction": {"@call": "suggestTitle", "catalogId": catalog.catalog_id(), "args": {}}
}});
let reply: Option<AgentMessage> = match router.route(&message)? {
    Routed::Call(call) => Some(call.value(json!("Weeknight meals"))),
    Routed::Reply(reply) => Some(reply),
    Routed::FunctionResponse(_) | Routed::Action(_) | Routed::Error(_) => None,
    _ => None,
};
assert_eq!(serde_json::to_value(reply)?["agentFunctionResponse"]["functionCallId"], "title-1");
# Ok::<(), Box<dyn std::error::Error>>(())
```

Send each returned reply to the renderer. A `Routed::FunctionResponse`
belongs in the outbound call table. Send actions and renderer errors through
your normal UI event handling. The application decides whether it will run
each function and what reply to return.

## Call a renderer function

For a session, take `RendererCalls` from `A2uiRequestProcessor::renderer_calls()`.
A renderer function called by the agent needs a catalog ID, must allow
`agentOnly` or `rendererOrAgent`, and must not require user activation.
`RendererCalls::call` prepares a message and registers it as pending.

```rust
use std::time::Duration;
use ag_ui_a2ui::agent::{RendererCall, RendererCalls};
use ag_ui_a2ui::message::FunctionResponse;
use serde_json::json;

// The example renderer already has the device catalog and implements getLocale.
let mut calls = RendererCalls::new().with_id_prefix("planner-");
let call = RendererCall::new("example.com:device", "getLocale");
let outbound = calls.call(call, Duration::from_secs(0))?;
let json = serde_json::to_value(outbound)?;
let id = json["callRendererFunction"]["functionCallId"].as_str().unwrap();
let response = FunctionResponse::value(id, json!("ko"));
let settled = calls.resolve(&response).unwrap();
assert_eq!(settled.outcome.value(), Some(&json!("ko")));
# Ok::<(), Box<dyn std::error::Error>>(())
```

Keep one table per conversation, or give each table a distinct prefix with
`with_id_prefix`. If the application or model supplies an ID, keep it unique.
Before sending model-authored calls, pass each message to `track`. On a
response, pass it to `resolve`. A response with no match may belong to the
model's turn. The default limits are 30 seconds and 64 pending calls. Use
`next_deadline()` to schedule a wake-up, and pass the current time to
`expire(now)` when it arrives. The renderer owns evaluation of catalog functions
in bindings, checks, and actions.
