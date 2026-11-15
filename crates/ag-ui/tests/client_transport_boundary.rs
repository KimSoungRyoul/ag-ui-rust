#![cfg(feature = "client")]

use ag_ui::client::transport::{ReplayTransport, Transport, TransportFuture, TransportStream};
use ag_ui::client::{Error, RemoteAgent, RunEnd, RunParams, Thread, Update};
use ag_ui::{Event, EventType, RunAgentInput};
use futures_util::{StreamExt, stream};
use serde_json::{Value, json};

#[derive(Clone)]
struct TranslatingRawTransport {
    events: Vec<Value>,
}

impl Transport for TranslatingRawTransport {
    fn run(&self, _input: RunAgentInput) -> TransportFuture {
        let values = self.events.clone().into_iter().map(|mut value| {
            // A transport-owned compatibility shim sees the raw shape before
            // the shared 1.0 boundary removes unknown event types.
            if value["type"] == "LEGACY_TEXT" {
                value["type"] = json!("TEXT_MESSAGE_CONTENT");
                value["messageId"] = json!("m");
                value["delta"] = json!("translated");
                value.as_object_mut().unwrap().remove("text");
            }
            Ok(value)
        });
        Box::pin(async move { Ok(TransportStream::json(stream::iter(values))) })
    }
}

#[tokio::test]
async fn raw_transport_translates_before_shared_enforcement() {
    let transport = TranslatingRawTransport {
        events: vec![
            json!({"type":"RUN_STARTED","threadId":"t","runId":"r"}),
            json!({"type":"LEGACY_TEXT","text":"translated"}),
            json!({"type":"FUTURE_EVENT","payload":true}),
            json!({"type":"RUN_FINISHED","threadId":"t","runId":"r"}),
        ],
    };
    let events: Vec<Event> = RemoteAgent::new(transport)
        .run_events(RunParams::new("t", "r"))
        .map(Result::unwrap)
        .collect()
        .await;
    assert_eq!(
        events.iter().map(Event::event_type).collect::<Vec<_>>(),
        [
            EventType::RunStarted,
            EventType::TextMessageContent,
            EventType::RunFinished
        ]
    );
}

#[tokio::test]
async fn typed_replay_uses_the_same_remote_agent_boundary() {
    let transport = ReplayTransport::new([
        Event::run_started("t", "r"),
        Event::run_finished_success("t", "r"),
    ]);
    let events: Vec<Event> = RemoteAgent::new(transport)
        .run_events(RunParams::new("t", "r"))
        .map(Result::unwrap)
        .collect()
        .await;
    assert_eq!(events.len(), 2);
    assert_eq!(events[0].event_type(), EventType::RunStarted);
    assert_eq!(events[1].event_type(), EventType::RunFinished);
}

/// Hands on typed events, as a source with no wire has them.
#[derive(Clone)]
struct Typed {
    events: Vec<Event>,
    enforced: bool,
}

impl Typed {
    fn new(events: impl IntoIterator<Item = Event>) -> Self {
        Self {
            events: events.into_iter().collect(),
            enforced: false,
        }
    }

    fn enforced(mut self) -> Self {
        self.enforced = true;
        self
    }
}

impl Transport for Typed {
    fn run(&self, _input: RunAgentInput) -> TransportFuture {
        let stream = TransportStream::typed(stream::iter(self.events.clone().into_iter().map(Ok)));
        let stream = if self.enforced {
            stream.enforced()
        } else {
            stream
        };
        Box::pin(async move { Ok(stream) })
    }
}

/// A value an `Event` can hold and the 1.0 schema refuses: `timestamp` is
/// bounded to the range a JSON number survives.
fn outside_the_schema() -> Event {
    let mut event = Event::run_started("t", "r");
    let Event::RunStarted(started) = &mut event else {
        unreachable!()
    };
    started.base.timestamp = Some(i64::MAX);
    event
}

fn retired(tag: &str) -> Event {
    serde_json::from_value(json!({"type": tag, "delta": "hm"})).unwrap()
}

