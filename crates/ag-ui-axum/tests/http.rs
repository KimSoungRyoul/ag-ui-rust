//! The endpoint over real HTTP.
//!
//! Every test here binds a port and speaks HTTP/1.1 down a socket, because the
//! two things most likely to be wrong are exactly the things a handler-level
//! test cannot see: the response headers hyper actually writes, and what
//! happens to the run when the socket goes away.
//!
//! The client is hand-rolled for the same reason. `read_chunk` hands back one
//! `Transfer-Encoding` chunk at a time, so a test can consume half a stream and
//! then pull the plug — which is the whole point of
//! [`dropping_the_client_cancels_the_run`].

use std::net::SocketAddr;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use ag_ui_axum::{AgentEndpoint, RouterExt};
use ag_ui_core::{Event, EventType, Interrupt, RunAgentInput, RunOutcome};
use ag_ui_server::{
    Agent, CancellationToken, Error as AgentError, FilterToolCalls, Result, RunContext,
};
use axum::Router;
use axum::routing::get;
use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::mpsc::{UnboundedSender, unbounded_channel};
use tokio::time::{sleep, timeout};

// ---------------------------------------------------------------- agents ----

/// Emits a step, a message and a tool call, then finishes.
struct Chatty;

impl Agent for Chatty {
    type State = ();

    async fn run(&self, ctx: &mut RunContext<()>) -> Result<RunOutcome> {
        let mut step = ctx.step("answer")?;
        let mut message = step.assistant_message()?;
        message.delta("Hello, ")?;
        message.delta("world.")?;
        message.end()?;

        let mut call = step.tool_call("internal_debug")?;
        call.args("{}")?;
        call.end()?;

        drop(step);
        Ok(RunOutcome::Success)
    }
}

/// Says something, then fails.
struct Broken;

impl Agent for Broken {
    type State = ();

    async fn run(&self, ctx: &mut RunContext<()>) -> Result<RunOutcome> {
        ctx.say("Looking that up…")?;
        Err(AgentError::agent("the weather service is down"))
    }
}

/// Pauses for human approval.
struct Cautious;

impl Agent for Cautious {
    type State = ();

    async fn run(&self, ctx: &mut RunContext<()>) -> Result<RunOutcome> {
        ctx.say("This will delete everything. Are you sure?")?;
        Ok(RunOutcome::interrupt(vec![Interrupt::new(
            "delete-everything",
            "tool_approval",
        )]))
    }
}

/// Streams forever, five milliseconds at a time.
///
/// Hands its cancellation token to the test on the way in, and counts what it
/// emitted — between them, a test can watch the run stop.
struct Endless {
    token: UnboundedSender<CancellationToken>,
    emitted: Arc<AtomicUsize>,
}

impl Agent for Endless {
    type State = ();

    async fn run(&self, ctx: &mut RunContext<()>) -> Result<RunOutcome> {
        let _ = self.token.send(ctx.cancel_token());

        let mut message = ctx.assistant_message()?;
        for index in 0..10_000 {
            message.delta(format!("token-{index} "))?;
            self.emitted.fetch_add(1, Ordering::SeqCst);
            sleep(Duration::from_millis(5)).await;
        }
        message.end()?;
        Ok(RunOutcome::Success)
    }
}

/// Finishes at once, after handing out its token.
struct Prompt {
    token: UnboundedSender<CancellationToken>,
}

impl Agent for Prompt {
    type State = ();

    async fn run(&self, ctx: &mut RunContext<()>) -> Result<RunOutcome> {
        let _ = self.token.send(ctx.cancel_token());
        ctx.say("done")?;
        Ok(RunOutcome::Success)
    }
}

/// Says nothing for `quiet`, so a keep-alive has something to fill.
struct Slow {
    quiet: Duration,
}

impl Agent for Slow {
    type State = ();

    async fn run(&self, ctx: &mut RunContext<()>) -> Result<RunOutcome> {
        sleep(self.quiet).await;
        ctx.say("finally")?;
        Ok(RunOutcome::Success)
    }
}

/// Emits once and then waits, like an agent blocked on a slow model call.
struct Waiting {
    token: UnboundedSender<CancellationToken>,
}

impl Agent for Waiting {
    type State = ();

    async fn run(&self, ctx: &mut RunContext<()>) -> Result<RunOutcome> {
        let _ = self.token.send(ctx.cancel_token());
        ctx.say("thinking…")?;
        sleep(Duration::from_secs(60)).await;
        Ok(RunOutcome::Success)
    }
}

