---
title: Serving over HTTP
description: Mounting an agent on an axum router, and the request and response the resulting endpoint speaks.
---

`ag_ui::serve` turns an [`Agent`](/ag-ui-rust/server/agent/) into a stream of events and
stops there, on purpose: it has no executor and no web framework, so it builds for wasm.
`ag_ui::axum` is the other half — the POST endpoint, the `text/event-stream` body, content
negotiation, and telling the agent when the client hangs up. It is the only crate in this
workspace that depends on tokio, axum or tower.

## Mounting an agent

```rust,no_run
// src/main.rs
use ag_ui::axum::RouterExt;
use ag_ui::RunOutcome;
use ag_ui::serve::{Agent, Result, RunContext};
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

#[tokio::main]
async fn main() {
    let app: Router = Router::new()
        .route("/health", get(|| async { "ok" }))
        .route_agui("/agent", Greeter);

    let listener = tokio::net::TcpListener::bind("0.0.0.0:3000").await.unwrap();
    axum::serve(listener, app).await.unwrap();
}
```

`route_agui(path, agent)` is `route(path, post(handler))` and nothing else. The endpoint
composes with the router's other routes, with `nest`, `merge` and `fallback`, and with any
layer applied before or after it. A `GET` on the path still gets axum's own `405`.

### The router state

The AG-UI handler reads only the request, so it is a `Handler<_, S>` for **every** router
state `S`. Mounting an agent therefore places no constraint on `S` beyond axum's own
`Clone + Send + Sync + 'static`, and works the same in a `Router<()>` and in a
`Router<AppState>` — including before `with_state` is called.

An agent that needs values from the application state should capture them when it is
constructed:

```rust
use ag_ui::axum::RouterExt;
use ag_ui::RunOutcome;
use ag_ui::serve::{Agent, Result, RunContext};
use axum::Router;
use std::sync::Arc;

#[derive(Clone)]
struct Catalog;

#[derive(Clone)]
struct AppState {
    catalog: Arc<Catalog>,
}

struct CartAgent {
    catalog: Arc<Catalog>,
}

impl Agent for CartAgent {
    type State = ();

    async fn run(&self, ctx: &mut RunContext<()>) -> Result<RunOutcome> {
        let _ = &self.catalog;
        ctx.say("Your cart is empty.")?;
        Ok(RunOutcome::Success)
    }
}

fn build(state: AppState) -> Router {
    let agent = CartAgent {
        catalog: Arc::clone(&state.catalog),
    };

    Router::new()
        .route_agui("/agent", agent)
        .with_state(state)
}

fn main() {
    let _ = build(AppState { catalog: Arc::new(Catalog) });
}
```

Extracting `State` inside the AG-UI handler would tie the one-line mount to a single
application's state type, which is the opposite of what it is for.

## What the endpoint answers with

| Situation | Answer |
| --- | --- |
| A run, however it ends | `200`, `text/event-stream`, terminated by `RUN_FINISHED` or `RUN_ERROR` |
| Body is not AG-UI JSON | `400` and a JSON message naming the field |
| `Content-Type` is not JSON | `415` |
| Body over the body limit | `413` |
| `Accept` excludes everything this build emits | `406` |
| Any method but `POST` | `405`, from axum |

A refusal answers with a JSON object rather than a bare status line, because the caller is a
program:

```json
{"code": "INVALID_INPUT", "message": "missing field `messages` at line 1 column 34"}
```

A run that *fails* is still a `200`. By the time an agent can fail, the status line is long
sent, so the failure is a `RUN_ERROR` event in a well-formed stream rather than a connection
that drops — which is exactly what lets a client tell "the agent errored" from "the network
died". The one case with no good answer is a panicking agent: it unwinds through hyper's
connection task and the client sees a truncated stream.

Each event is one SSE frame carrying the event's JSON:

```rust
use ag_ui::{Event, SseFormatter};

fn main() {
    let formatter = SseFormatter::new();
    let frame = formatter
        .encode_to_string(&Event::text_message_content("run-1-msg-1", "Hello!"))
        .expect("an event always serializes");

    assert_eq!(
        frame,
        "data: {\"type\":\"TEXT_MESSAGE_CONTENT\",\"messageId\":\"run-1-msg-1\",\"delta\":\"Hello!\"}\n\n"
    );
}
```

The response also carries `cache-control: no-cache, no-store, no-transform`,
`x-accel-buffering: no` and `vary: accept`. The `no-transform` is the half that matters: a
proxy that gzips this stream will also buffer it, and the point of the stream is that it
arrives a token at a time. The nginx header is its opt-out from the same behaviour, and is
inert everywhere else.

## Content negotiation

`negotiate` decides what to answer with, and refuses when the answer is "nothing" — a client
that asked for `application/xml` gets a `406`, not an SSE stream it cannot read. A missing or
empty `Accept` means `*/*`:

```rust
use ag_ui::axum::negotiate;

fn main() {
    assert!(negotiate(None).is_ok());
    assert!(negotiate(Some("")).is_ok());
    assert!(negotiate(Some("text/event-stream")).is_ok());
    assert!(negotiate(Some("text/*;q=0.4, application/json")).is_ok());

    assert!(negotiate(Some("application/xml")).is_err());
    assert!(negotiate(Some("*/*;q=0")).is_err());
}
```

## Changing the defaults

`AgentEndpoint` is the agent plus its per-run settings; `route_agui_with` mounts one.

