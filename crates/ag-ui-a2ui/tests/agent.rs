//! The agent layer through its public API: what a framework would write.
//!
//! The conformance suite (`tests/conformance.rs`) fixes what the layer reads
//! and emits. These tests fix what it promises a caller around that: what a
//! rejected answer leaves in the state, what a stream has committed when it
//! fails, and that nothing borrows.

#![cfg(feature = "agent")]

use std::sync::Arc;

use ag_ui_a2ui::agent::{
    A2uiGenerator, A2uiRequestProcessor, AfterRejectedBlock, AnswerPart, CatalogConfig,
    CatalogDocument, DirectJson, RendererCapabilities, RendererState, ResponsePart, ResponseStream,
    RetryDriver, StreamOptions, StreamPart, StreamedAnswer, Verdict,
};
use ag_ui_a2ui::{A2uiVersion, AgentMessage, Component, Error, ErrorCode};
use serde_json::json;

const BASIC: &str = "https://a2ui.org/specification/v0_9/catalogs/basic/catalog.json";

fn processor() -> A2uiRequestProcessor {
    let generator = A2uiGenerator::new(vec![CatalogConfig::basic(A2uiVersion::V0_9).unwrap()]);
    let renderer = RendererCapabilities::supporting(A2uiVersion::V0_9, [BASIC]);
    generator.create_processor(Some(&renderer)).unwrap()
}

/// A block that creates `surface` and draws one text in it.
fn surface(surface: &str, text: &str) -> String {
    format!(
        r#"<a2ui-json>[
  {{"version": "v0.9", "createSurface": {{"surfaceId": "{surface}", "catalogId": "{BASIC}"}}}},
  {{"version": "v0.9", "updateComponents": {{"surfaceId": "{surface}", "components": [
    {{"id": "root", "component": "Text", "text": "{text}"}}]}}}}
]</a2ui-json>"#
    )
}

/// A block that updates `surface` with a component the catalog does not have.
fn bad_update(surface: &str) -> String {
    format!(
        r#"<a2ui-json>[{{"version": "v0.9", "updateComponents": {{"surfaceId": "{surface}", "components": [
    {{"id": "extra", "component": "Sparkline"}}]}}}}]</a2ui-json>"#
    )
}

fn texts(parts: &[StreamPart]) -> Vec<String> {
    parts
        .iter()
        .map(|part| match part {
            StreamPart::Text(text) => format!("text {text}"),
            StreamPart::A2ui(messages) => format!("a2ui x{}", messages.len()),
            StreamPart::Rejected(error) => format!("rejected {}", category(error)),
        })
        .collect()
}

fn category(error: &Error) -> &'static str {
    match error {
        Error::Parse(_) | Error::Json(_) => "parse",
        Error::Validation { .. } => "validation",
        Error::Catalog(_) => "catalog",
        _ => "other",
    }
}

// --- an answer read whole is all or nothing ---------------------------------------------------

#[test]
fn a_rejected_answer_leaves_the_state_as_it_was() {
    let processor = processor();
    let mut state = processor.create_state();
    processor
        .parse_response_in(&surface("kept", "Before"), &mut state)
        .unwrap();
    let before = state.clone();

    // The first block is fine and would create a surface; the second is not.
    let answer = format!("Text.\n{}\n{}", surface("new", "Hi"), bad_update("kept"));
    let rejected = processor
        .parse_response_in(&answer, &mut state)
        .unwrap_err();
    assert_eq!(category(&rejected.error), "validation");
    assert_eq!(state, before, "nothing of the answer was applied");
    assert!(state.surface("new").is_none());

    // What was read before the failing block is reported, not applied.
    assert!(matches!(
        rejected.parts.as_slice(),
        [ResponsePart::Text(text), ResponsePart::A2ui(messages)]
            if text == "Text." && messages.len() == 2
    ));
}

