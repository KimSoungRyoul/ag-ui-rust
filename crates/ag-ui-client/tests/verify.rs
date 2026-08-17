//! Protocol verification: what a malformed stream should say.

use ag_ui_client::transport::ReplayTransport;
use ag_ui_client::{Error, Session, Update, Verifier, verify_all};
use ag_ui_core::{Event, TextMessageRole};
use futures_util::StreamExt;
use serde_json::json;

/// Runs a stream through a verifier and returns the first complaint.
fn complaint(events: &[Event]) -> String {
    let mut verifier = Verifier::new();
    for event in events {
        if let Err(error) = verifier.verify(event) {
            assert!(matches!(error, Error::Protocol(_)), "unexpected: {error:?}");
            return error.to_string();
        }
    }
    panic!("expected a violation, but the stream verified");
}

#[test]
fn content_for_a_message_that_was_never_opened_is_rejected() {
    let said = complaint(&[
        Event::run_started("t", "r"),
        Event::text_message_content("msg-1", "orphan"),
    ]);
    assert!(said.contains("never opened"), "{said}");
    assert!(said.contains("msg-1"), "{said}");
}

#[test]
fn content_for_a_different_message_than_the_open_one_is_rejected() {
    let said = complaint(&[
        Event::run_started("t", "r"),
        Event::text_message_start("msg-1", TextMessageRole::Assistant),
        Event::text_message_content("msg-2", "wrong message"),
    ]);
    assert!(said.contains("msg-2"), "{said}");
    assert!(said.contains("msg-1"), "{said}");
}

#[test]
fn a_second_message_cannot_open_while_one_is_still_open() {
    let said = complaint(&[
        Event::run_started("t", "r"),
        Event::text_message_start("msg-1", TextMessageRole::Assistant),
        Event::text_message_start("msg-2", TextMessageRole::Assistant),
    ]);
    assert!(said.contains("still open"), "{said}");
}

#[test]
fn a_tool_call_cannot_open_inside_a_message() {
    let said = complaint(&[
        Event::run_started("t", "r"),
        Event::text_message_start("msg-1", TextMessageRole::Assistant),
        Event::tool_call_start("call-1", "t"),
    ]);
    assert!(said.contains("still open"), "{said}");
}

#[test]
fn tool_call_arguments_for_the_wrong_call_are_rejected() {
    let said = complaint(&[
        Event::run_started("t", "r"),
        Event::tool_call_start("call-1", "t"),
        Event::tool_call_args("call-2", "{}"),
    ]);
    assert!(said.contains("call-2"), "{said}");
}

#[test]
fn reasoning_content_needs_its_message_opened_first() {
    let said = complaint(&[
        Event::run_started("t", "r"),
        Event::reasoning_message_content("r-1", "thinking"),
    ]);
    assert!(said.contains("never opened"), "{said}");
}

#[test]
fn nothing_may_follow_a_finished_run() {
    let said = complaint(&[
        Event::run_started("t", "r"),
        Event::run_finished_success("t", "r"),
        Event::text_message_start("msg-1", TextMessageRole::Assistant),
    ]);
    assert!(
        said.contains("after the run had already finished"),
        "{said}"
    );

    let said = complaint(&[
        Event::run_started("t", "r"),
        Event::run_error("boom"),
        Event::run_finished_success("t", "r"),
    ]);
    assert!(
        said.contains("after the run had already finished"),
        "{said}"
    );
}

#[test]
fn a_run_starts_once() {
    let said = complaint(&[Event::run_started("t", "r"), Event::run_started("t", "r")]);
    assert!(said.contains("twice"), "{said}");
}

#[test]
fn the_stream_opens_with_run_started() {
    let said = complaint(&[Event::text_message_start(
        "msg-1",
        TextMessageRole::Assistant,
    )]);
    assert!(said.contains("before RUN_STARTED"), "{said}");
}

#[test]
fn raw_and_custom_events_are_outside_the_ordering_rules() {
    // Both are escape hatches by definition, including before the run starts.
    let mut verifier = Verifier::new();
    verifier
        .verify(&Event::custom("hello", json!(1)))
        .expect("custom is allowed anywhere");
    verifier
        .verify(&Event::raw(json!({ "provider": "openai" })))
        .expect("raw is allowed anywhere");
    verifier
        .verify(&Event::run_started("t", "r"))
        .expect("and the run still starts cleanly");
}

#[test]
fn a_run_may_not_finish_with_a_message_still_open() {
    let said = complaint(&[
        Event::run_started("t", "r"),
        Event::text_message_start("msg-1", TextMessageRole::Assistant),
        Event::run_finished_success("t", "r"),
    ]);
    assert!(said.contains("still open"), "{said}");
}

#[test]
fn a_run_may_not_finish_with_a_step_still_running() {
    let said = complaint(&[
        Event::run_started("t", "r"),
        Event::step_started("plan"),
        Event::run_finished_success("t", "r"),
    ]);
    assert!(said.contains("still running"), "{said}");
}

#[test]
fn a_failing_run_may_abandon_what_it_had_open() {
    // Failing is exactly the case where the agent cannot close things neatly.
    verify_all(&[
        Event::run_started("t", "r"),
        Event::text_message_start("msg-1", TextMessageRole::Assistant),
        Event::text_message_content("msg-1", "half a sen"),
        Event::run_error("the model went away"),
    ])
    .expect("a run error may leave things open");
}

