//! Real socket tests exercise the example's public SDK integration.

use std::time::Duration;

use ag_ui::axum::RouterExt;
use ag_ui::client::{HttpAgent, RunEnd, SubmissionStatus};
use ag_ui::server::{Agent, Error, Result, RunContext};
use ag_ui::{Interrupt, ResumeEntry, RunOutcome};
use review_desk::ReviewState;
use serde_json::json;

#[tokio::test]
async fn review_workflow_uses_real_http_and_preserves_conversation_contracts() {
    let (url, server) = review_desk::serve_local().await.unwrap();
    let mut transcript = Vec::new();
    let result = tokio::time::timeout(
        Duration::from_secs(15),
        review_desk::client::workflow(&url, &mut transcript),
    )
    .await
    .expect("workflow including abort must terminate")
    .expect("successful real HTTP workflow");
    server.abort();

    let (evidence, snapshot) = result;
    assert_eq!(evidence.first.title, "release notes");
    assert_eq!(evidence.second.title, "broken dependency");
    assert_eq!(evidence.first, evidence.restored);
    assert_eq!(evidence.progress, [0, 50, 100]);
    assert_eq!(
        evidence.first.decisions,
        ["accept-review: approved", "publish-review: declined",]
    );
    assert!(
        evidence
            .subagents
            .iter()
            .any(|entry| entry.status == "failed")
    );
    assert!(
        evidence
            .subagents
            .iter()
            .any(|entry| entry.status == "finished")
    );
    assert!(
        evidence
            .subagents
            .iter()
            .all(|entry| !entry.invocation.is_empty())
    );
    assert!(
        evidence
            .messages
            .iter()
            .any(|message| message.speaker == "risk-check")
    );
    assert_eq!(evidence.surface_batches, 2);
    assert_eq!(evidence.generation_attempts, 2);
    assert_eq!(evidence.surface_data["title"], "release notes · reviewed");
    assert!(
        evidence
            .surface_data
            .as_object()
            .unwrap()
            .contains_key("memo")
    );
    assert!(evidence.surface_data["memo"].is_null());
    assert!(evidence.partial_approval_rejected);
    assert!(evidence.unique_message_ids);
    assert!(evidence.aborted);
    let saved: serde_json::Value = serde_json::from_slice(&snapshot).unwrap();
    assert!(saved.is_object());
    let transcript = String::from_utf8(transcript).unwrap();
    assert!(
        transcript.contains("partial answer rejected: true"),
        "{transcript}"
    );
    assert!(
        transcript.contains("Dependency fixture unavailable"),
        "{transcript}"
    );
}

#[tokio::test]
async fn a_thread_outlives_its_agent_and_unpolled_requests_leave_no_history() {
    let (url, server) = review_desk::serve_local().await.unwrap();
    let agent = HttpAgent::new(&url).unwrap();
    let mut thread = agent
        .thread_with_state("detached", ReviewState::default())
        .unwrap();
    drop(agent);

    drop(thread.send("review should never be sent").unwrap());
    assert!(thread.messages().is_empty());
    assert_eq!(thread.state().unwrap().revision, 0);
    let report = thread
        .send("review independently owned")
        .unwrap()
        .collect_report()
        .await;
    assert!(matches!(report.end, RunEnd::Success { .. }), "{report:?}");
    assert!(report.diagnostics.is_empty(), "{report:?}");
    assert_eq!(thread.state().unwrap().revision, 1);
    assert_eq!(thread.state().unwrap().title, "independently owned");
    server.abort();
}

#[tokio::test]
async fn application_validates_approval_payload_before_changing_domain_state() {
    let (url, server) = review_desk::serve_local().await.unwrap();
    let agent = HttpAgent::new(url).unwrap();
    let mut thread = agent
        .thread_with_state("invalid-answer", ReviewState::default())
        .unwrap();
    let report = thread.send("approve").unwrap().collect_report().await;
    assert!(matches!(report.end, RunEnd::Interrupted { .. }));
    let report = thread
        .resume_many([
            ResumeEntry::resolved("accept-review", json!({"approved": false})),
            ResumeEntry::cancelled("publish-review"),
        ])
        .unwrap()
        .collect_report()
        .await;
    assert!(matches!(report.end, RunEnd::Failed { .. }));
    assert!(thread.state().unwrap().decisions.is_empty());
    assert_eq!(thread.state().unwrap().revision, 0);
    server.abort();
}

#[derive(Debug)]
struct UncertainApproval;

impl Agent for UncertainApproval {
    type State = ();

    async fn run(&self, ctx: &mut RunContext<()>) -> Result<RunOutcome> {
        if ctx.resume_for("decision").is_some() {
            // The server has received the decision but cannot confirm the
            // domain operation. The client must not assume that it is safe
            // to resubmit this approval after reconnecting.
            return Err(Error::agent("lost operation receipt"));
        }
        Ok(RunOutcome::interrupt(vec![Interrupt::new(
            "decision",
            "tool_approval",
        )]))
    }
}

#[tokio::test]
async fn uncertain_approval_survives_disk_roundtrip_and_blocks_resubmission() {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}/agent", listener.local_addr().unwrap());
    let server = tokio::spawn(async move {
        axum::serve(
            listener,
            axum::Router::new().route_agui("/agent", UncertainApproval),
        )
        .await
        .unwrap();
    });
    let agent = HttpAgent::new(url).unwrap();
    let mut thread = agent.thread_with_state("uncertain", ()).unwrap();
    assert!(matches!(
        thread.send("approve").unwrap().collect_report().await.end,
        RunEnd::Interrupted { .. }
    ));
    let pending = thread.interrupts()[0].clone();
    let report = thread
        .resume(&pending, json!({"approved": true}))
        .unwrap()
        .collect_report()
        .await;
    assert!(matches!(report.end, RunEnd::Failed { .. }));
    assert_eq!(
        thread.submission().unwrap().status,
        SubmissionStatus::Unconfirmed
    );

    let bytes = serde_json::to_vec(&thread.snapshot()).unwrap();
    let mut restored = agent
        .restore_thread_with_state::<()>(serde_json::from_slice(&bytes).unwrap())
        .unwrap();
    assert_eq!(
        restored.submission().unwrap().status,
        SubmissionStatus::Unconfirmed
    );
    assert!(
        restored
            .resume(&pending, json!({"approved": true}))
            .is_err()
    );
    assert!(restored.send("continue").is_err());
    assert_eq!(restored.interrupts(), &[pending]);
    server.abort();
}

#[tokio::test]
async fn browser_stream_includes_live_events_and_sdk_assembled_card() {
    let (url, server) = review_desk::serve_local().await.unwrap();
    let base = url.trim_end_matches("/agent");
    let http = reqwest::Client::new();
    let page = http.get(base).send().await.unwrap().text().await.unwrap();
    assert!(page.contains("<title>Review Desk"));
    let response = http
        .get(format!("{base}/demo-events"))
        .send()
        .await
        .unwrap();
    assert!(
        response.headers()["content-type"]
            .to_str()
            .unwrap()
            .starts_with("text/event-stream")
    );
    let stream = tokio::time::timeout(Duration::from_secs(15), response.text())
        .await
        .unwrap()
        .unwrap();
    assert!(stream.contains("event: agui"), "{stream}");
    assert!(stream.contains("SUBAGENT_ERROR"), "{stream}");
    assert!(stream.contains("event: done"), "{stream}");
    assert!(stream.contains("release notes · reviewed"), "{stream}");
    assert!(
        stream.contains("\"partial_approval_rejected\":true"),
        "{stream}"
    );
    server.abort();
}
