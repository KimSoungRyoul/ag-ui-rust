//! Chunk normalization, and the edge cases that make it fiddly.

use ag_ui_client::apply::Applier;
use ag_ui_client::{ChunkNormalizer, normalize_all, verify_all};
use ag_ui_core::{
    Event, EventType, MessageId, TextMessageChunkEvent, TextMessageRole, ToolCallChunkEvent,
    ToolCallId,
};

fn types(events: &[Event]) -> Vec<EventType> {
    events.iter().map(Event::event_type).collect()
}

fn text_chunk(id: Option<&str>, delta: Option<&str>) -> Event {
    Event::text_message_chunk(
        id.map(MessageId::new),
        delta.map(std::borrow::ToOwned::to_owned),
    )
}

fn tool_chunk(id: Option<&str>, name: Option<&str>, delta: Option<&str>) -> Event {
    Event::tool_call_chunk(
        id.map(ToolCallId::new),
        name.map(std::borrow::ToOwned::to_owned),
        delta.map(std::borrow::ToOwned::to_owned),
    )
}

#[test]
fn a_chunk_stream_that_never_terminates_is_closed_at_the_end_of_the_stream() {
    // The last message of a run has nothing after it to imply its end, so the
    // end of the transport stream is what closes it.
    let events = normalize_all([
        text_chunk(Some("msg-1"), Some("Hel")),
        text_chunk(None, Some("lo")),
        text_chunk(None, Some("!")),
    ])
    .expect("normalizes");

    assert_eq!(
        types(&events),
        [
            EventType::TextMessageStart,
            EventType::TextMessageContent,
            EventType::TextMessageContent,
            EventType::TextMessageContent,
            EventType::TextMessageEnd,
        ]
    );

    let mut applier = Applier::new();
    for event in &events {
        applier.apply(event).expect("applies");
    }
    assert_eq!(applier.text_of("msg-1"), Some("Hello!"));
}

#[test]
fn a_chunk_with_no_preceding_id_is_a_protocol_error() {
    let error = normalize_all([text_chunk(None, Some("orphan"))])
        .expect_err("there is no message to attach the text to");
    assert!(
        error.to_string().contains("no messageId"),
        "unexpected error: {error}"
    );

    let error = normalize_all([tool_chunk(None, None, Some("{}"))])
        .expect_err("there is no call to attach the arguments to");
    assert!(
        error.to_string().contains("no toolCallId"),
        "unexpected error: {error}"
    );
}

#[test]
fn a_tool_call_chunk_that_opens_a_call_must_name_the_tool() {
    // Without a name there is nothing to put in TOOL_CALL_START, and inventing
    // one would silently call the wrong tool.
    let error = normalize_all([tool_chunk(Some("call-1"), None, Some("{}"))])
        .expect_err("a call has to be named");
    assert!(
        error.to_string().contains("without a toolCallName"),
        "unexpected error: {error}"
    );
}

#[test]
fn interleaved_chunk_streams_close_each_other() {
    // Two messages, alternating. Each switch ends the message before it — that
    // is the only signal the format gives.
    let events = normalize_all([
        text_chunk(Some("msg-1"), Some("a1")),
        text_chunk(Some("msg-2"), Some("b1")),
        text_chunk(None, Some("b2")),
        text_chunk(Some("msg-1"), Some("a2")),
    ])
    .expect("normalizes");

    assert_eq!(
        types(&events),
        [
            EventType::TextMessageStart,   // msg-1
            EventType::TextMessageContent, // a1
            EventType::TextMessageEnd,     // msg-1
            EventType::TextMessageStart,   // msg-2
            EventType::TextMessageContent, // b1
            EventType::TextMessageContent, // b2 — bare chunk continues msg-2
            EventType::TextMessageEnd,     // msg-2
            EventType::TextMessageStart,   // msg-1 again
            EventType::TextMessageContent, // a2
            EventType::TextMessageEnd,     // msg-1
        ]
    );

    // Re-opening a known id appends rather than duplicating the message.
    let mut applier = Applier::new();
    for event in &events {
        applier.apply(event).expect("applies");
    }
    assert_eq!(applier.messages().len(), 2);
    assert_eq!(applier.text_of("msg-1"), Some("a1a2"));
    assert_eq!(applier.text_of("msg-2"), Some("b1b2"));
}