#[test]
fn steps_must_match_their_names() {
    let said = complaint(&[Event::run_started("t", "r"), Event::step_finished("plan")]);
    assert!(said.contains("never started"), "{said}");

    let said = complaint(&[
        Event::run_started("t", "r"),
        Event::step_started("plan"),
        Event::step_started("plan"),
    ]);
    assert!(said.contains("already running"), "{said}");
}

#[test]
fn steps_may_nest_when_their_names_differ() {
    verify_all(&[
        Event::run_started("t", "r"),
        Event::step_started("outer"),
        Event::step_started("inner"),
        Event::step_finished("inner"),
        Event::step_finished("outer"),
        Event::run_finished_success("t", "r"),
    ])
    .expect("nested steps are fine");
}

#[test]
fn state_and_activity_events_may_interleave_with_a_message() {
    // Deliberately looser than the TypeScript verifier: a chunk-streaming
    // producer publishes state between two fragments of one message, and
    // rejecting that would make the verifier useless in practice.
    verify_all(&[
        Event::run_started("t", "r"),
        Event::text_message_start("msg-1", TextMessageRole::Assistant),
        Event::text_message_content("msg-1", "Hel"),
        Event::state_snapshot(json!({ "progress": 0.5 })),
        Event::step_started("mid-message"),
        Event::step_finished("mid-message"),
        Event::text_message_content("msg-1", "lo"),
        Event::text_message_end("msg-1"),
        Event::run_finished_success("t", "r"),
    ])
    .expect("state between fragments is legitimate");
}

#[test]
fn an_interrupt_outcome_with_no_interrupts_is_rejected() {
    let said = complaint(&[
        Event::run_started("t", "r"),
        Event::run_finished_interrupt("t", "r", Vec::new()),
    ]);
    assert!(said.contains("at least one interrupt"), "{said}");
}

#[test]
fn a_stream_that_stops_early_is_a_violation_of_its_own() {
    let mut verifier = Verifier::new();
    verifier.verify(&Event::run_started("t", "r")).expect("ok");
    let error = verifier.finish().expect_err("the run never finished");
    assert!(
        error.to_string().contains("ended before RUN_FINISHED"),
        "unexpected error: {error}"
    );

    // And a stream with no events at all never even started.
    let error = Verifier::new().finish().expect_err("nothing arrived");
    assert!(error.to_string().contains("ended before RUN_STARTED"));
}

#[test]
fn chunk_events_are_left_to_the_normalizer() {
    // The verifier runs after normalization, so chunks it does see are opaque
    // rather than violations.
    verify_all(&[
        Event::run_started("t", "r"),
        Event::text_message_chunk(Some("msg-1".into()), Some("hi".into())),
        Event::run_finished_success("t", "r"),
    ])
    .expect("chunks are not an ordering violation");
}

#[tokio::test]
async fn a_session_reports_a_malformed_stream_and_does_not_apply_it() {
    let transport = ReplayTransport::new([
        Event::run_started("thread-1", "run-1"),
        // No TEXT_MESSAGE_START: this content has nowhere to go.
        Event::text_message_content("msg-1", "orphan text"),
        Event::run_finished_success("thread-1", "run-1"),
    ]);
    let mut session = Session::<_>::new(transport, "thread-1");

    let updates: Vec<_> = session.send("hi").collect().await;
    let errors: Vec<String> = updates
        .iter()
        .filter_map(|update| match update {
            Update::Error(error) => Some(error.to_string()),
            _ => None,
        })
        .collect();

    assert_eq!(errors.len(), 1, "{errors:?}");
    assert!(errors[0].contains("never opened"), "{}", errors[0]);
    // Only the user's own message: the rejected event was not applied.
    assert_eq!(session.messages().len(), 1);
}

#[tokio::test]
async fn a_run_that_ends_untidily_still_ends() {
    // The producer forgot TEXT_MESSAGE_END. That is worth complaining about,
    // but the run is over: a caller waiting for `Done` must not wait forever,
    // and it must hear about it exactly once.
    let transport = ReplayTransport::new([
        Event::run_started("thread-1", "run-1"),
        Event::text_message_start("msg-1", TextMessageRole::Assistant),
        Event::text_message_content("msg-1", "Unterminated."),
        Event::run_finished_success("thread-1", "run-1"),
    ]);
    let mut session = Session::<_>::new(transport, "thread-1");

    let updates: Vec<_> = session.send("hi").collect().await;
    let errors: Vec<String> = updates
        .iter()
        .filter_map(|update| match update {
            Update::Error(error) => Some(error.to_string()),
            _ => None,
        })
        .collect();

    assert_eq!(errors.len(), 1, "{errors:?}");
    assert!(errors[0].contains("still open"), "{}", errors[0]);
    assert!(matches!(
        updates.last(),
        Some(Update::Done(ag_ui_client::RunEnd::Success { .. }))
    ));
    assert_eq!(session.applier().text_of("msg-1"), Some("Unterminated."));
}

#[tokio::test]
async fn verification_can_be_turned_off_for_a_producer_you_have_decided_to_live_with() {
    let transport = ReplayTransport::new([
        Event::run_started("thread-1", "run-1"),
        Event::text_message_content("msg-1", "orphan text"),
        Event::run_finished_success("thread-1", "run-1"),
    ]);
    let mut session = Session::<_>::builder(transport, "thread-1")
        .verify(false)
        .build();

    let updates: Vec<_> = session.send("hi").collect().await;
    assert!(
        !updates
            .iter()
            .any(|update| matches!(update, Update::Error(_)))
    );
    // The applier is tolerant, so the text still lands somewhere sensible.
    assert_eq!(session.applier().text_of("msg-1"), Some("orphan text"));
}