```rust
use ag_ui::axum::{AgentEndpoint, RouterExt};
use ag_ui::RunOutcome;
use ag_ui::serve::{Agent, FilterToolCalls, Result, RunContext};
use axum::Router;
use std::time::Duration;

struct CartAgent;

impl Agent for CartAgent {
    type State = ();

    async fn run(&self, _ctx: &mut RunContext<()>) -> Result<RunOutcome> {
        Ok(RunOutcome::Success)
    }
}

fn main() {
    let endpoint = AgentEndpoint::new(CartAgent)
        .transformer(|| FilterToolCalls::deny(["internal_debug"]))
        .keep_alive(Duration::from_secs(15))
        .echo_input(false);

    let app: Router = Router::new().route_agui_with("/agent", endpoint);
    let _ = app;
}
```

- **`transformer`** takes a *closure*, not a transformer. Any useful `StreamTransformer` is a
  small state machine — `FilterToolCalls` remembers which call ids it dropped — so one
  instance shared across concurrent runs would leak one run's state into another. The
  endpoint stores the recipe and builds a fresh chain per request.
- **`keep_alive`** sends an SSE comment whenever a run produces nothing for the interval. Off
  by default; turn it on when something between the agent and the browser closes idle
  connections. Most reverse proxies do, at 30 to 60 seconds, which is well inside the time a
  slow first token can take.
- **`echo_input`** echoes the request back on `RUN_STARTED`, so a recorded stream replays
  without the original HTTP body. Off by default — it is the largest payload in the protocol.

## Cancellation on disconnect

The response body owns the run: polling the stream *is* running the agent, so the body and
the run have exactly the same lifetime. When the client goes away, hyper drops the body and
the run goes with it. That much is automatic.

What is not automatic is telling everything the run reached *outside* itself — a spawned tool
call, an in-flight model request. So the body also holds a guard that trips the run's
`CancellationToken` on drop, and disarms itself if the run got to finish, so a completed run
is never reported as cancelled. The agent side of this is on
[Errors and cancellation](/ag-ui-rust/server/errors/).

## Reading the request yourself

`AgUiInput` is a plain axum extractor, so a hand-written handler that needs to look at the
request first — auth, tenant routing, a path segment naming which agent to run — parses the
body exactly the same way the mounted endpoint does:

```rust
use ag_ui::axum::AgUiInput;
use axum::Router;
use axum::extract::Path;
use axum::routing::post;

async fn handler(Path(agent): Path<String>, AgUiInput(input): AgUiInput) -> String {
    format!("{agent} runs thread {}", input.thread_id)
}

fn main() {
    let app: Router = Router::new().route("/agents/{agent}", post(handler));
    let _ = app;
}
```

Its rejection type is `ag_ui::axum::Error`, which implements `IntoResponse`. Take
`AgUiInput` and axum answers the `4xx` for you; take `Result<AgUiInput, Error>` and inspect
the failure first.

`SseResponse` is the other half, for a handler that does its own work before starting the
run. `route_agui` is this, with the defaults filled in:

```rust
use ag_ui::axum::SseResponse;
use ag_ui::{RunAgentInput, RunOutcome};
use ag_ui::serve::{Agent, Result, RunContext, Runner};

struct Greeter;

impl Agent for Greeter {
    type State = ();

    async fn run(&self, ctx: &mut RunContext<()>) -> Result<RunOutcome> {
        ctx.say("hi")?;
        Ok(RunOutcome::Success)
    }
}

fn serve(
    accept: Option<&str>,
    input: RunAgentInput,
) -> axum::response::Result<axum::response::Response> {
    let response = SseResponse::negotiate(accept)?;

    let runner = Runner::new(Greeter);
    // Take the token *before* `run` consumes the runner.
    let response = response.cancellation(runner.cancellation_token());

    Ok(response.stream(runner.run(input)))
}

fn main() {
    let response = serve(Some("text/event-stream"), RunAgentInput::new("t", "r"))
        .expect("SSE is acceptable");
    assert_eq!(response.headers()["content-type"], "text/event-stream");

    assert!(serve(Some("application/xml"), RunAgentInput::new("t", "r")).is_err());
}
```

## Why there is no `AgUiLayer`

A tower layer wraps a `Service`, so it sees a `Request` and a `Response` — and by then the
events have already been serialized into an SSE body. Applying a `StreamTransformer` there
would mean parsing the frames back into events, transforming, and re-encoding: slower, lossy
at the edges, and it would silently mangle the body of any *other* route the layer happened
to cover. `AgentEndpoint::transformer` applies transformers where the events are still
typed, one chain per run.

The layers you actually want — CORS, auth, timeouts, tracing, compression — are the ones
tower already ships, and they compose with this endpoint like any other route.

## API

- [`ag_ui::axum::RouterExt`](/ag-ui-rust/api/ag_ui/axum/trait.RouterExt.html) and
  [`AgentEndpoint`](/ag-ui-rust/api/ag_ui/axum/struct.AgentEndpoint.html)
- [`ag_ui::axum::AgUiInput`](/ag-ui-rust/api/ag_ui/axum/struct.AgUiInput.html)
- [`ag_ui::axum::SseResponse`](/ag-ui-rust/api/ag_ui/axum/struct.SseResponse.html) and
  [`negotiate`](/ag-ui-rust/api/ag_ui/axum/fn.negotiate.html)
- [`ag_ui::axum::Error`](/ag-ui-rust/api/ag_ui/axum/enum.Error.html)
- [`ag_ui::serve::Runner`](/ag-ui-rust/api/ag_ui/serve/struct.Runner.html), for a transport
  of your own
- The other end of the wire: [Transports](/ag-ui-rust/client/transports/)