#[test]
fn a_tool_call_chunk_closes_an_open_text_chunk_stream() {
    let events = normalize_all([
        text_chunk(Some("msg-1"), Some("Checking")),
        tool_chunk(Some("call-1"), Some("get_weather"), Some(r#"{"city":"#)),
        tool_chunk(None, None, Some(r#""Seoul"}"#)),
    ])
    .expect("normalizes");

    assert_eq!(
        types(&events),
        [
            EventType::TextMessageStart,
            EventType::TextMessageContent,
            EventType::TextMessageEnd,
            EventType::ToolCallStart,
            EventType::ToolCallArgs,
            EventType::ToolCallArgs,
            EventType::ToolCallEnd,
        ]
    );

    // And what comes out is a stream the verifier accepts.
    let mut run = vec![Event::run_started("thread-1", "run-1")];
    run.extend(events);
    run.push(Event::run_finished_success("thread-1", "run-1"));
    verify_all(&run).expect("normalized chunks should be a valid stream");
}

#[test]
fn the_run_ending_closes_an_open_chunk_stream_before_it() {
    let events = normalize_all([
        Event::run_started("thread-1", "run-1"),
        text_chunk(Some("msg-1"), Some("Done.")),
        Event::run_finished_success("thread-1", "run-1"),
    ])
    .expect("normalizes");

    assert_eq!(
        types(&events),
        [
            EventType::RunStarted,
            EventType::TextMessageStart,
            EventType::TextMessageContent,
            EventType::TextMessageEnd,
            EventType::RunFinished,
        ]
    );
}

#[test]
fn an_explicit_end_is_not_duplicated() {
    // A producer that opens with a chunk and closes explicitly gets exactly one
    // TEXT_MESSAGE_END.
    let events = normalize_all([
        text_chunk(Some("msg-1"), Some("Hi")),
        Event::text_message_end("msg-1"),
    ])
    .expect("normalizes");

    assert_eq!(
        types(&events),
        [
            EventType::TextMessageStart,
            EventType::TextMessageContent,
            EventType::TextMessageEnd,
        ]
    );
}

#[test]
fn an_explicit_message_absorbs_a_following_bare_chunk() {
    let events = normalize_all([
        Event::text_message_start("msg-1", TextMessageRole::Assistant),
        Event::text_message_content("msg-1", "Hel"),
        text_chunk(None, Some("lo")),
        Event::text_message_end("msg-1"),
    ])
    .expect("normalizes");

    assert_eq!(
        types(&events),
        [
            EventType::TextMessageStart,
            EventType::TextMessageContent,
            EventType::TextMessageContent,
            EventType::TextMessageEnd,
        ]
    );

    let mut applier = Applier::new();
    for event in &events {
        applier.apply(event).expect("applies");
    }
    assert_eq!(applier.text_of("msg-1"), Some("Hello"));
}

#[test]
fn events_between_chunks_do_not_split_the_message() {
    // A producer that streams chunks may well publish state between two
    // fragments of one message. Ending the message there would be wrong.
    let events = normalize_all([
        text_chunk(Some("msg-1"), Some("Hel")),
        Event::state_snapshot(serde_json::json!({ "progress": 0.5 })),
        text_chunk(None, Some("lo")),
    ])
    .expect("normalizes");

    assert_eq!(
        types(&events),
        [
            EventType::TextMessageStart,
            EventType::TextMessageContent,
            EventType::StateSnapshot,
            EventType::TextMessageContent,
            EventType::TextMessageEnd,
        ]
    );
}

#[test]
fn a_chunk_carries_its_role_and_name_onto_the_synthesized_start() {
    let chunk = Event::TextMessageChunk(TextMessageChunkEvent {
        message_id: Some(MessageId::new("msg-1")),
        role: Some(TextMessageRole::User),
        delta: Some("typed by a human".into()),
        name: Some("ada".into()),
        ..TextMessageChunkEvent::default()
    });

    let events = normalize_all([chunk]).expect("normalizes");
    let Event::TextMessageStart(start) = &events[0] else {
        panic!("expected a start event");
    };
    assert_eq!(start.role, TextMessageRole::User);
    assert_eq!(start.name.as_deref(), Some("ada"));

    let mut applier = Applier::new();
    for event in &events {
        applier.apply(event).expect("applies");
    }
    assert_eq!(applier.messages()[0].role(), ag_ui_core::Role::User);
}

#[test]
fn a_tool_chunk_carries_its_parent_message_onto_the_synthesized_start() {
    let chunk = Event::ToolCallChunk(ToolCallChunkEvent {
        tool_call_id: Some(ToolCallId::new("call-1")),
        tool_call_name: Some("get_weather".into()),
        parent_message_id: Some(MessageId::new("msg-1")),
        delta: Some("{}".into()),
        ..ToolCallChunkEvent::default()
    });

    let events = normalize_all([chunk]).expect("normalizes");
    let Event::ToolCallStart(start) = &events[0] else {
        panic!("expected a tool call start");
    };
    assert_eq!(
        start.parent_message_id.as_ref().map(|id| id.as_str()),
        Some("msg-1")
    );
}

#[test]
fn reasoning_chunks_normalize_the_same_way() {
    let events = normalize_all([
        Event::reasoning_message_chunk(Some(MessageId::new("r-1")), Some("Think".into())),
        Event::reasoning_message_chunk(None, Some("ing".into())),
    ])
    .expect("normalizes");

    assert_eq!(
        types(&events),
        [
            EventType::ReasoningMessageStart,
            EventType::ReasoningMessageContent,
            EventType::ReasoningMessageContent,
            EventType::ReasoningMessageEnd,
        ]
    );

    let mut applier = Applier::new();
    for event in &events {
        applier.apply(event).expect("applies");
    }
    assert_eq!(applier.reasoning_text(&"r-1".into()), Some("Thinking"));
}

#[test]
fn a_chunk_with_no_delta_still_opens_its_message() {
    let events = normalize_all([text_chunk(Some("msg-1"), None)]).expect("normalizes");
    assert_eq!(
        types(&events),
        [EventType::TextMessageStart, EventType::TextMessageEnd]
    );
}

#[test]
fn the_normalizer_reports_whether_a_stream_is_open() {
    let mut normalizer = ChunkNormalizer::new();
    let mut out = Vec::new();

    assert!(!normalizer.is_open());
    normalizer
        .normalize(text_chunk(Some("msg-1"), Some("Hi")), &mut out)
        .expect("normalizes");
    assert!(normalizer.is_open());

    normalizer.finish(&mut out);
    assert!(!normalizer.is_open());
    assert_eq!(
        out.last().map(Event::event_type),
        Some(EventType::TextMessageEnd)
    );
}

#[test]
fn the_applier_also_assembles_raw_chunks_on_its_own() {
    // Normalization is the recommended path, but an applier driven straight
    // from a raw stream must not silently drop the payload.
    let mut applier = Applier::new();
    for event in [
        text_chunk(Some("msg-1"), Some("Hel")),
        text_chunk(None, Some("lo")),
        tool_chunk(Some("call-1"), Some("t"), Some("{}")),
    ] {
        applier.apply(&event).expect("applies");
    }

    assert_eq!(applier.text_of("msg-1"), Some("Hello"));
    let ag_ui_core::Message::Assistant(assistant) = &applier.messages()[1] else {
        panic!("expected the tool call's message");
    };
    assert_eq!(
        assistant.tool_calls.as_ref().expect("calls")[0]
            .function
            .arguments,
        "{}"
    );
}