#[tokio::test]
async fn typed_events_are_delivered_as_given_unless_enforcement_is_asked_for() {
    let script = [outside_the_schema(), Event::run_finished_success("t", "r")];

    // Not serialized, not walked against the schema, not deserialized.
    let events: Vec<Event> = RemoteAgent::new(Typed::new(script.clone()))
        .run_events(RunParams::new("t", "r"))
        .map(Result::unwrap)
        .collect()
        .await;
    assert_eq!(events, script);

    // The same events through the boundary a peer's bytes go through.
    let enforced: Vec<_> = RemoteAgent::new(Typed::new(script).enforced())
        .run_events(RunParams::new("t", "r"))
        .collect()
        .await;
    assert!(
        matches!(&enforced[..], [Err(Error::Protocol(message))] if message.contains("timestamp")),
        "{enforced:?}"
    );
}

#[tokio::test]
async fn a_thread_gets_the_warnings_a_typed_stream_can_cause() {
    let mut started = Event::run_started("t", "r");
    let Event::RunStarted(event) = &mut started else {
        unreachable!()
    };
    event.protocol_version = Some("9.0".to_owned());
    let transport = Typed::new([
        started,
        retired("THINKING_START"),
        retired("THINKING_TEXT_MESSAGE_START"),
        retired("THINKING_TEXT_MESSAGE_CONTENT"),
        retired("THINKING_TEXT_MESSAGE_CONTENT"),
        retired("THINKING_TEXT_MESSAGE_END"),
        retired("THINKING_END"),
        Event::run_finished_success("t", "r"),
    ]);
    let mut thread = Thread::new(transport, "t");
    thread.set_next_run_id("r");

    let mut run = thread.send("go").unwrap();
    let mut warnings = Vec::new();
    let mut reasoning = String::new();
    let mut end = None;
    while let Some(update) = run.next().await {
        match update {
            Update::Warning(warning) => warnings.push(warning),
            Update::Reasoning(update) => reasoning = update.text,
            Update::Error(error) => panic!("{error}"),
            Update::Done(done) => end = Some(done),
            _ => {}
        }
    }
    drop(run);

    assert_eq!(end, Some(RunEnd::Success { result: None }));
    // Each once, however many events of that kind arrived.
    assert_eq!(
        warnings,
        [
            "producer declared newer or uninterpretable protocol version \"9.0\"",
            "translated retired THINKING_START to REASONING_START",
            "translated retired THINKING_TEXT_MESSAGE_START to REASONING_MESSAGE_START",
            "translated retired THINKING_TEXT_MESSAGE_CONTENT to REASONING_MESSAGE_CONTENT",
            "translated retired THINKING_TEXT_MESSAGE_END to REASONING_MESSAGE_END",
            "translated retired THINKING_END to REASONING_END",
        ]
    );
    // The retired events reached the thread as the reasoning they translate to.
    assert_eq!(reasoning, "hmhm");
    assert_eq!(thread.reasoning().len(), 1);
}

#[tokio::test]
async fn into_json_is_what_a_proxy_forwards() {
    // Wire JSON comes back untouched, unknown fields included.
    let wire = json!({"type": "RUN_STARTED", "threadId": "t", "runId": "r", "vendor": {"x": 1}});
    let transport = TranslatingRawTransport {
        events: vec![wire.clone()],
    };
    let raw: Vec<_> = transport
        .run(RunAgentInput::new("t", "r"))
        .await
        .unwrap()
        .into_json()
        .map(Result::unwrap)
        .collect()
        .await;
    assert_eq!(raw, [wire]);

    // Typed events are serialized.
    let event = Event::text_message_content("m", "hello");
    let raw: Vec<_> = Typed::new([event.clone()])
        .run(RunAgentInput::new("t", "r"))
        .await
        .unwrap()
        .into_json()
        .map(Result::unwrap)
        .collect()
        .await;
    assert_eq!(raw, [serde_json::to_value(&event).unwrap()]);
}

#[tokio::test]
async fn into_events_hands_the_warnings_to_the_caller() {
    let transport = TranslatingRawTransport {
        events: vec![json!({"type": "RUN_STARTED", "threadId": "t", "runId": "r", "extra": 1})],
    };
    let stream = transport.run(RunAgentInput::new("t", "r")).await.unwrap();
    let mut warnings = Vec::new();
    let events: Vec<_> = stream
        .into_events(|warning| warnings.push(warning))
        .collect()
        .await;
    assert!(matches!(&events[..], [Ok(Event::RunStarted(_))]));
    assert_eq!(warnings, ["unrecognised property at /extra"]);
}
