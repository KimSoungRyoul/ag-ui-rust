use ag_ui::{Event, EventEnvelope, JsonObject};
use serde_json::json;

fn object(value: serde_json::Value) -> JsonObject {
    value.as_object().unwrap().clone()
}

#[test]
fn flat_extensions_preserve_protocol_identity() {
    let protocol = Event::run_started("thread", "run");
    let envelope = EventEnvelope::new(protocol.clone())
        .with_extensions(object(json!({"hostProof": {"run": "run"}})))
        .unwrap();
    assert_eq!(envelope.protocol(), &protocol);
    let wire = serde_json::to_value(&envelope).unwrap();
    assert_eq!(
        wire,
        json!({"type":"RUN_STARTED", "threadId":"thread", "runId":"run", "hostProof":{"run":"run"}})
    );
    let restored: EventEnvelope = serde_json::from_value(wire).unwrap();
    assert_eq!(restored, envelope);
}

#[test]
fn absent_standard_fields_cannot_be_injected_as_extensions() {
    for key in [
        "type",
        "runId",
        "subagentRunId",
        "parentRunId",
        "metadata",
        "rawEvent",
        "timestamp",
        "replace",
        "role",
        "messages",
        "usage",
    ] {
        let event = EventEnvelope::new(Event::run_started("thread", "run"));
        assert!(
            event.with_extensions(object(json!({key: null}))).is_err(),
            "{key}"
        );
    }
}

#[test]
fn envelope_rejects_invalid_standard_input() {
    assert!(
        serde_json::from_value::<EventEnvelope>(
            json!({"type":"RUN_STARTED", "threadId":"thread", "hostProof":true})
        )
        .is_err()
    );
}

#[test]
fn no_extension_serializes_identically_to_native_event() {
    let protocol = Event::text_message_content("message", "line\nnext");
    assert_eq!(
        serde_json::to_string(&protocol).unwrap(),
        serde_json::to_string(&EventEnvelope::new(protocol)).unwrap()
    );
}