#[test]
fn an_answer_whose_tree_is_broken_keeps_only_its_text() {
    let processor = processor();
    let mut state = processor.create_state();
    let before = state.clone();
    let answer = format!(
        r#"Here.<a2ui-json>[
  {{"version": "v0.9", "createSurface": {{"surfaceId": "s", "catalogId": "{BASIC}"}}}},
  {{"version": "v0.9", "updateComponents": {{"surfaceId": "s", "components": [
    {{"id": "root", "component": "Card", "child": "missing"}}]}}}}
]</a2ui-json>"#
    );
    let rejected = processor
        .parse_response_in(&answer, &mut state)
        .unwrap_err();
    let Error::Validation { errors } = &rejected.error else {
        panic!("expected a validation error, got {:?}", rejected.error);
    };
    assert_eq!(errors.0[0].code, ErrorCode::UnresolvedChild);
    assert_eq!(
        errors.0[0].path,
        "messages[1].updateComponents.components[0].child"
    );
    assert!(matches!(
        rejected.parts.as_slice(),
        [ResponsePart::Text(text)] if text == "Here."
    ));
    assert_eq!(state, before);
}

#[test]
fn a_canvas_turn_is_checked_against_what_earlier_turns_built() {
    let processor = processor();
    let mut state = processor.create_state();
    processor
        .parse_response_in(&surface("canvas", "One"), &mut state)
        .unwrap();

    // A later turn updates the surface without creating it again.
    let update = r#"<a2ui-json>[{"version": "v0.9", "updateComponents": {"surfaceId": "canvas", "components": [
    {"id": "root", "component": "Text", "text": "Two"}]}}]</a2ui-json>"#;
    processor.parse_response_in(update, &mut state).unwrap();
    assert_eq!(
        state.surface("canvas").unwrap().components[0].props["text"],
        "Two"
    );

    // The same update is refused in a chat turn, where no surface exists yet.
    assert!(processor.parse_response(update).is_err());
}

#[test]
fn what_a_canvas_turn_leaves_behind_is_not_held_to_a_scope() {
    let processor = processor();
    let mut state = processor.create_state();
    // A list of rows, each bound to its item by a relative path.
    let list = format!(
        r#"<a2ui-json>[
  {{"version": "v0.9", "createSurface": {{"surfaceId": "canvas", "catalogId": "{BASIC}"}}}},
  {{"version": "v0.9", "updateComponents": {{"surfaceId": "canvas", "components": [
    {{"id": "root", "component": "List", "children": {{"componentId": "row", "path": "/items"}}}},
    {{"id": "row", "component": "Text", "text": {{"path": "name"}}}}]}}}},
  {{"version": "v0.9", "updateDataModel": {{"surfaceId": "canvas", "path": "/items", "value": [{{"name": "One"}}]}}}}
]</a2ui-json>"#
    );
    processor.parse_response_in(&list, &mut state).unwrap();

    // The next turn replaces the list. No message removes `row`, so it stays,
    // with its relative path and no list above it to give that a scope.
    let replaced = r#"<a2ui-json>[{"version": "v0.9", "updateComponents": {"surfaceId": "canvas", "components": [
    {"id": "root", "component": "Text", "text": "Booked."}]}}]</a2ui-json>"#;
    processor.parse_response_in(replaced, &mut state).unwrap();
    assert_eq!(state.surface("canvas").unwrap().components.len(), 2);

    // A relative path on a component that is drawn is still refused.
    let drawn = r#"<a2ui-json>[{"version": "v0.9", "updateComponents": {"surfaceId": "canvas", "components": [
    {"id": "root", "component": "Text", "text": {"path": "name"}}]}}]</a2ui-json>"#;
    let rejected = processor.parse_response_in(drawn, &mut state).unwrap_err();
    let Error::Validation { errors } = rejected.error else {
        panic!("expected a validation error");
    };
    assert_eq!(errors.0[0].code, ErrorCode::UnresolvedBinding);
    assert_eq!(
        errors.len(),
        1,
        "the row left behind is not reported: {errors}"
    );
}

#[test]
fn one_block_without_tags_is_read_like_an_answer() {
    let processor = processor();
    let mut state = processor.create_state();
    let block = format!(
        r#"[{{"version": "v0.9", "createSurface": {{"surfaceId": "tool", "catalogId": "{BASIC}"}}}}]"#
    );
    let parts = processor.parse_block_in(&block, &mut state).unwrap();
    assert!(matches!(parts.as_slice(), [ResponsePart::A2ui(messages)] if messages.len() == 1));
    assert!(state.surface("tool").is_some());

    // Text that is not a block is a parse error here, not conversation.
    let before = state.clone();
    let rejected = processor
        .parse_block_in("Here is your surface.", &mut state)
        .unwrap_err();
    assert_eq!(category(&rejected.error), "parse");
    assert_eq!(state, before);
}

