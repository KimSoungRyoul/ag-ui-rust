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
