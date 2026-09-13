---
title: Platforms and MSRV
description: Native, wasm and executor support boundaries.
---

The minimum Rust version is **1.85**, with edition **2024**.
CI's `msrv` job checks all workspace features and targets on that version.

## Supported configurations

| Configuration | Environment | Verification |
| --- | --- | --- |
| Protocol types, `server`, `client` | Native, custom executor | Unit, integration and doctests |
| `axum` | Native Tokio HTTP server | Real HTTP tests |
| `http` | Native reqwest HTTP client | Real HTTP tests |
| Protocol, `server`, `client`, A2UI | `wasm32-unknown-unknown` | Compilation and dependency checks for selected features |

A wasm compile does not prove browser execution of the Rust SDK.
No browser transport is shipped; supply one yourself. Review Desk's TypeScript renderer checks are separate.

## Separate execution from transport

The normal dependency graphs of `server` and `client` alone contain no Tokio.
Enabling `axum` or `http` brings Tokio in. HTTP is opt-in.

```rust
use ag_ui::client::transport::{Transport, TransportFuture, boxed_stream};
use ag_ui::client::Result;
use ag_ui::{Event, RunAgentInput};
use futures_util::stream;

/// Replays a fixed script — the shape a browser transport built on `fetch`
/// and `EventSource` would also take.
struct Canned(Vec<Event>);

impl Transport for Canned {
    // Failing to connect is an error from the future; failing mid-stream is an
    // error item in the stream. That split is the whole interface.
    fn run(&self, _input: RunAgentInput) -> TransportFuture {
        let events: Vec<Result<Event>> = self.0.iter().cloned().map(Ok).collect();
        Box::pin(async move { Ok(boxed_stream(stream::iter(events))) })
    }
}
```

This is a replay transport example. In a browser, replace it with an implementation that
sends POST requests and reads streaming responses. `EventStream` and `TransportFuture`
require `Send` on native targets and omit that bound on wasm.

## Check locally

```sh
cargo check -p ag-ui --target wasm32-unknown-unknown --no-default-features --features client,sse
cargo check -p ag-ui-a2ui --target wasm32-unknown-unknown --all-features
cargo tree -p ag-ui --no-default-features --features server -e normal
cargo tree -p ag-ui --no-default-features --features client -e normal
```

Install the target with `rustup target add wasm32-unknown-unknown` first.
In addition to compilation, CI asserts Tokio is absent from the relevant normal dependency graphs.
That guarantee excludes dev-dependencies and HTTP configurations.
See [Feature flags](/ag-ui-rust/reference/features/) for the complete configurations.
