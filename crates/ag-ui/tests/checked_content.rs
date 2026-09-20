use ag_ui::{Error, ReasoningMessageContentEvent, TextMessageContentEvent};

#[test]
fn checked_content_rejects_empty_deltas_without_changing_valid_payloads() {
    assert!(matches!(
        TextMessageContentEvent::try_new("m", ""),
        Err(Error::Protocol(_))
    ));
    assert!(matches!(
        ReasoningMessageContentEvent::try_new("r", ""),
        Err(Error::Protocol(_))
    ));
    assert_eq!(
        TextMessageContentEvent::try_new("m", "text").unwrap(),
        TextMessageContentEvent::new("m", "text")
    );
    assert_eq!(
        ReasoningMessageContentEvent::try_new("r", "reason").unwrap(),
        ReasoningMessageContentEvent::new("r", "reason")
    );
}

#[test]
fn lifecycle_constructors_preserve_omitted_results_and_codes() {
    use ag_ui::{Event, RunFinishedEvent, SubagentErrorEvent, SubagentFinishedEvent};
    use serde_json::json;
    assert_eq!(
        serde_json::to_value(Event::from(SubagentFinishedEvent::success("child", None))).unwrap(),
        json!({"type":"SUBAGENT_FINISHED","subagentRunId":"child","outcome":{"type":"success"}})
    );
    assert_eq!(
        serde_json::to_value(Event::from(SubagentFinishedEvent::suspended("child"))).unwrap(),
        json!({"type":"SUBAGENT_FINISHED","subagentRunId":"child","outcome":{"type":"suspended"}})
    );
    assert_eq!(
        serde_json::to_value(Event::from(SubagentErrorEvent::coded(
            "child", "failed", None
        )))
        .unwrap(),
        json!({"type":"SUBAGENT_ERROR","subagentRunId":"child","message":"failed"})
    );
    let event = RunFinishedEvent::interrupted(
        "thread",
        "run",
        vec![ag_ui::Interrupt::new("approval", "tool")],
    );
    assert_eq!(
        serde_json::to_value(Event::from(event)).unwrap(),
        json!({"type":"RUN_FINISHED","threadId":"thread","runId":"run","outcome":{"type":"interrupt","interrupts":[{"id":"approval","reason":"tool"}]}})
    );
}
