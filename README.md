# ag-ui-rust

A Rust SDK for [AG-UI](https://docs.ag-ui.com) and
[A2UI](https://a2ui.org): host an agent, connect to it, and generate declarative UI.
This is an independent project, not an official AG-UI SDK.

This source targets the `0.5.0-alpha.2` prerelease based on
[upstream PR #2778](https://github.com/ag-ui-protocol/ag-ui/pull/2778) at
`a459eace56b3aae72d0927bf1cdabda05de75c0f`. The previous stable release
is `0.4.5`. Choose the guide that matches your dependency:
[0.5.0-alpha.2](https://kimsoungryoul.github.io/ag-ui-rust/v0.5.0-alpha.2/start/)
or [0.4.5](https://kimsoungryoul.github.io/ag-ui-rust/v0.4.5/start/).
Both guides are also available in
[Korean](https://kimsoungryoul.github.io/ag-ui-rust/ko/versions/).

`0.5.0-alpha.2` replaces alpha.1 for new consumers. It constrains a transitive
derive dependency that otherwise breaks a fresh Rust 1.85 build of alpha.1.

## Packages

| Package / feature | Use it for |
|---|---|
| `ag-ui` | Protocol types and event encoding |
| `ag-ui/axum` | Hosting an agent over HTTP |
| `ag-ui/http` | `HttpAgent`, conversations in `Thread`, and streamed updates |
| `ag-ui/client` | The same conversation API over a custom transport |
| `ag-ui-a2ui/toolkit` | A2UI operations, parsing, bindings and semantic validation |
| `ag-ui-a2ui/author` | Schema-validated A2UI generation with an async model callback |
| `ag-ui-a2ui/ag-ui-server` | Sending generated A2UI from an AG-UI server |

The existing `ag-ui-core` and `ag-ui-client` community crates on crates.io
are separate packages.

Install both prerelease packages from crates.io at the same exact version:

```toml
[dependencies]
ag-ui = { version = "=0.5.0-alpha.2", features = ["http"] }
ag-ui-a2ui = "=0.5.0-alpha.2"
```

For a server, enable `axum` instead of `http`:

```toml
[dependencies]
ag-ui = { version = "=0.5.0-alpha.2", features = ["axum"] }
axum = "0.8"
tokio = { version = "1", features = ["macros", "rt-multi-thread", "net"] }
```

For a client:

```toml
[dependencies]
ag-ui = { version = "=0.5.0-alpha.2", features = ["http"] }
futures-util = "0.3"
tokio = { version = "1", features = ["macros", "rt-multi-thread"] }
```

For local development against a source checkout, replace each registry
dependency with a path to its crate. For example:

```toml
[dependencies]
ag-ui = { path = "/absolute/path/to/ag-ui-rust/crates/ag-ui", features = ["http"] }
ag-ui-a2ui = { path = "/absolute/path/to/ag-ui-rust/crates/ag-ui-a2ui" }
```

## Host an agent

```rust,no_run
use ag_ui::axum::RouterExt;
use ag_ui::server::{Agent, Result, RunContext};
use ag_ui::RunOutcome;

struct Greeter;

impl Agent for Greeter {
    type State = ();

    async fn run(&self, ctx: &mut RunContext<()>) -> Result<RunOutcome> {
        ctx.say("Hello from Rust.")?;
        Ok(RunOutcome::Success)
    }
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let app: axum::Router = axum::Router::new().route_agui("/agent", Greeter);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:3000").await?;
    axum::serve(listener, app).await?;
    Ok(())
}
```

`ctx.assistant_message()` streams individual text deltas. State publishing chooses
a snapshot or JSON Patch. Subagent event scopes attach output to an invocation;
the application or agent framework owns the actual execution. Explicitly finish,
fail or suspend a subagent scope: dropping it never reports success.

## Talk to an agent

```rust,no_run
use ag_ui::client::{HttpAgent, Update};
use futures_util::StreamExt;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let agent = HttpAgent::new("http://localhost:3000/agent")?;
    let mut thread = agent.thread("conversation-1");

    {
        let mut run = thread.send("hello")?;
        while let Some(update) = run.next().await {
            match update {
                Update::Message(message) => println!("{:?}", message.change),
                Update::Error(error) => eprintln!("{error}"),
                Update::Done(end) => println!("{end:?}"),
                _ => {}
            }
        }
    }

    let report = thread.send("and next?")?.collect_report().await;
    println!("{:?}; diagnostics: {:?}", report.end, report.diagnostics);
    println!("{} messages", thread.messages().len());
    Ok(())
}
```

An `HttpAgent` holds connection settings. Each `Thread` owns its conversation,
state and pending approvals. Each send starts a run. Creating a thread is local;
loading server history and storing thread snapshots belong to the application.
`Transport` remains available for replay tests and custom connections.

## Examples and migration

See [examples/](examples/README.md) for runnable applications and their regression
commands. Each project consumes the public SDK rather than copying its reducers.

The [0.5 migration notes](docs/migration-0.5.md) cover AG-UI 1.0
wire behavior and API changes from 0.4.5. Version 0.4 changed the conversation
API and A2UI data-update semantics; see [that migration guide](docs/migration-0.4.md) and
[the reviewed design](docs/sdk-api-improvement-proposal.ko.md).

The [0.5 QA record](docs/QA.md#05-alpha2-msrv-verification-2026-09-27) records
the independent consumers, QWENCLOUD run, and release gates. Check
[crates.io](https://crates.io/crates/ag-ui) for registry availability.

The [documentation site](https://kimsoungryoul.github.io/ag-ui-rust/) includes
English and Korean guides. Rust examples in the README, site and repository
skills are compiled as doctests.

## Development

```sh
cargo nextest run --workspace --all-features
cargo test --doc --workspace --all-features
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo run --locked -p xtask -- drift-check
cargo run --locked -p xtask -- drift-check --local
prek run --all-files
```

The client and server runtimes use executor-neutral futures. HTTP and axum are
optional. CI checks Rust 1.85, wasm feature builds, dependency boundaries, docs,
and the vendored AG-UI 1.0 schema and event baseline.

## License

MIT. Vendored A2UI schemas and conformance fixtures retain their Apache-2.0 license.

Dependency security is checked with `cargo audit --deny warnings` on every CI run,
daily against current RustSec advisories, and before release and registry publishing.
Vulnerabilities and warnings (including yanked crates) fail the check.

## Protocol boundary and interoperability

[Protocol and application boundaries](docs/protocol-boundary.md) describes the
standalone event verifier, consumer-owned JSON ordering and compatibility
corrections. The [official TypeScript comparison](e2e/interop/README.md) checks
schema semantics separately from the event/field drift gate.
