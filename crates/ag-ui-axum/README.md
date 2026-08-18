# ag-ui-axum

Serve an [AG-UI](https://github.com/ag-ui-protocol/ag-ui) agent from an
[axum](https://crates.io/crates/axum) router.

[`ag-ui-server`](https://github.com/KimSoungRyoul/ag-ui-rust/tree/main/crates/ag-ui-server)
turns an `Agent` into a stream of events and stops there, on purpose: it has no executor
and no web framework, so it builds for wasm. This crate is the other half — the POST
endpoint, the `text/event-stream` body, content negotiation, and telling the agent when the
client hangs up. It is the only crate in the workspace that depends on tokio, axum or tower.

```toml
[dependencies]
ag-ui-axum = { git = "https://github.com/KimSoungRyoul/ag-ui-rust" }
ag-ui-server = { git = "https://github.com/KimSoungRyoul/ag-ui-rust" }
ag-ui-core = { git = "https://github.com/KimSoungRyoul/ag-ui-rust" }
axum = "0.8"
```

Not on crates.io — these crates are unpublished, and some of the `ag-ui-*` names there
belong to other projects, so depend on the repository rather than on a version number.

Mounting an agent is one line, and the router is still an ordinary router:

```rust
use ag_ui_axum::RouterExt;
use ag_ui_core::RunOutcome;
use ag_ui_server::{Agent, Result, RunContext};
use axum::Router;
use axum::routing::get;

struct Greeter;

impl Agent for Greeter {
    type State = ();

    async fn run(&self, ctx: &mut RunContext<()>) -> Result<RunOutcome> {
        ctx.say("Hello!")?;
        Ok(RunOutcome::Success)
    }
}

let app: Router = Router::new()
    .route("/health", get(|| async { "ok" }))
    .route_agui("/agent", Greeter);
```

Serving it is ordinary axum — `route_agui` returns the same `Router` you started with:

```rust,no_run
use ag_ui_axum::RouterExt;
use ag_ui_core::RunOutcome;
use ag_ui_server::{Agent, Result, RunContext};
use axum::Router;

struct Greeter;

impl Agent for Greeter {
    type State = ();

    async fn run(&self, ctx: &mut RunContext<()>) -> Result<RunOutcome> {
        ctx.say("Hello!")?;
        Ok(RunOutcome::Success)
    }
}

#[tokio::main]
async fn main() {
    let app: Router = Router::new().route_agui("/agent", Greeter);
    let listener = tokio::net::TcpListener::bind("0.0.0.0:3000").await.unwrap();
    axum::serve(listener, app).await.unwrap();
}
```

A `POST /agent` carrying a `RunAgentInput` is answered with a `text/event-stream` whose
frames are the run's events, starting at `RUN_STARTED` and ending at `RUN_FINISHED`.

See the [repository](https://github.com/KimSoungRyoul/ag-ui-rust) for the design rationale.

## License

MIT
