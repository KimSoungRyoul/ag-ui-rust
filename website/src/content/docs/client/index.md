---
title: Call an agent from Rust
description: Build a Rust client with HTTP, conversation history and streamed updates.
---

Build a Rust client that connects to an AG-UI server, prints streamed output and retains conversation history.
Start the server from [Build an agent server](/ag-ui-rust/server/) first, or replace the URL with an existing AG-UI endpoint.
Requires Rust 1.85 or newer and a C toolchain for the HTTP client's TLS dependencies.

## 1. Create the project

```sh
cargo new agent-client
cd agent-client
```

```toml
# Cargo.toml
[dependencies]
ag-ui = { git = "https://github.com/KimSoungRyoul/ag-ui-rust", features = ["http"] }
futures-util = "0.3"
tokio = { version = "1", features = ["rt-multi-thread", "macros"] }
```

## 2. Connect and consume updates

Replace `src/main.rs` with this code. `HttpAgent` owns connection settings,
`Thread` owns conversation history and state, and `Update` describes changes for the view.

```rust,no_run
use ag_ui::client::{HttpAgent, MessageChangeKind, RunEnd, Update};
use futures_util::StreamExt;
use std::io::Write;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let agent = HttpAgent::new("http://127.0.0.1:3000/agent")?;
    let mut thread = agent.thread("thread-1");
    {
        let mut run = thread.send("Hello")?;
        while let Some(update) = run.next().await {
            match update {
                Update::Message(message) => {
                    if let MessageChangeKind::Content { delta } = message.change {
                        print!("{delta}");
                        std::io::stdout().flush()?;
                    }
                }
                Update::Error(error) => eprintln!("{error}"),
                Update::Done(RunEnd::Success { .. }) => println!(),
                Update::Done(end) => eprintln!("{end:?}"),
                _ => {}
            }
        }
    }
    println!("{} messages in the thread", thread.messages().len());
    Ok(())
}
```

## 3. Check the result

With the server running, use `cargo run`.
Expect `Hello from Rust.` and `2 messages in the thread`.
Call `send()` on the same `Thread` again to include the conversation in the next request.

## Component guides

| What you need | Component | Guide |
| --- | --- | --- |
| Connect and retain a conversation | `HttpAgent`, `Thread` | [Connect and manage threads](/ag-ui-rust/client/thread/) |
| Handle changes, diagnostics and completion | `RunStream`, `Update`, `RunEnd` | [The update stream](/ag-ui-rust/client/updates/) |
| Display messages, tools and child agents | Message ID, tool call ID, subagent registry | [Rendering](/ag-ui-rust/client/rendering/) |
| Read shared state in the UI | `Update::State`, `state`, `raw_state` | [Read shared state](/ag-ui-rust/client/state/) |
| Approve, decline and resume | `resume_many`, `decline`, snapshot | [Approvals and recovery](/ag-ui-rust/client/interrupts/) |
| Replace HTTP or test locally | `Transport`, `ReplayTransport` | [Transports](/ag-ui-rust/client/transports/) |

Connect existing client functions using [Client tools and results](/ag-ui-rust/client/tools/).

## Client application responsibilities

The SDK assembles events into messages and state. Your application connects `Update` to UI components,
configures authentication and chooses where to store snapshots. Creating a new object with the same thread ID does not fetch server history.

This guide covers **clients written in Rust**. For a TypeScript browser client, use the
[official TypeScript SDK](https://docs.ag-ui.com/sdk/js/client/overview) and the
[review-desk example](/ag-ui-rust/examples/review-desk/).
See [board-watch](/ag-ui-rust/examples/board-watch/) for a Rust CLI implementation.
