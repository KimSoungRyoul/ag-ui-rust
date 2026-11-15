//! AG-UI 1.0 stream outcomes at the public Thread boundary.

#![cfg(feature = "client")]

use ag_ui::client::transport::ReplayTransport;
use ag_ui::client::{Error, RunEnd, SubmissionStatus, Thread, Update};
use ag_ui::{
    Event, Interrupt, Message, RunErrorEvent, RunFinishedEvent, RunOutcome, TextMessageRole,
    ToolCallId,
};
use futures_util::StreamExt as _;
use serde_json::{Value, json};

fn official_stream(text: &str) -> (String, Vec<Event>) {
    let fixture: Value = serde_json::from_str(text).expect("official fixture is JSON");
    let events: Vec<Event> =
        serde_json::from_value(fixture["stream"].clone()).expect("official events decode");
    let Event::RunStarted(first) = &events[0] else {
        panic!("official fixture starts a run");
    };
    (first.thread_id.as_str().to_owned(), events)
}

#[tokio::test]
async fn official_fatal_streams_end_the_thread_run_at_the_first_violation() {
    for (fixture, expected) in [
        (
            include_str!("fixtures/ag-ui-1.0/streams/first-chunk-missing-id-fatal.json"),
            "messageId",
        ),
        (
            include_str!("fixtures/ag-ui-1.0/streams/step-finished-without-start-fatal.json"),
            "never started",
        ),
        (
            include_str!("fixtures/ag-ui-1.0/streams/content-without-start-fatal.json"),
            "never opened",
        ),
    ] {
        let (thread_id, events) = official_stream(fixture);
        let mut thread = Thread::new(ReplayTransport::new(events).matching_requests(), thread_id);
        let updates: Vec<_> = thread.send("go").expect("request accepted").collect().await;
        assert!(
            matches!(updates.last(), Some(Update::Done(RunEnd::Failed { .. }))),
            "{expected}: {updates:?}"
        );
        let Some(Update::Error(error)) = updates.get(updates.len() - 2) else {
            panic!("{expected}: a fatal error must precede Done: {updates:?}");
        };
        assert!(error.to_string().contains(expected), "{error}");
        assert_eq!(
            thread.messages().len(),
            1,
            "a rejected event must not add a message"
        );
    }
}

#[tokio::test]
async fn a_stream_that_opens_with_run_error_ends_with_the_servers_failure() {
    // The upstream stream `run-error-first-run-continues`: a run that failed
    // before it began. The failure is the server's, message and code, and
    // nothing presents it as a protocol violation.
    let failed = Event::RunError(
        RunErrorEvent::new("run-error-first: the agent was unreachable").with_code("UNAVAILABLE"),
    );
    let mut thread = Thread::new(ReplayTransport::new([failed]).matching_requests(), "t");
    let report = thread.send("go").unwrap().collect_report().await;
    let expected = RunEnd::Failed {
        message: "run-error-first: the agent was unreachable".into(),
        code: Some("UNAVAILABLE".into()),
    };
    assert_eq!(report.end, expected);
    assert_eq!(thread.last_run_end(), Some(&expected));
    let kinds: Vec<_> = report.diagnostics.iter().map(|d| d.kind).collect();
    assert_eq!(kinds, ["run"], "{:?}", report.diagnostics);
}