// ----------------------------------------------------------------- tests ----

#[tokio::test(flavor = "multi_thread")]
async fn a_run_streams_its_events_in_order() {
    let addr = serve(Router::new().route_agui("/agent", Chatty)).await;
    let (head, body) = request(addr, &[], &input()).await;

    assert_eq!(head.status, 200);
    let types: Vec<EventType> = events(&body).iter().map(Event::event_type).collect();
    assert_eq!(
        types,
        [
            EventType::RunStarted,
            EventType::StepStarted,
            EventType::TextMessageStart,
            EventType::TextMessageContent,
            EventType::TextMessageContent,
            EventType::TextMessageEnd,
            EventType::ToolCallStart,
            EventType::ToolCallArgs,
            EventType::ToolCallEnd,
            EventType::StepFinished,
            EventType::RunFinished,
        ]
    );

    let Some(Event::RunFinished(finished)) = events(&body).pop() else {
        panic!("the stream should end with RUN_FINISHED: {body}");
    };
    assert_eq!(finished.thread_id.as_str(), "thread-1");
    assert_eq!(finished.run_id.as_str(), "run-1");
    assert_eq!(finished.outcome, Some(RunOutcome::Success));
}

#[tokio::test(flavor = "multi_thread")]
async fn the_response_is_framed_as_an_unbuffered_event_stream() {
    let addr = serve(Router::new().route_agui("/agent", Chatty)).await;
    let (head, _) = request(addr, &[], &input()).await;

    assert_eq!(head.header("content-type"), Some("text/event-stream"));
    assert_eq!(
        head.header("transfer-encoding"),
        Some("chunked"),
        "the body must stream, not arrive as one buffered response"
    );
    // Whatever is between the agent and the browser must not hold events back.
    let cache_control = head.header("cache-control").unwrap_or_default();
    assert!(cache_control.contains("no-cache"), "{cache_control}");
    assert!(cache_control.contains("no-transform"), "{cache_control}");
    assert_eq!(head.header("x-accel-buffering"), Some("no"));
    // The body was chosen by `Accept`.
    assert_eq!(head.header("vary"), Some("accept"));
}

