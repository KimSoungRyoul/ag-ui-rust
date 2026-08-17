//! The awkward agent: a backend that produces the shapes real providers do.
//!
//! This is not the application. It is the fixture the application is aimed at,
//! and it lives here because a client dogfood is only worth as much as the
//! streams it is pointed at. `task-board`'s agent is well-behaved — bracketed
//! messages, whole tool arguments, one call at a time — so it proves the happy
//! path and nothing else. Real streams are worse:
//!
//! - text arrives as `TEXT_MESSAGE_CHUNK` with the id **only on the first one**;
//! - tool arguments are split at arbitrary byte offsets, including between a
//!   backslash and the character it escapes;
//! - a model calls two tools at once and their events interleave;
//! - a producer in another language sends something the protocol forbids.
//!
//! Every scenario is chosen by the first word of the user's message, so a
//! transcript names what it exercised. Nothing here is timed and nothing is
//! random: the same message produces the same bytes every run.
//!
//! # Why so much of it is raw `emit`
//!
//! `ag-ui-server`'s typestate handles bracket what they open, which is exactly
//! what a chunk event is defined not to do — there is no `ctx.text_chunk()`,
//! and two overlapping [`ToolCallHandle`](ag_ui_server::ToolCallHandle)s are a
//! borrow-check error by design. Producing provider-shaped output therefore
//! means dropping to [`RunContext::emit`], which is the documented escape
//! hatch. See the report: this is a finding, not a complaint about the design.

use ag_ui_a2ui::constants::RENDER_A2UI_TOOL_NAME;
use ag_ui_a2ui::message::Component;
use ag_ui_a2ui::toolkit::envelope::wrap_as_operations_envelope;
use ag_ui_a2ui::toolkit::ops::{Intent, SurfaceSpec, assemble_ops};
use ag_ui_axum::RouterExt;
use ag_ui_core::{
    Event, Interrupt, MessageId, ResumeStatus, RunOutcome, TextMessageRole, ToolCallId,
};
use ag_ui_server::{Agent, CancellationToken, Error, Result, RunContext};
use axum::Router;
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use serde_json::json;
use tokio::sync::mpsc::UnboundedSender;

use crate::board::{Board, Task};

/// Where the awkward agent is mounted.
pub const ROUTE: &str = "/agent";

/// Where the hand-framed, deliberately illegal streams are mounted.
///
/// `/raw/{scenario}` — see [`raw_script`].
pub const RAW_ROUTE: &str = "/raw/{scenario}";

/// The interrupt ids the `approve` scenario pauses on.
pub const BUDGET: &str = "approve-budget";
/// The second of them. Two, because answering one per request never terminates.
pub const DATE: &str = "confirm-date";

/// An agent that answers in the shapes a provider adapter produces.
#[derive(Debug, Default)]
pub struct Awkward {
    /// Set by tests: reports, as the run's future is dropped, whether the run
    /// had been cancelled. A client that stops reading has to reach the agent,
    /// and nothing observable from the client side proves that it did.
    exits: Option<UnboundedSender<bool>>,
}

impl Awkward {
    /// The agent as the CLI serves it.
    pub fn new() -> Self {
        Self::default()
    }

    /// The agent, reporting every run's cancellation state as it exits.
    pub fn reporting(exits: UnboundedSender<bool>) -> Self {
        Self { exits: Some(exits) }
    }
}

/// Reports whether the run was cancelled on *every* way out, including the one
/// where the agent's future is simply dropped mid-await.
struct ExitGuard {
    token: CancellationToken,
    report: UnboundedSender<bool>,
}

impl Drop for ExitGuard {
    fn drop(&mut self) {
        let _ = self.report.send(self.token.is_cancelled());
    }
}

impl Agent for Awkward {
    type State = Board;

    async fn run(&self, ctx: &mut RunContext<Board>) -> Result<RunOutcome> {
        let said = ctx.last_user_text().unwrap_or_default();
        let scenario = said.split_whitespace().next().unwrap_or("").to_lowercase();

        let _guard = self.exits.clone().map(|report| ExitGuard {
            token: ctx.cancel_token(),
            report,
        });

        match scenario.as_str() {
            "chunks" => chunked_text(ctx),
            "call" => chunked_call(ctx),
            "parallel" => parallel_calls(ctx),
            "mixed" => mixed_stream(ctx),
            "busy" => return busy(ctx),
            "approve" => return Ok(approvals(ctx)),
            "slow" => return slow(ctx).await,
            "fail" => Err(Error::agent("the model refused, and said so at length")),
            _ => board_turn(ctx),
        }?;

        Ok(RunOutcome::Success)
    }
}