#[tokio::test]
async fn a_run_error_after_run_finished_fails_the_run() {
    // The upstream stream `late-run-error-after-finished-run-continues`. The
    // thread reads past RUN_FINISHED to the end of the stream, verifying or
    // not, and a late RUN_ERROR turns the reported success into a failure.
    for verify in [true, false] {
        let transport = ReplayTransport::new([
            Event::run_started("t", "r"),
            Event::text_message_start("late-m-1", TextMessageRole::Assistant),
            Event::text_message_content("late-m-1", "delivered before the failure"),
            Event::text_message_end("late-m-1"),
            Event::run_finished_success("t", "r"),
            Event::run_error("late-run-error: the transport failed while flushing"),
        ])
        .matching_requests();
        let mut thread = Thread::<_>::builder(transport, "t")
            .verify(verify)
            .build()
            .unwrap();
        let updates: Vec<_> = thread.send("go").unwrap().collect().await;
        let expected = RunEnd::Failed {
            message: "late-run-error: the transport failed while flushing".into(),
            code: None,
        };
        assert!(
            matches!(updates.last(), Some(Update::Done(end)) if *end == expected),
            "verify {verify}: {updates:?}"
        );
        let errors: Vec<_> = updates
            .iter()
            .filter_map(|update| match update {
                Update::Error(error) => Some(error),
                _ => None,
            })
            .collect();
        assert!(
            matches!(errors[..], [Error::Run { .. }]),
            "verify {verify}: {errors:?}"
        );
        assert_eq!(thread.last_run_end(), Some(&expected));
        // Everything delivered before the failure stays delivered.
        assert_eq!(
            thread.applier().text_of("late-m-1"),
            Some("delivered before the failure")
        );
    }
}

#[tokio::test]
async fn a_later_run_on_the_stream_is_left_unread_without_failing_the_first() {
    // 1.0 lets RUN_STARTED follow a closed run. Until a thread reads several
    // runs (#44), the first run keeps the outcome it reported, and the
    // diagnostic says the rest was not read — not that it was malformed.
    for verify in [true, false] {
        let transport = ReplayTransport::new([
            Event::run_started("t", "r"),
            Event::run_finished_success("t", "r"),
            Event::run_started("t", "r-later"),
            Event::text_message_start("later-m-1", TextMessageRole::Assistant),
            Event::text_message_end("later-m-1"),
            Event::run_finished_success("t", "r-later"),
        ])
        .matching_requests();
        let mut thread = Thread::<_>::builder(transport, "t")
            .verify(verify)
            .build()
            .unwrap();
        let report = thread.send("go").unwrap().collect_report().await;
        assert_eq!(
            report.end,
            RunEnd::Success { result: None },
            "verify {verify}"
        );
        assert_eq!(report.diagnostics.len(), 1, "verify {verify}: {report:?}");
        assert_eq!(report.diagnostics[0].kind, "unsupported");
        assert!(
            report.diagnostics[0].message.contains("later run"),
            "{report:?}"
        );
        assert_eq!(thread.messages().len(), 1, "the later run is not applied");
    }
}

#[tokio::test]
async fn a_late_run_error_withdraws_the_outcome_it_follows() {
    // The interrupts a RUN_FINISHED reported belong to a run that then failed:
    // nothing is pending, and nothing is announced.
    let transport = ReplayTransport::new([
        Event::run_started("t", "r"),
        Event::run_finished_interrupt("t", "r", vec![Interrupt::new("approval", "approval")]),
        Event::run_error("failed after pausing"),
    ])
    .matching_requests();
    let mut thread = Thread::new(transport, "t");
    let updates: Vec<_> = thread.send("go").unwrap().collect().await;
    assert!(
        !updates
            .iter()
            .any(|update| matches!(update, Update::Interrupt(_))),
        "{updates:?}"
    );
    assert!(
        matches!(updates.last(), Some(Update::Done(RunEnd::Failed { .. }))),
        "{updates:?}"
    );
    assert!(thread.interrupts().is_empty());
    assert!(thread.applier().interrupts().is_empty());
}