#[tokio::test(flavor = "multi_thread")]
async fn events_arrive_before_the_run_ends() {
    let addr = serve(Router::new().route_agui(
        "/agent",
        Slow {
            quiet: Duration::from_secs(30),
        },
    ))
    .await;

    let mut client = Client::connect(addr).await;
    client.post("/agent", &[], &input()).await;
    assert_eq!(client.read_head().await.status, 200);

    // The agent will not finish for thirty seconds. RUN_STARTED still has to be
    // on the wire now, or the endpoint is buffering the whole run.
    let first = timeout(Duration::from_secs(5), client.read_chunk())
        .await
        .expect("RUN_STARTED should not wait for the run to finish")
        .expect("a chunk, not the terminator");
    let first = String::from_utf8(first).expect("utf-8");
    assert_eq!(
        events(&first).first().map(Event::event_type),
        Some(EventType::RunStarted)
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn a_failing_agent_still_ends_the_stream_with_run_error() {
    let addr = serve(Router::new().route_agui("/agent", Broken)).await;
    let (head, body) = request(addr, &[], &input()).await;

    // The status line went out before the agent could fail, so the failure is
    // in the stream — not a dropped connection and not a 500.
    assert_eq!(head.status, 200);
    let events = events(&body);
    let types: Vec<EventType> = events.iter().map(Event::event_type).collect();
    assert_eq!(
        types,
        [
            EventType::RunStarted,
            EventType::TextMessageStart,
            EventType::TextMessageContent,
            EventType::TextMessageEnd,
            EventType::RunError,
        ]
    );

    let Some(Event::RunError(error)) = events.last() else {
        panic!("the stream should end with RUN_ERROR: {body}");
    };
    assert_eq!(error.code.as_deref(), Some("AGENT_ERROR"));
    assert!(
        error.message.contains("the weather service is down"),
        "{error:?}"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn an_interrupt_outcome_rides_out_on_run_finished() {
    let addr = serve(Router::new().route_agui("/agent", Cautious)).await;
    let (head, body) = request(addr, &[], &input()).await;

    assert_eq!(head.status, 200);
    let Some(Event::RunFinished(finished)) = events(&body).pop() else {
        panic!("the stream should end with RUN_FINISHED: {body}");
    };

    let outcome = finished.outcome.expect("an interrupt outcome");
    assert!(outcome.is_interrupt(), "{outcome:?}");
    assert_eq!(outcome.interrupts().len(), 1);
    assert_eq!(outcome.interrupts()[0].id, "delete-everything");
    assert_eq!(outcome.interrupts()[0].reason, "tool_approval");
}

#[tokio::test(flavor = "multi_thread")]
async fn a_malformed_body_is_a_400_that_says_why() {
    let addr = serve(Router::new().route_agui("/agent", Chatty)).await;

    let (head, body) = request(addr, &[], br#"{"threadId": "#).await;
    assert_eq!(head.status, 400);
    assert_eq!(head.header("content-type"), Some("application/json"));
    let message = message_of(&body);
    assert!(message.contains("RunAgentInput"), "{message}");
    assert!(message.contains("line 1 column"), "{message}");

    // Valid JSON, wrong shape: the message names the field.
    let (head, body) = request(addr, &[], br#"{"threadId":"t","runId":"r"}"#).await;
    assert_eq!(head.status, 400);
    assert!(message_of(&body).contains("messages"), "{body}");

    // No body at all.
    let (head, body) = request(addr, &[], b"").await;
    assert_eq!(head.status, 400);
    assert!(message_of(&body).contains("empty"), "{body}");
}

#[tokio::test(flavor = "multi_thread")]
async fn a_body_that_is_not_json_is_a_415() {
    let addr = serve(Router::new().route_agui("/agent", Chatty)).await;
    let (head, body) = request(
        addr,
        &[("content-type", "application/x-www-form-urlencoded")],
        b"threadId=t&runId=r",
    )
    .await;

    assert_eq!(head.status, 415);
    assert!(
        message_of(&body).contains("x-www-form-urlencoded"),
        "{body}"
    );
}

/// The endpoint inherits axum's body limit rather than inventing one, and a
/// caller's `DefaultBodyLimit` layer still governs it.
#[tokio::test(flavor = "multi_thread")]
async fn an_oversized_body_is_a_413() {
    let app = Router::new()
        .route_agui("/agent", Chatty)
        .layer(axum::extract::DefaultBodyLimit::max(1024));
    let addr = serve(app).await;

    let mut oversized =
        br#"{"threadId":"t","runId":"r","messages":[],"tools":[],"context":[],"forwardedProps":""#
            .to_vec();
    oversized.extend(std::iter::repeat_n(b'x', 4096));
    oversized.extend(br#""}"#);

    let (head, _) = request(addr, &[], &oversized).await;
    assert_eq!(head.status, 413);
}

#[tokio::test(flavor = "multi_thread")]
async fn an_accept_this_endpoint_cannot_satisfy_is_a_406() {
    let addr = serve(Router::new().route_agui("/agent", Chatty)).await;

    for accept in ["application/xml", "text/event-stream;q=0", "text/plain"] {
        let (head, body) = request(addr, &[("accept", accept)], &input()).await;
        assert_eq!(head.status, 406, "{accept} should be refused");
        // The refusal says what it could have sent instead of failing silently.
        assert!(message_of(&body).contains("text/event-stream"), "{body}");
    }

    // …and the ones it can satisfy still work.
    for accept in [
        "*/*",
        "text/event-stream",
        "text/*",
        "application/json, text/*;q=0.1",
    ] {
        let (head, _) = request(addr, &[("accept", accept)], &input()).await;
        assert_eq!(head.status, 200, "{accept} should be served");
    }
}

/// The one that matters: a client that goes away must reach the agent.
#[tokio::test(flavor = "multi_thread")]
async fn dropping_the_client_cancels_the_run() {
    let (tx, mut rx) = unbounded_channel();
    let emitted = Arc::new(AtomicUsize::new(0));
    let agent = Endless {
        token: tx,
        emitted: Arc::clone(&emitted),
    };
    let addr = serve(Router::new().route_agui("/agent", agent)).await;

    let mut client = Client::connect(addr).await;
    client.post("/agent", &[], &input()).await;
    assert_eq!(client.read_head().await.status, 200);

    // Read a few frames so the run is unambiguously under way.
    for _ in 0..3 {
        client
            .read_chunk()
            .await
            .expect("the stream should still be running");
    }

    let token = rx
        .recv()
        .await
        .expect("the agent should have handed out its token");
    assert!(!token.is_cancelled(), "nothing has disconnected yet");

    // Pull the plug. Closing a socket with data still queued on it resets the
    // connection, and the agent is mid-sentence, so there is: the server's next
    // write fails rather than waiting on a timeout.
    drop(client);

    timeout(Duration::from_secs(5), token.cancelled())
        .await
        .expect("the disconnect should have cancelled the run");

    // And the agent really stopped, rather than streaming into a dead socket.
    sleep(Duration::from_millis(200)).await;
    let after_cancel = emitted.load(Ordering::SeqCst);
    sleep(Duration::from_millis(200)).await;
    assert_eq!(
        emitted.load(Ordering::SeqCst),
        after_cancel,
        "the agent kept running after the client left"
    );
}

/// The same thing, for a run that is not writing anything.
///
/// The chatty case could pass on the write path alone — the server notices
/// because its next write fails. The case that actually matters in production
/// is the other one: the user hits stop while the agent is thirty seconds into
/// a model call, so there is no write to fail and the server has to notice on
/// the read side.
#[tokio::test(flavor = "multi_thread")]
async fn a_client_that_leaves_mid_thought_cancels_the_run() {
    let (tx, mut rx) = unbounded_channel();
    let addr = serve(Router::new().route_agui("/agent", Waiting { token: tx })).await;

    let mut client = Client::connect(addr).await;
    client.post("/agent", &[], &input()).await;
    assert_eq!(client.read_head().await.status, 200);
    client
        .read_chunk()
        .await
        .expect("the run should have started");

    let token = rx
        .recv()
        .await
        .expect("the agent should have handed out its token");

    // Drain what the agent said before it went quiet, so the socket closes with
    // nothing outstanding — a plain FIN, not a reset.
    while let Ok(Some(_)) = timeout(Duration::from_millis(200), client.read_chunk()).await {}
    drop(client);

    timeout(Duration::from_secs(5), token.cancelled())
        .await
        .expect("leaving during a long model call should cancel the run");
}

/// The other half of the same guard: finishing is not disconnecting.
#[tokio::test(flavor = "multi_thread")]
async fn a_run_that_completes_is_never_reported_as_cancelled() {
    let (tx, mut rx) = unbounded_channel();
    let addr = serve(Router::new().route_agui("/agent", Prompt { token: tx })).await;

    let (head, body) = request(addr, &[], &input()).await;
    assert_eq!(head.status, 200);
    assert_eq!(
        events(&body).last().map(Event::event_type),
        Some(EventType::RunFinished)
    );

    let token = rx
        .recv()
        .await
        .expect("the agent should have handed out its token");
    // The response body is dropped after the last byte is written; give that
    // drop every chance to fire before claiming it did not cancel.
    sleep(Duration::from_millis(200)).await;
    assert!(
        !token.is_cancelled(),
        "a completed run must not look like a disconnected one"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn keep_alive_comments_fill_a_silent_run() {
    let endpoint = AgentEndpoint::new(Slow {
        quiet: Duration::from_millis(400),
    })
    .keep_alive(Duration::from_millis(50));
    let addr = serve(Router::new().route_agui_with("/agent", endpoint)).await;

    let (head, body) = request(addr, &[], &input()).await;
    assert_eq!(head.status, 200);

    // Comments carry no `data:` line, so they are invisible to the decoder …
    let types: Vec<EventType> = events(&body).iter().map(Event::event_type).collect();
    assert_eq!(
        types,
        [
            EventType::RunStarted,
            EventType::TextMessageStart,
            EventType::TextMessageContent,
            EventType::TextMessageEnd,
            EventType::RunFinished,
        ]
    );
    // … but they were on the wire, holding the connection open.
    assert!(
        body.matches(":\n\n").count() >= 2,
        "expected keep-alive comments in {body:?}"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn an_agent_mounts_inside_a_stateful_router_alongside_other_routes() {
    #[derive(Clone)]
    struct AppState {
        greeting: &'static str,
    }

    // `route_agui` is called on a `Router<AppState>`, before `with_state`:
    // mounting an agent constrains the state type not at all.
    let app: Router = Router::new()
        .route(
            "/health",
            get(
                |axum::extract::State(state): axum::extract::State<AppState>| async move {
                    state.greeting
                },
            ),
        )
        .route_agui("/agent", Chatty)
        .nest(
            "/v2",
            Router::new().route_agui_with("/agent", AgentEndpoint::new(Chatty)),
        )
        .with_state(AppState { greeting: "ok" });

    let addr = serve(app).await;

    let (head, body) = request(addr, &[], &input()).await;
    assert_eq!(head.status, 200);
    assert_eq!(
        events(&body).last().map(Event::event_type),
        Some(EventType::RunFinished)
    );

    // The nested mount answers the same way.
    let mut client = Client::connect(addr).await;
    client.post("/v2/agent", &[], &input()).await;
    assert_eq!(client.read_head().await.status, 200);

    // The user's own routes are untouched …
    let mut client = Client::connect(addr).await;
    client.get("/health").await;
    let head = client.read_head().await;
    assert_eq!(head.status, 200);
    assert_eq!(client.read_body(&head).await, b"ok");

    // … and axum still owns method routing on the agent's path.
    let mut client = Client::connect(addr).await;
    client.get("/agent").await;
    assert_eq!(client.read_head().await.status, 405);
}

#[tokio::test(flavor = "multi_thread")]
async fn a_transformer_rewrites_the_stream_per_run() {
    let endpoint =
        AgentEndpoint::new(Chatty).transformer(|| FilterToolCalls::deny(["internal_debug"]));
    let addr = serve(Router::new().route_agui_with("/agent", endpoint)).await;

    // Twice, because a transformer is a state machine: a chain shared across
    // runs would carry the first run's dropped ids into the second.
    for attempt in 0..2 {
        let (head, body) = request(addr, &[], &input()).await;
        assert_eq!(head.status, 200);
        let types: Vec<EventType> = events(&body).iter().map(Event::event_type).collect();
        assert_eq!(
            types,
            [
                EventType::RunStarted,
                EventType::StepStarted,
                EventType::TextMessageStart,
                EventType::TextMessageContent,
                EventType::TextMessageContent,
                EventType::TextMessageEnd,
                EventType::StepFinished,
                EventType::RunFinished,
            ],
            "run {attempt} should have no tool call in it"
        );
    }
}

/// One mounted agent, many runs at once: nothing behind the endpoint is shared
/// mutable state.
#[tokio::test(flavor = "multi_thread")]
async fn concurrent_runs_do_not_interfere() {
    let endpoint =
        AgentEndpoint::new(Chatty).transformer(|| FilterToolCalls::deny(["internal_debug"]));
    let addr = serve(Router::new().route_agui_with("/agent", endpoint)).await;

    let runs: Vec<_> = (0..16)
        .map(|_| tokio::spawn(async move { request(addr, &[], &input()).await }))
        .collect();

    for run in runs {
        let (head, body) = run.await.expect("the request task to finish");
        assert_eq!(head.status, 200);
        let types: Vec<EventType> = events(&body).iter().map(Event::event_type).collect();
        assert_eq!(
            types.first(),
            Some(&EventType::RunStarted),
            "every run gets its own stream: {body}"
        );
        assert_eq!(types.last(), Some(&EventType::RunFinished), "{body}");
        assert!(
            !types.contains(&EventType::ToolCallStart),
            "every run gets its own transformer chain: {body}"
        );
    }
}

// --------------------------------------------------------------- harness ----

/// A router on a real port.
async fn serve(app: Router) -> SocketAddr {
    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .expect("a free port on loopback");
    let addr = listener.local_addr().expect("the bound address");
    tokio::spawn(async move {
        axum::serve(listener, app).await.expect("the server to run");
    });
    addr
}

/// A minimal `RunAgentInput` body.
fn input() -> Vec<u8> {
    serde_json::to_vec(&RunAgentInput::new("thread-1", "run-1")).expect("serializable input")
}

/// One request, one whole response.
async fn request(addr: SocketAddr, headers: &[(&str, &str)], body: &[u8]) -> (Head, String) {
    let mut client = Client::connect(addr).await;
    client.post("/agent", headers, body).await;
    let head = client.read_head().await;
    let body = client.read_body(&head).await;
    (
        head,
        String::from_utf8(body).expect("a utf-8 response body"),
    )
}

/// The `message` field of this crate's JSON error body.
fn message_of(body: &str) -> String {
    let value: serde_json::Value = serde_json::from_str(body).expect("a JSON error body");
    value["message"]
        .as_str()
        .expect("a message field")
        .to_owned()
}

/// Decodes an SSE body into the events it carries.
///
/// Frames with no `data:` line — keep-alive comments — carry no event and are
/// skipped, which is exactly what a browser's `EventSource` does with them.
fn events(body: &str) -> Vec<Event> {
    body.split("\n\n")
        .filter(|frame| !frame.trim().is_empty())
        .filter_map(|frame| {
            let data: Vec<&str> = frame
                .lines()
                .filter_map(|line| line.strip_prefix("data: "))
                .collect();
            if data.is_empty() {
                return None;
            }
            Some(serde_json::from_str(&data.join("\n")).expect("an AG-UI event"))
        })
        .collect()
}

/// A response's status and headers.
struct Head {
    status: u16,
    headers: Vec<(String, String)>,
}

impl Head {
    fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(key, _)| key == name)
            .map(|(_, value)| value.as_str())
    }
}

/// An HTTP/1.1 client that hands back one chunk at a time.
struct Client {
    stream: TcpStream,
    pending: Vec<u8>,
}

impl Client {
    async fn connect(addr: SocketAddr) -> Self {
        let stream = TcpStream::connect(addr).await.expect("a connection");
        Self {
            stream,
            pending: Vec::new(),
        }
    }

    async fn post(&mut self, path: &str, headers: &[(&str, &str)], body: &[u8]) {
        let mut request = format!(
            "POST {path} HTTP/1.1\r\nhost: localhost\r\ncontent-length: {}\r\n",
            body.len()
        );
        if !headers.iter().any(|(name, _)| *name == "content-type") {
            request.push_str("content-type: application/json\r\n");
        }
        for (name, value) in headers {
            request.push_str(&format!("{name}: {value}\r\n"));
        }
        request.push_str("\r\n");

        self.stream
            .write_all(request.as_bytes())
            .await
            .expect("the request head to go out");
        self.stream
            .write_all(body)
            .await
            .expect("the request body to go out");
    }

    async fn get(&mut self, path: &str) {
        let request = format!("GET {path} HTTP/1.1\r\nhost: localhost\r\n\r\n");
        self.stream
            .write_all(request.as_bytes())
            .await
            .expect("the request to go out");
    }

    async fn read_head(&mut self) -> Head {
        let raw = self.take_until(b"\r\n\r\n").await;
        let text = String::from_utf8(raw).expect("a utf-8 response head");
        let mut lines = text.lines();

        let status = lines
            .next()
            .and_then(|line| line.split_whitespace().nth(1).map(str::to_owned))
            .and_then(|code| code.parse().ok())
            .expect("a status line");
        let headers = lines
            .filter(|line| !line.is_empty())
            .filter_map(|line| line.split_once(':'))
            .map(|(name, value)| (name.trim().to_ascii_lowercase(), value.trim().to_owned()))
            .collect();

        Head { status, headers }
    }

    /// One `Transfer-Encoding: chunked` chunk, or `None` at the terminator.
    async fn read_chunk(&mut self) -> Option<Vec<u8>> {
        let header = self.take_until(b"\r\n").await;
        let header = String::from_utf8_lossy(&header);
        let size = usize::from_str_radix(
            header
                .trim()
                .split(';')
                .next()
                .expect("a chunk size")
                .trim(),
            16,
        )
        .expect("a hexadecimal chunk size");

        if size == 0 {
            self.take_until(b"\r\n").await; // end of trailers
            return None;
        }

        let chunk = self.take(size).await;
        self.take(2).await; // the chunk's own CRLF
        Some(chunk)
    }

    async fn read_body(&mut self, head: &Head) -> Vec<u8> {
        if let Some(length) = head.header("content-length") {
            let length = length.parse().expect("a numeric content-length");
            return self.take(length).await;
        }
        let mut body = Vec::new();
        while let Some(chunk) = self.read_chunk().await {
            body.extend(chunk);
        }
        body
    }

    /// Consumes `count` bytes, reading more as needed.
    async fn take(&mut self, count: usize) -> Vec<u8> {
        while self.pending.len() < count {
            assert!(self.fill().await, "the connection closed mid-body");
        }
        self.pending.drain(..count).collect()
    }

    /// Consumes everything up to and including the first `needle`.
    async fn take_until(&mut self, needle: &[u8]) -> Vec<u8> {
        loop {
            if let Some(at) = self
                .pending
                .windows(needle.len())
                .position(|window| window == needle)
            {
                return self.pending.drain(..at + needle.len()).collect();
            }
            assert!(
                self.fill().await,
                "the connection closed before {:?}",
                String::from_utf8_lossy(needle)
            );
        }
    }

    /// Reads once. `false` once the peer is done sending.
    async fn fill(&mut self) -> bool {
        let mut buffer = [0_u8; 8192];
        let read = self
            .stream
            .read(&mut buffer)
            .await
            .expect("the connection to stay readable");
        self.pending.extend_from_slice(&buffer[..read]);
        read > 0
    }
}
