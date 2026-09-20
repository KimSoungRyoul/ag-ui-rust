#![cfg(all(feature = "server", feature = "verify"))]

use ag_ui::server::EventVerifier;
use ag_ui::{Event, TextMessageRole};

#[test]
fn host_queries_follow_stream_lifecycle_without_publishing_events() {
    let mut verifier = EventVerifier::new();
    verifier
        .observe(&Event::run_started("thread", "run"))
        .unwrap();
    assert_eq!(verifier.primary_open(), None);
    verifier
        .observe(&Event::text_message_start(
            "text",
            TextMessageRole::Assistant,
        ))
        .unwrap();
    assert_eq!(
        verifier.primary_open().as_deref(),
        Some("TEXT_MESSAGE(text)")
    );
    verifier.observe(&Event::text_message_end("text")).unwrap();
    verifier
        .observe(&Event::reasoning_start("reasoning"))
        .unwrap();
    verifier
        .observe(&Event::reasoning_message_start("reasoning"))
        .unwrap();
    assert_eq!(
        verifier.primary_open().as_deref(),
        Some("REASONING(reasoning)")
    );
    assert_eq!(
        verifier.reasoning_message_open().as_deref(),
        Some("REASONING_MESSAGE(reasoning)")
    );
    verifier
        .observe(&Event::reasoning_message_end("reasoning"))
        .unwrap();
    assert_eq!(verifier.reasoning_message_open(), None);
    verifier
        .observe(&Event::reasoning_end("reasoning"))
        .unwrap();
    verifier
        .observe(&Event::tool_call_start("call", "lookup"))
        .unwrap();
    assert_eq!(verifier.primary_open().as_deref(), Some("TOOL_CALL(call)"));
    verifier.observe(&Event::tool_call_end("call")).unwrap();
    assert_eq!(verifier.primary_open(), None);
    verifier
        .observe(&Event::run_finished("thread", "run"))
        .unwrap();
}

#[test]
fn active_subagent_queries_keep_announcement_order_after_partial_closure() {
    let mut verifier = EventVerifier::new();
    verifier
        .observe(&Event::run_started("thread", "run"))
        .unwrap();
    for id in ["z", "a", "m"] {
        verifier
            .observe(&Event::from(ag_ui::SubagentStartedEvent::new(id, id)))
            .unwrap();
    }
    assert!(verifier.subagent_open("a"));
    assert!(!verifier.subagent_closed("a"));
    verifier
        .observe(&Event::from(ag_ui::SubagentFinishedEvent::new("a")))
        .unwrap();
    assert!(!verifier.subagent_open("a"));
    assert!(verifier.subagent_closed("a"));
    verifier
        .observe(&Event::from(ag_ui::SubagentStartedEvent::new("b", "b")))
        .unwrap();
    let ids: Vec<_> = verifier
        .ordered_subagents()
        .into_iter()
        .map(|id| id.to_string())
        .collect();
    assert_eq!(ids, ["z", "m", "b"]);
    assert!(
        verifier
            .observe(&Event::from(ag_ui::SubagentStartedEvent::new("a", "a")))
            .is_err()
    );
    for id in ["z", "m", "b"] {
        verifier
            .observe(&Event::from(ag_ui::SubagentFinishedEvent::new(id)))
            .unwrap();
    }
    assert!(verifier.ordered_subagents().is_empty());
    verifier
        .observe(&Event::run_finished("thread", "run"))
        .unwrap();
}