#[tokio::test]
async fn events_outside_the_run_fail_the_thread_run() {
    // Nothing but RUN_STARTED or RUN_ERROR may open a stream, RAW and CUSTOM
    // included; nothing but RUN_ERROR may follow RUN_FINISHED — the upstream
    // stream `event-after-run-finished-fatal`.
    for (events, expected) in [
        (
            vec![
                Event::raw(json!({ "provider": "openai" })),
                Event::run_started("t", "r"),
                Event::run_finished_success("t", "r"),
            ],
            "before RUN_STARTED",
        ),
        (
            vec![
                Event::custom("early", json!(1)),
                Event::run_started("t", "r"),
                Event::run_finished_success("t", "r"),
            ],
            "before RUN_STARTED",
        ),
        (
            vec![
                Event::run_started("t", "r"),
                Event::run_finished_success("t", "r"),
                Event::text_message_start("after-m-1", TextMessageRole::Assistant),
            ],
            "already finished",
        ),
    ] {
        let mut thread = Thread::new(ReplayTransport::new(events).matching_requests(), "t");
        let report = thread.send("go").unwrap().collect_report().await;
        assert!(
            matches!(report.end, RunEnd::Failed { .. }),
            "{expected}: {report:?}"
        );
        assert_eq!(report.diagnostics.len(), 1, "{expected}: {report:?}");
        assert_eq!(report.diagnostics[0].kind, "protocol");
        assert!(
            report.diagnostics[0].message.contains(expected),
            "{expected}: {report:?}"
        );
        assert_eq!(
            thread.messages().len(),
            1,
            "a rejected event must not add a message"
        );
    }
}

#[tokio::test]
async fn official_unappliable_state_delta_is_a_diagnostic_and_keeps_the_prior_state() {
    let (thread_id, events) = official_stream(include_str!(
        "fixtures/ag-ui-1.0/streams/state-delta-unappliable-warns-and-keeps.json"
    ));
    let mut thread = Thread::new(ReplayTransport::new(events).matching_requests(), thread_id);
    let report = thread.send("go").unwrap().collect_report().await;
    assert_eq!(report.end, RunEnd::Success { result: None });
    assert_eq!(report.diagnostics.len(), 1);
    assert_eq!(report.diagnostics[0].kind, "patch");
    assert_eq!(thread.raw_state(), &json!({"plan": {"status": "open"}}));
}

#[tokio::test]
async fn an_official_malformed_patch_pointer_fails_the_thread_run() {
    let invalid: Event = serde_json::from_str(include_str!(
        "fixtures/ag-ui-1.0/fixtures/StateDeltaEvent/invalid/patch-pointer-without-leading-slash.json"
    ))
    .expect("the loose Rust patch type can represent the malformed pointer");
    let transport = ReplayTransport::new([
        Event::run_started("t", "r"),
        invalid,
        Event::run_finished_success("t", "r"),
    ])
    .matching_requests();
    let mut thread = Thread::new(transport, "t");
    let report = thread.send("go").unwrap().collect_report().await;
    assert!(matches!(report.end, RunEnd::Failed { .. }));
    assert_eq!(report.diagnostics.len(), 1);
    assert_eq!(report.diagnostics[0].kind, "protocol");
    assert_eq!(thread.raw_state(), &json!({}));
}

#[tokio::test]
async fn official_remote_cancelled_outcome_is_distinct_from_local_abort() {
    let (thread_id, events) = official_stream(include_str!(
        "fixtures/ag-ui-1.0/streams/run-finished-cancelled-outcome-accepted.json"
    ));
    let mut thread = Thread::new(ReplayTransport::new(events).matching_requests(), thread_id);
    let report = thread.send("go").unwrap().collect_report().await;
    assert_eq!(report.end, RunEnd::Cancelled);
    assert!(report.diagnostics.is_empty());
    assert_eq!(thread.last_run_end(), Some(&RunEnd::Cancelled));
    assert_eq!(
        thread.applier().text_of("run-finished-cancelled-m-1"),
        Some("delivered before the stop")
    );
}

#[tokio::test]
async fn remote_cancellation_does_not_confirm_a_submitted_interrupt_response() {
    let question = Interrupt::new("approval", "tool_approval");
    let cancelled =
        Event::RunFinished(RunFinishedEvent::new("t", "r").with_outcome(RunOutcome::Cancelled));
    let transport = ReplayTransport::with_runs([
        vec![
            Event::run_started("t", "r"),
            Event::run_finished_interrupt("t", "r", vec![question.clone()]),
        ],
        vec![Event::run_started("t", "r"), cancelled],
    ])
    .matching_requests();
    let mut thread = Thread::new(transport, "t");
    thread.send("go").unwrap().collect_report().await;
    let pending = thread.interrupts()[0].clone();
    let report = thread
        .resume(&pending, true)
        .unwrap()
        .collect_report()
        .await;
    assert_eq!(report.end, RunEnd::Cancelled);
    assert!(report.diagnostics.is_empty());
    assert_eq!(thread.interrupts(), &[question]);
    assert_eq!(
        thread
            .submission()
            .expect("response stays unconfirmed")
            .status,
        SubmissionStatus::Unconfirmed
    );
}

