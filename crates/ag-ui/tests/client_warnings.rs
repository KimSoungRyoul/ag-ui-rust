//! What the compatibility boundary removes or translates is reported to the
//! caller: as a `warning` diagnostic on the `Thread` run that saw it, or to the
//! callback of a standalone decoder.
#![cfg(feature = "client")]

use std::cell::RefCell;

use ag_ui::RunAgentInput;
use ag_ui::client::transport::{
    RawTransportFuture, Transport, TransportFuture, boxed_raw_stream, enforce_events,
};
use ag_ui::client::{Error, RunEnd, RunReport, Thread, Update};
use futures_util::{StreamExt, stream};
use serde_json::{Value, json};

/// Serves wire JSON as it arrived, so the boundary sees what a producer sent.
struct Wire(Vec<Value>);

impl Transport for Wire {
    fn run(&self, _: RunAgentInput) -> TransportFuture {
        panic!("a thread reads raw events")
    }

    fn run_raw(&self, _: RunAgentInput) -> RawTransportFuture {
        let events = self.0.clone();
        Box::pin(async move { Ok(boxed_raw_stream(stream::iter(events.into_iter().map(Ok)))) })
    }
}

fn started() -> Value {
    json!({"type": "RUN_STARTED", "threadId": "t", "runId": "r"})
}

fn finished() -> Value {
    json!({"type": "RUN_FINISHED", "threadId": "t", "runId": "r"})
}

fn thread(events: Vec<Value>, diagnostic_limit: usize) -> Thread<Wire> {
    let mut thread = Thread::builder(Wire(events), "t")
        .diagnostic_limit(diagnostic_limit)
        .build()
        .unwrap();
    thread.set_next_run_id("r");
    thread
}

async fn report(events: Vec<Value>) -> RunReport {
    thread(events, 100)
        .send("go")
        .unwrap()
        .collect_report()
        .await
}

fn warnings(report: &RunReport) -> Vec<&str> {
    report
        .diagnostics
        .iter()
        .filter(|diagnostic| diagnostic.kind == "warning")
        .map(|diagnostic| diagnostic.message.as_str())
        .collect()
}

#[tokio::test]
async fn stripped_material_is_reported_and_the_run_survives() {
    let report = report(vec![
        started(),
        json!({"type": "TEXT_MESSAGE_START", "messageId": "m1", "role": "assistant"}),
        json!({"type": "TEXT_MESSAGE_CONTENT", "messageId": "m1", "delta": "hi", "futureProp": 1}),
        json!({"type": "FUTURE_EVENT", "somethingNew": true}),
        json!({"type": "TEXT_MESSAGE_END", "messageId": "m1"}),
        json!({"type": "RUN_FINISHED", "threadId": "t", "runId": "r", "outcome": {"type": "paused"}}),
    ])
    .await;
    assert_eq!(report.end, RunEnd::Success { result: None });
    let warnings = warnings(&report);
    assert_eq!(warnings.len(), 3, "{warnings:?}");
    assert!(warnings[0].contains("/futureProp"), "{warnings:?}");
    assert!(warnings[1].contains("FUTURE_EVENT"), "{warnings:?}");
    assert!(warnings[2].contains("/outcome"), "{warnings:?}");
    assert_eq!(report.diagnostics.len(), 3, "{:?}", report.diagnostics);
}

#[tokio::test]
async fn a_translated_retired_event_is_reported() {
    let report = report(vec![
        started(),
        json!({"type": "THINKING_START", "title": "Working"}),
        json!({"type": "THINKING_TEXT_MESSAGE_START"}),
        json!({"type": "THINKING_TEXT_MESSAGE_CONTENT", "delta": "Let me check."}),
        json!({"type": "THINKING_TEXT_MESSAGE_END"}),
        json!({"type": "THINKING_END"}),
        finished(),
    ])
    .await;
    assert_eq!(report.end, RunEnd::Success { result: None });
    let warnings = warnings(&report);
    for tag in [
        "THINKING_START.title",
        "THINKING_START ",
        "THINKING_TEXT_MESSAGE_START",
        "THINKING_TEXT_MESSAGE_CONTENT",
        "THINKING_TEXT_MESSAGE_END",
        "THINKING_END",
    ] {
        assert!(
            warnings.iter().any(|warning| warning.contains(tag)),
            "{tag}: {warnings:?}"
        );
    }
}

#[tokio::test]
async fn a_newer_protocol_version_is_reported() {
    let report = report(vec![
        json!({"type": "RUN_STARTED", "threadId": "t", "runId": "r", "protocolVersion": "1.1"}),
        finished(),
    ])
    .await;
    assert_eq!(report.end, RunEnd::Success { result: None });
    let warnings = warnings(&report);
    assert_eq!(warnings.len(), 1, "{warnings:?}");
    assert!(warnings[0].contains("1.1"), "{warnings:?}");
}