// --- the retry driver ---------------------------------------------------------------------------

#[test]
fn after_retry_and_after_giving_up_the_state_is_what_it_was() {
    let processor = processor();
    let mut state = processor.create_state();
    processor
        .parse_response_in(&surface("kept", "Before"), &mut state)
        .unwrap();
    let before = state.clone();
    let mut retry = RetryDriver::new(&processor, 2);

    let bad = format!("{}\n{}", surface("new", "Hi"), bad_update("kept"));
    let Verdict::Retry(feedback) = retry.judge(&bad, &mut state) else {
        panic!("the first rejection asks again");
    };
    assert_eq!(state, before);
    assert_eq!(feedback.attempt(), 1);
    assert_eq!(feedback.errors()[0].code, ErrorCode::UnknownComponent);
    // The second block's first message is the third of the answer.
    assert_eq!(
        feedback.errors()[0].path,
        "messages[2].updateComponents.components[0].component"
    );
    let prompt = feedback.prompt("Add a sparkline.");
    assert!(prompt.starts_with("Add a sparkline.\n\n## Correction required\n"));

    let Verdict::GiveUp(error) = retry.judge(&bad, &mut state) else {
        panic!("the attempts are used up");
    };
    assert!(matches!(
        error,
        Error::RecoveryExhausted { attempts: 2, ref last } if last.len() == 1
    ));
    assert_eq!(state, before);
    assert_eq!(retry.attempts(), 2);
}

#[test]
fn an_answer_that_cannot_be_read_is_fed_back_too() {
    let processor = processor();
    let mut state = processor.create_state();
    let mut retry = RetryDriver::new(&processor, 3);
    let Verdict::Retry(feedback) =
        retry.judge("<a2ui-json>[{\"version\": </a2ui-json>", &mut state)
    else {
        panic!("a block that is not JSON asks again");
    };
    assert_eq!(feedback.errors()[0].path, "response");
    assert!(feedback.errors()[0].message.contains("not JSON"));
}

#[test]
fn a_state_of_other_catalogs_is_not_the_models_to_fix() {
    let processor = processor();
    let other = CatalogDocument::new(json!({
        "catalogId": "other", "protocolVersion": "0.9", "components": {},
    }))
    .unwrap();
    let mut foreign = RendererState::new(vec![other]).unwrap();
    assert!(!processor.accepts_state(&foreign));

    let mut retry = RetryDriver::new(&processor, 3);
    assert!(matches!(
        retry.judge("Hello.", &mut foreign),
        Verdict::GiveUp(Error::Catalog(_))
    ));
    assert!(matches!(
        processor.open_stream(&foreign, StreamOptions::default()),
        Err(Error::Catalog(_))
    ));
    assert!(matches!(
        StreamedAnswer::open(&processor, &foreign),
        Err(Error::Catalog(_))
    ));
}

// --- a streamed answer --------------------------------------------------------------------------

#[test]
fn the_state_holds_exactly_what_a_stream_emitted() {
    let processor = processor();
    let mut state = processor.create_state();
    let mut stream = processor
        .open_stream(&state, StreamOptions::default())
        .unwrap();

    let first = stream
        .feed(&format!("Here.{}", surface("a", "One")), &mut state)
        .unwrap();
    assert_eq!(texts(&first), ["text Here.", "a2ui x2"]);
    assert!(state.surface("a").is_some());
    let after_first = state.clone();

    // The second block creates a surface, then is rejected.
    let second = format!(
        r#"<a2ui-json>[
  {{"version": "v0.9", "createSurface": {{"surfaceId": "b", "catalogId": "{BASIC}"}}}},
  {{"version": "v0.9", "updateComponents": {{"surfaceId": "b", "components": [
    {{"id": "root", "component": "Sparkline"}}]}}}}
]</a2ui-json>"#
    );
    let rejected = stream.feed(&second, &mut state).unwrap_err();
    assert_eq!(category(&rejected.error), "validation");
    // Nothing of the rejected block had been emitted, so nothing of it stays.
    assert!(rejected.parts.is_empty());
    assert_eq!(state, after_first);

    // The stream has ended.
    assert!(!stream.is_in_block());
    assert_eq!(
        category(&stream.feed("more", &mut state).unwrap_err().error),
        "parse"
    );
    assert!(stream.close(&mut state).unwrap().is_empty());
}

