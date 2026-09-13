---
title: Getting started
description: Choose your server or client implementation path.
---

Connect an agent's output to a user interface with AG-UI. This SDK lets an
**agent application (server)** emit messages, tool calls and shared state, and an
**agent client** turn that event stream into a conversation and a view.

This is the implementation guide for `ag-ui-rust`, an independent Rust SDK.
See the [official AG-UI documentation](https://docs.ag-ui.com/introduction) for protocol concepts and specifications.

## What are you building?

| Your application | Start with | What you will build |
| --- | --- | --- |
| Agent application (server) | [Connect an agent to AG-UI](/ag-ui-rust/server/) | An `Agent` implementation and an HTTP streaming endpoint |
| Agent client | [Call an agent from Rust](/ag-ui-rust/client/) | A server connection, conversation history and streamed output |
| Both | Start the server, then connect the client | One conversation between two Rust programs |

## From a request to a view

1. The **client** sends user input and conversation history.
2. The server's **`Agent::run`** calls application logic or a model.
3. **`RunContext`** emits text, tool calls and state changes as AG-UI events.
4. The client's **`Thread`** applies events to messages and state, exposing each change as an `Update`.

Your application or agent framework implements model calls, tool execution, authentication,
storage and workflow resumption. The SDK supplies protocol types, event production,
transports and client state updates.

## SDK responsibilities

| Application or framework | AG-UI SDK |
| --- | --- |
| Model loop, executable tool registration and execution | Carry tool names, arguments and results as protocol data |
| Graph routing, parallel tasks and subagent execution | Report steps and child-agent output and status |
| Approval policy, storage and workflow resumption | Carry interrupts and responses; snapshot a local conversation |

`Tool` is data: a name, description and argument JSON Schema. It does not register a function.
See [Tool calls and results](/ag-ui-rust/server/tools/) for connecting tools that already exist.

## Prerequisites

Use Rust **1.85 or newer**. Enable the `axum` feature of `ag-ui` for the server,
or `http` for an HTTP client. Each build guide includes its dependencies and runnable code.

Choose dependencies in [Crates and features](/ag-ui-rust/start/crates/) and read
[How AG-UI works](/ag-ui-rust/start/protocol/) for the relationship between requests, events and runs.

## Try a working application

- [task-board](/ag-ui-rust/examples/task-board/): a server with state changes, tool calls and approvals.
- [board-watch](/ag-ui-rust/examples/board-watch/): a Rust CLI client for that server.
- [review-desk](/ag-ui-rust/examples/review-desk/): an example connected to a browser UI.
