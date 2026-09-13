---
title: Connect an agent to AG-UI
description: Implement an agent, expose HTTP, and add server components.
---

This page connects an existing agent to AG-UI. Your application or framework owns the model loop,
executable tool registration, graph routing and subagent execution.
The `Greeter` below is a minimal example of the protocol connection.

Build a server that streams text at `http://127.0.0.1:3000/agent`.
Start with a small `Agent`, then add the components your application needs. Requires Rust 1.85 or newer.

## 1. Create the project

```sh
cargo new agent-server
cd agent-server
```

```toml
# Cargo.toml
[dependencies]
ag-ui = { git = "https://github.com/KimSoungRyoul/ag-ui-rust", features = ["axum"] }
axum = "0.8"
tokio = { version = "1", features = ["rt-multi-thread", "macros", "net"] }
```

## 2. Implement an agent and connect HTTP

Replace `src/main.rs` with the following code. Put application logic in `Agent`,
write client output through `RunContext`, and connect it to HTTP with `route_agui`.

```rust,no_run
// src/main.rs
use ag_ui::axum::RouterExt;
use ag_ui::RunOutcome;
use ag_ui::server::{Agent, Result, RunContext};
use axum::Router;

struct Greeter;

impl Agent for Greeter {
    type State = ();

    async fn run(&self, ctx: &mut RunContext<()>) -> Result<RunOutcome> {
        // Streams as TEXT_MESSAGE_START / _CONTENT / _END.
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
    println!("agent on http://127.0.0.1:3000/agent");
    axum::serve(listener, app).await.unwrap();
}
```

## 3. Run and check the response

Start the server with `cargo run`, then send a request from another terminal.

```sh
curl -N -X POST http://127.0.0.1:3000/agent \
  -H 'content-type: application/json' \
  -d '{"threadId":"thread-1","runId":"run-1","messages":[],"tools":[],"context":[]}'
```

Expect `RUN_STARTED` → `TEXT_MESSAGE_START` → `TEXT_MESSAGE_CONTENT` → `TEXT_MESSAGE_END`
→ `RUN_FINISHED`, with `Hello from Rust.` as the text content.
The SDK generates the run's opening and closing events.

## Component guides

| What you need | Component | Guide |
| --- | --- | --- |
| Connect a model or application logic | `Agent`, `RunContext`, `RunOutcome` | [Agent and run context](/ag-ui-rust/server/agent/) |
| Expose HTTP and handle requests | `RouterExt`, `AgentEndpoint`, `AgUiInput` | [HTTP endpoint](/ag-ui-rust/server/axum/) |
| Stream a response incrementally | Message handle, `delta`, `end` | [Streaming text](/ag-ui-rust/server/text/) |
| Describe tool calls and results | Tool call handle | [Tool calls](/ag-ui-rust/server/tools/) |
| Update state shared with the UI | `State`, snapshots, deltas | [Shared state](/ag-ui-rust/server/state/) |
| Wait for a decision and resume | `Interrupt`, resume input | [Human in the loop](/ag-ui-rust/server/interrupts/) |
| Attribute child agent output | Subagent handle, invocation ID | [Subagents](/ag-ui-rust/server/subagents/) |
| Handle failures and disconnects | Server errors, cancellation token | [Errors and cancellation](/ag-ui-rust/server/errors/) |

## Connect application logic

For example, a task-board server reads task data, emits state changes and returns an
interrupt when it needs approval. The application performs database writes and authorization.
Inject your services or model client into the `Agent` implementation and emit their output through `RunContext`.

Follow the [task-board example](/ag-ui-rust/examples/task-board/) for that flow.
Use [agent tests](/ag-ui-rust/design/testing/) to check it without HTTP.
Next, [connect a Rust client](/ag-ui-rust/client/) to display the response.
