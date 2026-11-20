# ag-ui-rust

A Rust SDK for [AG-UI](https://docs.ag-ui.com) and
[A2UI](https://a2ui.org): host an agent, connect to it, and generate declarative UI.
This independent project maintains the Rust SDK proposed for inclusion in the
AG-UI community SDKs in [PR #2778](https://github.com/ag-ui-protocol/ag-ui/pull/2778).

SDK **1.0.0** supports the [AG-UI 1.0 wire specification](https://github.com/ag-ui-protocol/ag-ui/tree/main/spec/1.0).
The crate versions follow Rust API SemVer. Compatible bug fixes bump the patch
version, additive APIs bump the minor version, and breaking API changes bump the
major version. Wire protocol support is documented separately.

Start with the [English guide](https://kimsoungryoul.github.io/ag-ui-rust/v1.0.0/start/)
or [한국어 가이드](https://kimsoungryoul.github.io/ag-ui-rust/ko/v1.0.0/start/).
For applications upgrading from 0.5.0-alpha.3, see the [migration notes](docs/migration-0.5.md#changes-in-100).

## Packages

| Package / feature | Use it for |
|---|---|
| `ag-ui` | Protocol types and event encoding |
| `ag-ui/axum` | Hosting an agent over HTTP |
| `ag-ui/http` | `HttpAgent`, conversations in `Thread`, and streamed updates |
| `ag-ui/client` | The same conversation API over a custom transport |
| `ag-ui-a2ui` with default features off | A2UI wire messages, catalogs, bindings and semantic validation |
| `ag-ui-a2ui/agent` (default) | Catalog negotiation, generation prompts, response parsing and retry decisions |
| `ag-ui-a2ui/ag-ui` | Optional AG-UI history and tool-definition adapter |
| `ag-ui-a2ui/author` | Schema-validated A2UI generation with an async model callback |
| `ag-ui-a2ui/ag-ui-server` | Surface painting from an AG-UI server |
| `ag-ui-a2ui/a2a` | A2A extensions, capabilities and data parts |

The existing `ag-ui-core` and `ag-ui-client` community crates on crates.io
are separate packages.

### Released packages

Install the packages from crates.io:

```toml
[dependencies]
ag-ui = { version = "1.0.0", features = ["http"] }
ag-ui-a2ui = "1.0.0"
```

For a server, enable `axum` instead of `http`:

```toml
[dependencies]
ag-ui = { version = "1.0.0", features = ["axum"] }
axum = "0.8"
tokio = { version = "1", features = ["macros", "rt-multi-thread", "net"] }
```

For a client:

```toml
[dependencies]
ag-ui = { version = "1.0.0", features = ["http"] }
futures-util = "0.3"
tokio = { version = "1", features = ["macros", "rt-multi-thread"] }
```

### Development checkout

For local development, clone `ag-ui-rust` beside your application and use a
path dependency. Add `ag-ui-a2ui` only for an application that uses A2UI.

```toml
[dependencies]
ag-ui = { path = "../ag-ui-rust/crates/ag-ui", features = ["http"] }
ag-ui-a2ui = { path = "../ag-ui-rust/crates/ag-ui-a2ui" }
```

For the server example, enable `axum` on the local `ag-ui` dependency and add
`axum` and `tokio` as shown above. For the client, enable `http` and add
`futures-util` and `tokio`. The A2UI adapter and `author` are separate features.
See the [A2UI feature guide](https://kimsoungryoul.github.io/ag-ui-rust/v1.0.0/a2ui/)
for the combination that fits your transport and authoring flow.

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
                Update::RunEnded(run) => println!("{run:?}"),
                Update::Done(end) => println!("stream outcome: {end:?}"),
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
state and pending approvals. Each send requests a run. The response can contain replayed runs as well.
Creating a thread is local;
loading server history and storing thread snapshots belong to the application.
`Transport` remains available for replay tests and custom connections.

## Examples and migration

See [examples/](examples/README.md) for runnable applications and their regression
commands. Each project consumes the public SDK rather than copying its reducers.

The [migration notes](docs/migration-0.5.md) cover the move to alpha.3
and the changes through 1.0.0. Version 0.4 changed the conversation
API and A2UI data-update semantics; see [that migration guide](docs/migration-0.4.md).
[The 0.3 → 0.4 design record](docs/sdk-api-improvement-proposal.ko.md) is kept as
history, not as the current design.

The [0.5 QA record](docs/QA.md) records
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
cargo run --locked -p xtask -- docs-check
prek run --all-files
```

`prek install` once per clone turns the git hooks on. Work starts from an issue and
the branch carries its number, `<issue>-<type>-<slug>`; the `prepare-commit-msg` hook
then prefixes each commit with it, as in `[#20] fix: read and write the 1.0 key`.

The client and server runtimes use executor-neutral futures. HTTP and axum are
optional. CI checks Rust 1.85, wasm feature builds, dependency boundaries, docs,
the vendored AG-UI 1.0 schema and event baseline, and the upstream 1.0 conformance
corpus, run as table tests.

## License

MIT. Vendored A2UI schemas and conformance fixtures retain their Apache-2.0 license, and
the vendored AG-UI 1.0 conformance corpus its upstream MIT notice
(`crates/ag-ui/tests/fixtures/ag-ui-1.0/LICENSE`).

Dependency security is checked with `cargo audit --deny warnings` on every CI run,
daily against current RustSec advisories, and before release and registry publishing.
Vulnerabilities and warnings (including yanked crates) fail the check.

## Protocol boundary and interoperability

[Protocol and application boundaries](docs/protocol-boundary.md) describes the
standalone event verifier, consumer-owned JSON ordering and compatibility
corrections. The [official TypeScript comparison](e2e/interop/README.md) checks
schema semantics separately from the event/field drift gate.