#[test]
fn what_a_block_emitted_before_it_was_rejected_stays_and_can_be_rewound() {
    let processor = processor();
    let mut state = processor.create_state();
    processor
        .parse_response_in(&surface("kept", "Before"), &mut state)
        .unwrap();
    let opened = state.clone();
    let mut stream = processor
        .open_stream(&state, StreamOptions::default())
        .unwrap();

    // The create closes in the first chunk and is emitted there.
    let emitted = stream
        .feed(
            &format!(
                r#"<a2ui-json>[{{"version": "v0.9", "createSurface": {{"surfaceId": "b", "catalogId": "{BASIC}"}}}},"#
            ),
            &mut state,
        )
        .unwrap();
    assert_eq!(texts(&emitted), ["a2ui x1"]);
    assert!(stream.is_in_block());

    let rejected = stream
        .feed(
            r#"{"version": "v0.9", "updateComponents": {"surfaceId": "b", "components": [{"id": "root", "component": "Sparkline"}]}}]</a2ui-json>"#,
            &mut state,
        )
        .unwrap_err();
    assert_eq!(category(&rejected.error), "validation");
    // A renderer that was sent the first chunk's part holds the surface, and
    // so does the state.
    assert!(state.surface("b").is_some());
    assert_ne!(state, opened);

    // A caller that did not send it goes back.
    stream.rewind(&mut state);
    assert_eq!(state, opened);
}

#[test]
fn a_stream_can_read_past_a_rejected_block() {
    let processor = processor();
    let mut state = processor.create_state();
    let mut stream = processor
        .open_stream(
            &state,
            StreamOptions {
                after_rejected_block: AfterRejectedBlock::Continue,
                ..StreamOptions::default()
            },
        )
        .unwrap();
    let answer = format!(
        "Before {} between {} after <a2ui-json>[",
        bad_update("nowhere"),
        surface("good", "Hi")
    );
    let parts = stream.feed(&answer, &mut state).unwrap();
    assert_eq!(
        texts(&parts),
        [
            "text Before",
            "rejected validation",
            "text between",
            "a2ui x2",
            "text after"
        ]
    );
    assert!(stream.is_in_block());
    assert_eq!(
        texts(&stream.close(&mut state).unwrap()),
        ["rejected parse"]
    );
    assert!(state.surface("good").is_some());
}

#[test]
fn prose_under_a_progressive_key_shows_as_it_is_written() {
    let generator = A2uiGenerator::new(vec![CatalogConfig::basic(A2uiVersion::V0_9).unwrap()])
        .with_format(Arc::new(DirectJson::new().with_progressive_keys(["text"])));
    let processor = generator
        .create_processor(Some(&RendererCapabilities::supporting(
            A2uiVersion::V0_9,
            [BASIC],
        )))
        .unwrap();
    let mut state = processor.create_state();
    let mut stream = processor
        .open_stream(&state, StreamOptions::default())
        .unwrap();

    let chunks = [
        format!(
            r#"<a2ui-json>[{{"version": "v0.9", "createSurface": {{"surfaceId": "s", "catalogId": "{BASIC}"}}}}, "#
        ),
        r#"{"version": "v0.9", "updateComponents": {"surfaceId": "s", "components": [{"id": "root"#
            .to_string(),
        r#"", "component": "Text", "text": "Hel"#.to_string(),
        r#"lo, wor"#.to_string(),
        r#"ld"}]}}]</a2ui-json>"#.to_string(),
    ];
    let mut seen = Vec::new();
    for chunk in &chunks {
        for part in stream.feed(chunk, &mut state).unwrap() {
            if let StreamPart::A2ui(messages) = part {
                for message in messages {
                    let value = serde_json::to_value(&message).unwrap();
                    if let Some(text) = value.pointer("/updateComponents/components/0/text") {
                        seen.push(text.as_str().unwrap().to_string());
                    }
                }
            }
        }
        // The state holds what was emitted, at every chunk.
        if let Some(text) = seen.last() {
            assert_eq!(
                state.surface("s").unwrap().components[0].props["text"],
                *text
            );
        }
    }
    stream.close(&mut state).unwrap();
    // An id is never ended early; the prose is.
    assert_eq!(seen, ["Hel", "Hello, wor", "Hello, world"]);
}