/// Text as `TEXT_MESSAGE_CHUNK`, the id on the first chunk only.
///
/// The last three fragments split a ZWJ emoji sequence between its parts: every
/// fragment is valid UTF-8 on its own — a `String` cannot be otherwise — but
/// the *grapheme* is only whole once they are rejoined.
fn chunked_text(ctx: &mut RunContext<Board>) -> Result<()> {
    let id = ctx.new_message_id();
    let fragments = [
        "Chunked text arrives in frag",
        "ments, and the client rejoins ",
        "them — emoji included: 👩",
        "\u{200d}",
        "💻.",
    ];

    for (index, fragment) in fragments.iter().enumerate() {
        // The id rides on the first chunk and nothing else. A client that does
        // not remember it drops everything after this line.
        let carried = (index == 0).then(|| id.clone());
        ctx.emit(Event::text_message_chunk(
            carried,
            Some((*fragment).to_owned()),
        ))?;
    }
    Ok(())
}

/// One tool call as `TOOL_CALL_CHUNK`, arguments split at hostile offsets.
///
/// The split after `line\` is the case every provider adapter gets wrong once:
/// the backslash and the `n` it escapes arrive in different events, so anything
/// that parses a fragment on its own sees invalid JSON.
fn chunked_call(ctx: &mut RunContext<Board>) -> Result<()> {
    let id = ctx.new_tool_call_id();
    let fragments = [
        r#"{"no"#,
        r#"te":"line\"#,
        r#"nbreak","ti"#,
        r#"tle":"ship "#,
        r#"the SDK","depth":3}"#,
    ];

    for (index, fragment) in fragments.iter().enumerate() {
        let first = index == 0;
        ctx.emit(Event::tool_call_chunk(
            first.then(|| id.clone()),
            first.then(|| "add_task".to_owned()),
            Some((*fragment).to_owned()),
        ))?;
    }

    // A result still needs an explicit event; only the call itself chunks.
    let message_id = ctx.new_message_id();
    ctx.emit(Event::tool_call_result(
        message_id,
        id,
        r#"{"id":1,"title":"ship the SDK"}"#,
    ))
}