#[tokio::test]
async fn a_registered_null_is_reported_once_per_run() {
    let report = report(vec![
        json!({"type": "RUN_STARTED", "threadId": "t", "runId": "r", "rawEvent": null}),
        json!({"type": "TEXT_MESSAGE_START", "messageId": "m1", "rawEvent": null}),
        json!({"type": "TEXT_MESSAGE_CONTENT", "messageId": "m1", "delta": "a", "rawEvent": null}),
        json!({"type": "TEXT_MESSAGE_END", "messageId": "m1", "rawEvent": null}),
        json!({"type": "RUN_FINISHED", "threadId": "t", "runId": "r", "result": null}),
    ])
    .await;
    assert_eq!(report.end, RunEnd::Success { result: None });
    let warnings = warnings(&report);
    assert_eq!(warnings.len(), 2, "{warnings:?}");
    assert!(warnings[0].contains("rawEvent"), "{warnings:?}");
    assert!(warnings[1].contains("result"), "{warnings:?}");
}

#[tokio::test]
async fn every_warning_is_an_update_and_the_report_keeps_the_retention_limit() {
    let mut thread = thread(
        vec![
            json!({"type": "RUN_STARTED", "threadId": "t", "runId": "r", "a": 1, "b": 2, "c": 3}),
            finished(),
        ],
        1,
    );
    let mut run = thread.send("go").unwrap();
    let mut warnings = Vec::new();
    while let Some(update) = run.next().await {
        match update {
            Update::Error(Error::Warning(warning)) => warnings.push(warning),
            Update::Error(error) => panic!("{error}"),
            Update::Done(end) => assert_eq!(end, RunEnd::Success { result: None }),
            _ => {}
        }
    }
    assert_eq!(
        warnings,
        [
            "unrecognised property at /a",
            "unrecognised property at /b",
            "unrecognised property at /c"
        ]
    );
    let report = run.collect_report().await;
    assert_eq!(report.diagnostics.len(), 1, "{:?}", report.diagnostics);
    assert_eq!(report.diagnostics[0].kind, "warning");
    assert_eq!(
        report.diagnostics[0].message,
        "protocol warning: unrecognised property at /a"
    );
    assert_eq!(report.diagnostics_omitted, 2);
}

#[tokio::test]
async fn standalone_decoding_hands_each_warning_to_a_callback_ahead_of_its_event() {
    let raw = stream::iter(
        [
            json!({"type": "RUN_STARTED", "threadId": "t", "runId": "r", "extra": 1}),
            json!({"type": "FUTURE_EVENT"}),
            finished(),
        ]
        .map(Ok),
    );
    let log = RefCell::new(Vec::new());
    let mut events = Box::pin(enforce_events(raw, |warning| {
        log.borrow_mut().push(warning)
    }));
    while let Some(event) = events.next().await {
        let tag = event.unwrap().event_type().as_str().to_owned();
        log.borrow_mut().push(tag);
    }
    drop(events);
    assert_eq!(
        log.into_inner(),
        [
            "unrecognised property at /extra",
            "RUN_STARTED",
            "unrecognised union member \"FUTURE_EVENT\" at /",
            "RUN_FINISHED"
        ]
    );
}

#[tokio::test]
async fn a_path_names_a_list_member_by_its_index_in_the_input() {
    // Two unknown operations are dropped ahead of a malformed known one; the
    // error still points at the third member the producer sent.
    let raw = stream::iter([Ok(json!({"type": "STATE_DELTA", "delta": [
        {"op": "frobnicate"}, {"op": "frobnicate"}, {"op": "add"}
    ]}))]);
    let mut warnings = Vec::new();
    let events: Vec<_> = enforce_events(raw, |warning| warnings.push(warning))
        .collect()
        .await;
    let [Err(error)] = events.as_slice() else {
        panic!("{events:?}");
    };
    assert!(error.to_string().contains("/delta/2 "), "{error}");
    assert_eq!(
        warnings,
        [
            "unrecognised union member \"frobnicate\" at /delta/0",
            "unrecognised union member \"frobnicate\" at /delta/1"
        ]
    );
}

#[tokio::test]
async fn a_warning_quotes_a_bounded_and_escaped_slice_of_the_input() {
    let long = "x".repeat(1 << 20);
    let mut event = started();
    event[&long] = json!(true);
    event["line\nbreak\u{1b}[31m"] = json!(true);
    let report = report(vec![event, finished()]).await;
    assert_eq!(report.end, RunEnd::Success { result: None });
    let warnings = warnings(&report);
    assert_eq!(warnings.len(), 2, "{warnings:?}");
    for warning in warnings {
        assert!(warning.len() < 256, "{} bytes", warning.len());
        assert!(!warning.contains(char::is_control), "{warning:?}");
    }
}