#[tokio::test]
async fn pending_frontend_tool_calls_are_visible_in_the_run_report_and_thread() {
    let finished = Event::RunFinished(
        RunFinishedEvent::new("t", "r")
            .with_outcome(RunOutcome::success_with_pending_tool_calls(["call-1"])),
    );
    let transport = ReplayTransport::new([
        Event::run_started("t", "r"),
        Event::tool_call_start("call-1", "lookup"),
        Event::tool_call_args("call-1", "{}"),
        Event::tool_call_end("call-1"),
        finished,
    ])
    .matching_requests();
    let mut thread = Thread::new(transport, "t");
    let report = thread.send("go").unwrap().collect_report().await;
    let expected = RunEnd::SuccessWithPendingToolCalls {
        result: None,
        pending_tool_call_ids: vec![ToolCallId::new("call-1")],
    };
    assert_eq!(report.end, expected);
    assert_eq!(thread.last_run_end(), Some(&expected));
    assert!(report.diagnostics.is_empty());
}

#[tokio::test]
async fn every_declared_frontend_call_must_be_answered_before_the_next_run() {
    let first = vec![
        Event::run_started("t", "r1"),
        Event::tool_call_start("call-1", "lookup"),
        Event::tool_call_end("call-1"),
        Event::tool_call_start("call-2", "lookup"),
        Event::tool_call_end("call-2"),
        Event::RunFinished(RunFinishedEvent::new("t", "r1").with_outcome(
            RunOutcome::success_with_pending_tool_calls(["call-1", "call-2"]),
        )),
    ];
    let transport = ReplayTransport::with_runs([
        first,
        vec![
            Event::run_started("t", "r2"),
            Event::run_finished_success("t", "r2"),
        ],
    ])
    .matching_requests();
    let inspect = transport.clone();
    let mut thread = Thread::new(transport, "t");
    assert!(matches!(
        thread.send("first").unwrap().collect_report().await.end,
        RunEnd::SuccessWithPendingToolCalls { .. }
    ));

    let before = thread.messages().len();
    let error = thread.send("too soon").expect_err("calls need answers");
    assert!(error.to_string().contains("call-1"), "{error}");
    assert!(error.to_string().contains("call-2"), "{error}");
    assert_eq!(thread.messages().len(), before);
    assert_eq!(inspect.requests().len(), 1);

    thread
        .push_message(Message::tool("answer-1", "call-1", "done"))
        .unwrap();
    let error = thread
        .send("still too soon")
        .expect_err("second call is pending");
    assert!(error.to_string().contains("call-2"), "{error}");
    assert_eq!(inspect.requests().len(), 1);

    thread
        .push_message(Message::tool("answer-2", "call-2", "done"))
        .unwrap();
    assert_eq!(
        thread.send("continue").unwrap().collect_report().await.end,
        RunEnd::Success { result: None }
    );
    let requests = inspect.requests();
    assert_eq!(requests.len(), 2);
    assert!(matches!(
        requests[1].messages.last(),
        Some(Message::User(_))
    ));
    assert!(requests[1].messages.iter().any(
        |message| matches!(message, Message::Tool(tool) if tool.tool_call_id.as_str() == "call-1")
    ));
    assert!(requests[1].messages.iter().any(
        |message| matches!(message, Message::Tool(tool) if tool.tool_call_id.as_str() == "call-2")
    ));
}

