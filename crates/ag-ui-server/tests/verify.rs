//! One test per ordering rule, plus proof that a valid stream passes.
//!
//! The typed handles make most of these unreachable — which is the point — so
//! the violations here go through the raw [`RunContext::emit`] escape hatch.

#![cfg(feature = "verify")]

use ag_ui_core::{Event, EventType, Interrupt, RunAgentInput, RunOutcome, TextMessageRole};
use ag_ui_server::{Agent, Error, EventReceiver, Result, Rule, RunContext, run};
use futures_util::StreamExt as _;

fn context() -> (RunContext<()>, EventReceiver) {
    RunContext::new(RunAgentInput::new("t", "r")).expect("an empty state always decodes")
}

/// The rule an emit was rejected by.
fn rule(error: Error) -> Rule {
    match error {
        Error::Verification(violation) => violation.rule,
        other => panic!("expected a verification error, got {other:?}"),
    }
}

#[test]
fn a_valid_stream_passes() {
    let (mut ctx, _events) = context();
    for event in [
        Event::run_started("t", "r"),
        Event::step_started("plan"),
        Event::text_message_start("m1", TextMessageRole::Assistant),
        Event::text_message_content("m1", "hello"),
        Event::text_message_end("m1"),
        Event::reasoning_start("m2"),
        Event::reasoning_message_start("m2"),
        Event::reasoning_message_content("m2", "hmm"),
        Event::reasoning_message_end("m2"),
        Event::reasoning_end("m2"),
        Event::tool_call_start("c1", "search"),
        Event::tool_call_args("c1", "{}"),
        Event::tool_call_end("c1"),
        Event::tool_call_result("m3", "c1", "ok"),
        Event::state_snapshot(serde_json::json!({"a": 1})),
        Event::custom("ping", serde_json::json!(1)),
        Event::step_finished("plan"),
        Event::run_finished_success("t", "r"),
    ] {
        ctx.emit(event.clone())
            .unwrap_or_else(|error| panic!("{event:?} should be accepted: {error}"));
    }
}