/// Two calls in flight at once, their events interleaved by id.
///
/// Legal on the wire and legal for the applier, and *unwritable* with the
/// typestate handles: two open [`ToolCallHandle`](ag_ui_server::ToolCallHandle)s
/// do not compile. Hence the raw emits.
fn parallel_calls(ctx: &mut RunContext<Board>) -> Result<()> {
    let (first, second) = (ctx.new_tool_call_id(), ctx.new_tool_call_id());
    let (first_result, second_result) = (ctx.new_message_id(), ctx.new_message_id());

    ctx.emit(Event::tool_call_start(first.clone(), "add_task"))?;
    ctx.emit(Event::tool_call_start(second.clone(), "add_task"))?;
    ctx.emit(Event::tool_call_args(first.clone(), r#"{"title":"#))?;
    ctx.emit(Event::tool_call_args(second.clone(), r#"{"title":"#))?;
    ctx.emit(Event::tool_call_args(first.clone(), r#""write it down"}"#))?;
    ctx.emit(Event::tool_call_args(second.clone(), r#""read it back"}"#))?;
    ctx.emit(Event::tool_call_end(first.clone()))?;
    ctx.emit(Event::tool_call_end(second.clone()))?;
    ctx.emit(Event::tool_call_result(
        first_result,
        first,
        r#"{"id":1,"title":"write it down"}"#,
    ))?;
    ctx.emit(Event::tool_call_result(
        second_result,
        second,
        r#"{"id":2,"title":"read it back"}"#,
    ))?;

    ctx.update_state(|board| {
        board.tasks = vec![task(1, "write it down"), task(2, "read it back")];
    })
}

/// Reasoning, then text, then a call — all as chunks, with no explicit end
/// anywhere.
///
/// Each stream is closed only by the next one starting, and the last by the end
/// of the run. A client that waits for an explicit terminator hangs on the
/// final message forever.
fn mixed_stream(ctx: &mut RunContext<Board>) -> Result<()> {
    let thought = ctx.new_message_id();
    ctx.emit(Event::reasoning_message_chunk(
        Some(thought),
        Some("three streams, no brackets".to_owned()),
    ))?;

    let text = ctx.new_message_id();
    ctx.emit(Event::text_message_chunk(
        Some(text),
        Some("Reading the board".to_owned()),
    ))?;
    ctx.emit(Event::text_message_chunk(
        None,
        Some(", then adding to it.".to_owned()),
    ))?;

    let call = ctx.new_tool_call_id();
    ctx.emit(Event::tool_call_chunk(
        Some(call),
        Some("add_task".to_owned()),
        Some(r#"{"title":"unbracketed"}"#.to_owned()),
    ))
}

/// Does work *and then* pauses, in one run.
///
/// The interaction the separate scenarios miss: two calls in flight, state
/// published, and only then a decision the agent needs a human for. What it
/// exercises on the client side is that a run which already grew the
/// conversation can still pause — and that resuming carries the tool messages
/// the first half produced, rather than starting from the user's turn.
fn busy(ctx: &mut RunContext<Board>) -> Result<RunOutcome> {
    if ctx.resume_for(BUDGET).is_none() {
        parallel_calls(ctx)?;
        ctx.say("Two added. The third needs sign-off.")?;
        return Ok(RunOutcome::interrupt(vec![Interrupt {
            id: BUDGET.to_owned(),
            reason: "tool_approval".to_owned(),
            message: Some("Add the third task too?".to_owned()),
            ..Default::default()
        }]));
    }

    let approved = ctx
        .resume_for(BUDGET)
        .is_some_and(|entry| entry.status == ResumeStatus::Resolved);
    if approved {
        let id = ctx.new_tool_call_id();
        let result = ctx.new_message_id();
        ctx.emit(Event::tool_call_start(id.clone(), "add_task"))?;
        ctx.emit(Event::tool_call_args(
            id.clone(),
            r#"{"title":"sign it off"}"#,
        ))?;
        ctx.emit(Event::tool_call_end(id.clone()))?;
        ctx.emit(Event::tool_call_result(
            result,
            id,
            r#"{"id":3,"title":"sign it off"}"#,
        ))?;
        ctx.update_state(|board| board.tasks.push(task(3, "sign it off")))?;
        ctx.say("Three on the board.")?;
    } else {
        ctx.say("Left it at two.")?;
    }
    Ok(RunOutcome::Success)
}

/// Pauses on two decisions at once.
///
/// Two, not one, because answering them one request at a time never terminates
/// — the agent only ever sees the answers the current request carries.
fn approvals(ctx: &mut RunContext<Board>) -> RunOutcome {
    let pending: Vec<Interrupt> = [(BUDGET, "Approve the budget?"), (DATE, "Confirm the date?")]
        .into_iter()
        .filter(|(id, _)| ctx.resume_for(id).is_none())
        .map(|(id, question)| Interrupt {
            id: id.to_owned(),
            reason: "tool_approval".to_owned(),
            message: Some(question.to_owned()),
            ..Default::default()
        })
        .collect();

    if !pending.is_empty() {
        return RunOutcome::interrupt(pending);
    }

    let declined: Vec<&str> = [BUDGET, DATE]
        .into_iter()
        .filter(|id| {
            ctx.resume_for(id)
                .is_some_and(|entry| entry.status == ResumeStatus::Cancelled)
        })
        .collect();

    let _ = match declined.len() {
        0 => ctx.say("Both approved. Booked."),
        2 => ctx.say("Both declined. Nothing booked."),
        _ => ctx.say(format!(
            "Declined: {}. Nothing booked.",
            declined.join(", ")
        )),
    };
    RunOutcome::Success
}

/// Says one thing, then waits forever — an agent thirty seconds into a model
/// call, which is when a user actually hits stop.
async fn slow(ctx: &mut RunContext<Board>) -> Result<RunOutcome> {
    ctx.say("working on it, this will take a while")?;
    std::future::pending::<()>().await;
    Ok(RunOutcome::Success)
}

/// The well-behaved turn: reasoning, a bracketed message, state, and a surface.
///
/// Here so that pointing the watcher at this server shows the same shape as
/// pointing it at `task-board`, and the awkward scenarios read as departures
/// from something rather than as the only thing on offer.
fn board_turn(ctx: &mut RunContext<Board>) -> Result<()> {
    let mut step = ctx.step("turn")?;
    step.think("nothing unusual about this one")?;

    let mut message = step.assistant_message()?;
    for word in "Two tasks on the board. ".split_inclusive(' ') {
        message.delta(word)?;
    }
    // A handle can reach the run state mid-message, so the reply can quote the
    // board it is about to publish without closing first.
    message.state_mut().tasks = vec![task(1, "draft the agenda"), task(2, "book the room")];
    message.delta(message.state().summary())?;
    message.end()?;

    step.publish_state()?;
    surface(&mut step)
}

/// Ships the board as an A2UI surface in a tool result envelope.
fn surface(ctx: &mut RunContext<Board>) -> Result<()> {
    let spec = SurfaceSpec::new("board-watch")
        .with_components(vec![
            Component::new("root", "Column").with("children", json!(["heading", "list"])),
            Component::new("heading", "Text")
                .with("text", json!({"path": "/title"}))
                .with("variant", json!("h2")),
            Component::new("list", "List")
                .with("children", json!({"componentId": "row", "path": "/tasks"})),
            Component::new("row", "Text").with("text", json!({"path": "line"})),
        ])
        .with_data_model(json!({
            "title": "Watched board",
            "tasks": ctx
                .state()
                .tasks
                .iter()
                .map(|task| json!({"line": task.line()}))
                .collect::<Vec<_>>(),
        }));

    let envelope =
        wrap_as_operations_envelope(&assemble_ops(Intent::Create, &spec)).map_err(Error::agent)?;
    let mut call = ctx.tool_call(RENDER_A2UI_TOOL_NAME)?;
    call.args_json(&json!({"surfaceId": "board-watch"}))?;
    call.result(envelope)?;
    Ok(())
}

fn task(id: u32, title: &str) -> Task {
    Task {
        id,
        title: title.to_owned(),
        estimate_minutes: None,
        done: false,
    }
}

/// The whole fake backend: the agent, the raw endpoint, and a health check.
pub fn router(agent: Awkward) -> Router {
    Router::new()
        .route("/health", get(|| async { "ok" }))
        .route(RAW_ROUTE, post(raw).get(raw))
        .route_agui(ROUTE, agent)
}

/// A hand-framed `text/event-stream`, bypassing `ag-ui-server` entirely.
///
/// The server's own verifier will not emit a malformed stream — that is what it
/// is for — so the only way to hand the *client's* verifier something to reject
/// is to frame the bytes here, the way a producer in another language does.
/// [`SseFormatter`](ag_ui_core::SseFormatter) is the same encoder the real
/// endpoint uses; only the ordering is wrong.
async fn raw(axum::extract::Path(scenario): axum::extract::Path<String>) -> Response {
    use ag_ui_core::SseFormatter;

    let formatter = SseFormatter::new();
    let mut body = String::new();
    for event in raw_script(&scenario) {
        match formatter.encode_to_string(&event) {
            Ok(frame) => body.push_str(&frame),
            Err(error) => {
                return (
                    axum::http::StatusCode::INTERNAL_SERVER_ERROR,
                    error.to_string(),
                )
                    .into_response();
            }
        }
    }

    (
        [(axum::http::header::CONTENT_TYPE, "text/event-stream")],
        body,
    )
        .into_response()
}

/// The scripted illegal streams, by name.
///
/// - `unbracketed` — content for a message that was never started.
/// - `truncated` — a run that stops without saying how.
/// - `orphan-result` — a result for a call nobody made.
pub fn raw_script(scenario: &str) -> Vec<Event> {
    let started = Event::run_started("raw", "raw-1");
    match scenario {
        "unbracketed" => vec![
            started,
            Event::text_message_content(MessageId::new("ghost"), "text nobody opened"),
            Event::run_finished_success("raw", "raw-1"),
        ],
        "truncated" => vec![
            started,
            Event::text_message_start(MessageId::new("cut"), TextMessageRole::Assistant),
            Event::text_message_content(MessageId::new("cut"), "half a sen"),
        ],
        "orphan-result" => vec![
            started,
            Event::tool_call_result(
                MessageId::new("answer"),
                ToolCallId::new("never-called"),
                "{}",
            ),
            Event::run_finished_success("raw", "raw-1"),
        ],
        _ => vec![started, Event::run_finished_success("raw", "raw-1")],
    }
}