#[test]
fn a_streamed_answer_lays_surfaces_out_among_the_text() {
    let processor = processor();
    let mut state = processor.create_state();
    let mut answer = StreamedAnswer::open(&processor, &state).unwrap();
    for chunk in [
        "Here ".to_string(),
        "it is: ".to_string(),
        surface("a", "Hi"),
        format!(" and {} then ", bad_update("a")),
        r#"<a2ui-json>{"version": "v0.9", "deleteSurface": {"surfaceId": "a"}}</a2ui-json> gone."#
            .to_string(),
    ] {
        answer.feed(&chunk, &mut state).unwrap();
    }
    answer.close(&mut state).unwrap();

    let laid_out: Vec<String> = answer
        .parts()
        .iter()
        .map(|part| match part {
            AnswerPart::Text(text) => format!("text {text}"),
            AnswerPart::Surface(id) => format!("surface {id}"),
            AnswerPart::Rejected(error) => format!("rejected {}", category(error)),
        })
        .collect();
    assert_eq!(
        laid_out,
        [
            "text Here it is:",
            "text and",
            "rejected validation",
            "text then",
            "text gone."
        ],
        "the surface the answer deleted left its parts"
    );
    assert!(state.surface("a").is_none());
}

// --- what a framework holds -----------------------------------------------------------------------

/// Everything a framework keeps for one conversation, in one struct, moved
/// into a task and held across an await.
struct Conversation {
    processor: A2uiRequestProcessor,
    state: RendererState,
    retry: RetryDriver,
    stream: ResponseStream,
}

#[test]
fn a_conversation_moves_between_tasks_and_survives_an_await() {
    async fn model(_prompt: &str) -> String {
        surface("s", "Hello")
    }
    async fn turn(mut conversation: Conversation) -> Conversation {
        let answer = model("Show a greeting.").await;
        let verdict = conversation.retry.judge(&answer, &mut conversation.state);
        assert!(matches!(verdict, Verdict::Done(_)));
        conversation
            .stream
            .feed("More.", &mut conversation.state)
            .unwrap();
        conversation
    }
    fn spawn<F: std::future::Future + Send + 'static>(future: F) -> F {
        future
    }

    let processor = processor();
    let state = processor.create_state();
    let conversation = Conversation {
        retry: RetryDriver::new(&processor, 3),
        stream: processor
            .open_stream(&state, StreamOptions::default())
            .unwrap(),
        state,
        processor,
    };
    let mut task = std::pin::pin!(spawn(turn(conversation)));
    let mut context = std::task::Context::from_waker(std::task::Waker::noop());
    let std::task::Poll::Ready(conversation) =
        std::future::Future::poll(task.as_mut(), &mut context)
    else {
        panic!("the turn has nothing to wait for");
    };
    assert!(conversation.state.surface("s").is_some());
    assert!(conversation.processor.accepts_state(&conversation.state));
}

// --- creation -------------------------------------------------------------------------------------

#[test]
fn an_example_outside_the_active_catalogs_fails_when_the_processor_is_created() {
    let example = vec![
        AgentMessage::create_surface("s", BASIC),
        AgentMessage::update_components("s", vec![Component::new("root", "Sparkline")]),
    ];
    let generator = A2uiGenerator::new(vec![CatalogConfig::basic(A2uiVersion::V0_9).unwrap()])
        .with_examples(vec![example]);
    let renderer = RendererCapabilities::supporting(A2uiVersion::V0_9, [BASIC]);
    assert!(matches!(
        generator.create_processor(Some(&renderer)),
        Err(Error::Validation { .. })
    ));
    assert!(matches!(
        generator.create_processor(None),
        Err(Error::Catalog(_))
    ));
}