#[tokio::test]
async fn unanswered_calls_are_derived_when_success_omits_pending_ids() {
    let transport = ReplayTransport::with_runs([
        vec![
            Event::run_started("t", "r1"),
            Event::tool_call_start("call-1", "lookup"),
            Event::tool_call_end("call-1"),
            Event::run_finished_success("t", "r1"),
        ],
        vec![
            Event::run_started("t", "r2"),
            Event::run_finished_success("t", "r2"),
        ],
    ])
    .matching_requests();
    let inspect = transport.clone();
    let mut thread = Thread::new(transport, "t");
    assert_eq!(
        thread.send("first").unwrap().collect_report().await.end,
        RunEnd::Success { result: None }
    );
    let error = thread.run().expect_err("pending call must be derived");
    assert!(error.to_string().contains("call-1"), "{error}");
    assert_eq!(inspect.requests().len(), 1);

    thread
        .push_message(Message::tool("answer-1", "call-1", "done"))
        .unwrap();
    assert_eq!(
        thread.run().unwrap().collect_report().await.end,
        RunEnd::Success { result: None }
    );
    assert_eq!(inspect.requests().len(), 2);
}

#[tokio::test]
async fn a_server_tool_result_already_answers_its_call() {
    let transport = ReplayTransport::with_runs([
        vec![
            Event::run_started("t", "r1"),
            Event::tool_call_start("call-1", "lookup"),
            Event::tool_call_end("call-1"),
            Event::tool_call_result("result-1", "call-1", "done"),
            Event::run_finished_success("t", "r1"),
        ],
        vec![
            Event::run_started("t", "r2"),
            Event::run_finished_success("t", "r2"),
        ],
    ])
    .matching_requests();
    let mut thread = Thread::new(transport, "t");
    assert_eq!(
        thread.run().unwrap().collect_report().await.end,
        RunEnd::Success { result: None }
    );
    assert_eq!(
        thread.run().unwrap().collect_report().await.end,
        RunEnd::Success { result: None }
    );
}

#[tokio::test]
async fn send_message_can_answer_the_last_pending_tool_without_early_mutation() {
    let transport = ReplayTransport::with_runs([
        vec![
            Event::run_started("t", "r1"),
            Event::tool_call_start("call-1", "lookup"),
            Event::tool_call_end("call-1"),
            Event::run_finished_success("t", "r1"),
        ],
        vec![
            Event::run_started("t", "r2"),
            Event::run_finished_success("t", "r2"),
        ],
    ])
    .matching_requests();
    let inspect = transport.clone();
    let mut thread = Thread::new(transport, "t");
    thread.run().unwrap().collect_report().await;
    let before = thread.messages().to_vec();

    // Preparing, then abandoning, the answer must leave both the transcript
    // and the unanswered-call guard intact.
    drop(
        thread
            .send_message(Message::tool("answer", "call-1", "done"))
            .unwrap(),
    );
    assert_eq!(thread.messages(), before);
    assert_eq!(inspect.requests().len(), 1);
    assert!(thread.run().is_err());

    let report = thread
        .send_message(Message::tool("answer", "call-1", "done"))
        .unwrap()
        .collect_report()
        .await;
    assert_eq!(report.end, RunEnd::Success { result: None });
    let requests = inspect.requests();
    assert_eq!(requests.len(), 2);
    assert_eq!(
        requests[1].messages.last(),
        Some(&Message::tool("answer", "call-1", "done"))
    );
}

#[tokio::test]
async fn send_message_still_rejects_other_pending_tools_without_mutation_or_dispatch() {
    let transport = ReplayTransport::new([
        Event::run_started("t", "r1"),
        Event::tool_call_start("call-1", "lookup"),
        Event::tool_call_end("call-1"),
        Event::tool_call_start("call-2", "lookup"),
        Event::tool_call_end("call-2"),
        Event::run_finished_success("t", "r1"),
    ])
    .matching_requests();
    let inspect = transport.clone();
    let mut thread = Thread::new(transport, "t");
    thread.run().unwrap().collect_report().await;
    let before = thread.messages().to_vec();
    let error = thread
        .send_message(Message::tool("answer", "call-1", "done"))
        .unwrap_err();
    assert!(error.to_string().contains("call-2"), "{error}");
    assert!(!error.to_string().contains("call-1"), "{error}");
    assert_eq!(thread.messages(), before);
    assert_eq!(inspect.requests().len(), 1);
}