#[test]
fn parallel_tool_calls_and_overlapping_messages_are_accepted() {
    // The typestate handles cannot express this — a second overlapping handle
    // is a borrow-check error, which is the whole point of them — so parallel
    // tool calling goes through `emit`. The verifier must not be the thing that
    // stops it: upstream's client keys everything by id
    // (`client/src/verify/verify.ts`, `activeMessages` / `activeToolCalls` as
    // maps, and "should allow overlapping text messages and tool calls" in its
    // tests), so this is the stream a provider doing parallel calls sends.
    let (mut ctx, _events) = context();
    for event in [
        Event::run_started("t", "r"),
        Event::text_message_start("m1", TextMessageRole::Assistant),
        Event::text_message_content("m1", "Checking both."),
        Event::tool_call_start("c1", "get_weather"),
        Event::tool_call_start("c2", "get_time"),
        Event::tool_call_args("c1", r#"{"city":"#),
        Event::tool_call_args("c2", r#"{"zone":"#),
        Event::tool_call_args("c1", r#""Seoul"}"#),
        Event::tool_call_end("c1"),
        Event::tool_call_args("c2", r#""KST"}"#),
        Event::tool_call_end("c2"),
        Event::text_message_end("m1"),
        Event::run_finished_success("t", "r"),
    ] {
        ctx.emit(event.clone())
            .unwrap_or_else(|error| panic!("{event:?} should be accepted: {error}"));
    }
}

#[test]
fn content_without_a_start_is_rejected() {
    let (mut ctx, _events) = context();
    let error = ctx
        .emit(Event::text_message_content("m1", "hello"))
        .expect_err("m1 was never opened");
    assert_eq!(rule(error), Rule::NotOpen);
}

#[test]
fn an_end_without_a_start_is_rejected() {
    let (mut ctx, _events) = context();
    let error = ctx
        .emit(Event::text_message_end("m1"))
        .expect_err("m1 was never opened");
    assert_eq!(rule(error), Rule::NotOpen);
}

#[test]
fn a_duplicate_start_is_rejected() {
    let (mut ctx, _events) = context();
    ctx.emit(Event::text_message_start("m1", TextMessageRole::Assistant))
        .expect("the first start is fine");
    let error = ctx
        .emit(Event::text_message_start("m1", TextMessageRole::Assistant))
        .expect_err("m1 is already open");
    assert_eq!(rule(error), Rule::DuplicateStart);
}

#[test]
fn a_second_run_started_is_rejected() {
    let (mut ctx, _events) = context();
    ctx.emit(Event::run_started("t", "r"))
        .expect("the first start is fine");
    let error = ctx
        .emit(Event::run_started("t", "r"))
        .expect_err("the run already started");
    assert_eq!(rule(error), Rule::DuplicateRunStarted);
}

#[test]
fn finishing_with_a_message_open_is_rejected() {
    let (mut ctx, _events) = context();
    ctx.emit(Event::text_message_start("m1", TextMessageRole::Assistant))
        .expect("start");
    let error = ctx
        .emit(Event::run_finished_success("t", "r"))
        .expect_err("m1 is still open");
    assert_eq!(rule(error), Rule::OpenAtFinish);
}

#[test]
fn finishing_with_a_step_open_is_rejected() {
    let (mut ctx, _events) = context();
    ctx.emit(Event::step_started("plan")).expect("start");
    let error = ctx
        .emit(Event::run_finished_success("t", "r"))
        .expect_err("the step is still open");
    assert_eq!(rule(error), Rule::OpenAtFinish);
}

#[test]
fn run_error_may_leave_things_open() {
    let (mut ctx, _events) = context();
    ctx.emit(Event::text_message_start("m1", TextMessageRole::Assistant))
        .expect("start");
    ctx.emit(Event::run_error("the model hung up"))
        .expect("a failed run cannot be expected to have tidied up");
}

#[test]
fn events_after_the_run_ended_are_rejected() {
    let (mut ctx, _events) = context();
    ctx.emit(Event::run_finished_success("t", "r"))
        .expect("finish");
    let error = ctx
        .emit(Event::custom("late", serde_json::json!(1)))
        .expect_err("the run has ended");
    assert_eq!(rule(error), Rule::RunEnded);
}

#[test]
fn a_result_for_an_unknown_call_is_rejected() {
    let (mut ctx, _events) = context();
    let error = ctx
        .emit(Event::tool_call_result("m1", "c-nope", "ok"))
        .expect_err("c-nope was never started");
    assert_eq!(rule(error), Rule::UnknownId);
}

#[test]
fn a_result_before_the_call_ends_is_rejected() {
    let (mut ctx, _events) = context();
    ctx.emit(Event::tool_call_start("c1", "search"))
        .expect("start");
    let error = ctx
        .emit(Event::tool_call_result("m1", "c1", "ok"))
        .expect_err("c1 has no TOOL_CALL_END yet");
    assert_eq!(rule(error), Rule::OutOfOrder);
}

#[test]
fn a_chunk_needs_no_bracketing() {
    let (mut ctx, _events) = context();
    ctx.emit(Event::text_message_chunk(
        Some("m1".into()),
        Some("whole thing".to_owned()),
    ))
    .expect("chunks are self-contained");
    ctx.emit(Event::tool_call_chunk(
        Some("c1".into()),
        Some("search".to_owned()),
        Some("{}".to_owned()),
    ))
    .expect("chunks are self-contained");
    ctx.emit(Event::tool_call_result("m2", "c1", "ok"))
        .expect("a chunked call is a known call");
}

#[test]
fn the_error_names_the_event_the_rule_and_the_id() {
    let (mut ctx, _events) = context();
    let error = ctx
        .emit(Event::text_message_content("m1", "hello"))
        .expect_err("m1 was never opened");
    let message = error.to_string();
    assert!(message.contains("TEXT_MESSAGE_CONTENT"), "{message}");
    assert!(message.contains("not-open"), "{message}");
    assert!(message.contains("\"m1\""), "{message}");
}

#[tokio::test]
async fn a_violation_in_a_real_run_becomes_run_error() {
    struct Sloppy;

    impl Agent for Sloppy {
        type State = ();

        async fn run(&self, ctx: &mut RunContext<()>) -> Result<RunOutcome> {
            ctx.emit(Event::text_message_content("m1", "no start"))?;
            Ok(RunOutcome::Success)
        }
    }

    let events: Vec<Event> = run(Sloppy, RunAgentInput::new("t", "r"))
        .map(|event| event.expect("the run stream should not break"))
        .collect()
        .await;

    let Event::RunError(error) = events.last().expect("a terminal event") else {
        panic!("expected RUN_ERROR, got {:?}", events.last());
    };
    assert_eq!(error.code.as_deref(), Some("PROTOCOL_VIOLATION"));
    assert_eq!(
        events.iter().map(Event::event_type).collect::<Vec<_>>(),
        [EventType::RunStarted, EventType::RunError]
    );
}

#[tokio::test]
async fn a_run_left_open_by_a_raw_emit_still_terminates() {
    struct LeavesOpen;

    impl Agent for LeavesOpen {
        type State = ();

        async fn run(&self, ctx: &mut RunContext<()>) -> Result<RunOutcome> {
            // A raw start with no handle to close it: RUN_FINISHED will be
            // rejected, and the driver must fall back to RUN_ERROR rather than
            // end the stream with nothing.
            ctx.emit(Event::text_message_start("m1", TextMessageRole::Assistant))?;
            Ok(RunOutcome::interrupt(vec![Interrupt::new("i", "why not")]))
        }
    }

    let events: Vec<Event> = run(LeavesOpen, RunAgentInput::new("t", "r"))
        .map(|event| event.expect("the run stream should not break"))
        .collect()
        .await;

    let Event::RunError(error) = events.last().expect("a terminal event") else {
        panic!("expected RUN_ERROR, got {:?}", events.last());
    };
    assert_eq!(error.code.as_deref(), Some("PROTOCOL_VIOLATION"));
    assert!(
        error.message.contains("open-at-finish"),
        "{}",
        error.message
    );
}