#[test]
fn the_prompt_snippet_describes_what_negotiation_left_active() {
    let charts = CatalogDocument::new(json!({
        "catalogId": "test/charts",
        "protocolVersion": "0.9",
        "components": {"Chart": {"properties": {}}, "Gauge": {"properties": {}}},
    }))
    .unwrap();
    let generator = A2uiGenerator::new(vec![
        CatalogConfig::basic(A2uiVersion::V0_9).unwrap(),
        CatalogConfig::new(charts).keeping_components(["Chart"]),
    ]);
    let processor = generator
        .create_processor(Some(&RendererCapabilities::supporting(
            A2uiVersion::V0_9,
            ["test/charts"],
        )))
        .unwrap();
    let snippet = processor.prompt_snippet();
    assert!(snippet.contains("<a2ui-json>"));
    assert!(snippet.contains("test/charts") && snippet.contains("Chart"));
    assert!(
        !snippet.contains("Gauge"),
        "a pruned component is not offered"
    );
    assert!(
        !snippet.contains("ChoicePicker"),
        "nor is a catalog the renderer did not ask for"
    );

    // The generator is as it was: the next renderer negotiates from scratch.
    assert_eq!(generator.catalogs().len(), 2);
    assert_eq!(
        generator.catalogs()[1].catalog().catalog().components.len(),
        2
    );
}

#[cfg(feature = "schema-validation")]
#[test]
fn a_schema_check_sees_what_the_semantic_checks_cannot() {
    let plain = processor();
    let strict = plain.checking_schemas(&Default::default()).unwrap();
    // `colour` is not a property of Text. Only a schema engine can say so.
    let answer = surface("s", "Hi").replace(r#""text": "Hi""#, r#""text": "Hi", "colour": "red""#);
    plain.parse_response(&answer).unwrap();
    let rejected = strict.parse_response(&answer).unwrap_err();
    assert_eq!(category(&rejected.error), "validation");

    // A state made by either processor serves both, and is checked by the
    // one that reads into it.
    let mut state = plain.create_state();
    assert!(strict.parse_response_in(&answer, &mut state).is_err());
    plain.parse_response_in(&answer, &mut state).unwrap();
}

// --- cost -----------------------------------------------------------------------------------------

/// Not a test of behaviour: prints what streaming one large message costs
/// today. Run with
/// `cargo test -p ag-ui-a2ui --features agent --release --test agent -- --ignored --nocapture`.
#[test]
#[ignore = "a measurement, not an assertion"]
fn the_cost_of_streaming_one_large_message() {
    let processor = processor();
    for components in [250usize, 1000, 4000] {
        let mut answer = format!(
            r#"<a2ui-json>[{{"version": "v0.9", "createSurface": {{"surfaceId": "s", "catalogId": "{BASIC}"}}}}, {{"version": "v0.9", "updateComponents": {{"surfaceId": "s", "components": [{{"id": "root", "component": "Column", "children": ["#
        );
        let ids: Vec<String> = (0..components).map(|n| format!("\"t{n}\"")).collect();
        answer.push_str(&ids.join(", "));
        answer.push_str("]}");
        for n in 0..components {
            answer.push_str(&format!(
                r#", {{"id": "t{n}", "component": "Text", "text": "Row number {n} of the table"}}"#
            ));
        }
        answer.push_str("]}}]</a2ui-json>");

        let whole = std::time::Instant::now();
        processor.parse_response(&answer).unwrap();
        let whole = whole.elapsed();

        for chunk_size in [64usize, 1024] {
            let mut state = processor.create_state();
            let mut stream = processor
                .open_stream(&state, StreamOptions::default())
                .unwrap();
            let started = std::time::Instant::now();
            let mut emitted = 0;
            for chunk in answer.as_bytes().chunks(chunk_size) {
                emitted += stream
                    .feed(std::str::from_utf8(chunk).unwrap(), &mut state)
                    .unwrap()
                    .len();
            }
            stream.close(&mut state).unwrap();
            println!(
                "{components} components, {} bytes, {chunk_size}-byte chunks: streamed in {:?} \
                 ({emitted} parts), read whole in {whole:?}",
                answer.len(),
                started.elapsed()
            );
        }
    }
}
