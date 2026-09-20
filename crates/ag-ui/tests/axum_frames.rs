#![cfg(feature = "axum")]

use ag_ui::axum::{SseFrame, SseResponse};
use ag_ui::{Event, EventEnvelope};
use axum::body::to_bytes;
use futures_util::{StreamExt, stream};
use serde::Serialize;
use serde_json::json;
use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};

async fn body(response: axum::response::Response) -> String {
    String::from_utf8(
        to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap()
            .to_vec(),
    )
    .unwrap()
}

#[tokio::test]
async fn frames_send_extensions_and_safe_multiline_comments_with_sdk_headers() {
    let envelope = EventEnvelope::new(Event::run_started("thread", "run"))
        .with_extensions(json!({"hostProof":true}).as_object().unwrap().clone())
        .unwrap();
    let response = SseResponse::negotiate(None)
        .unwrap()
        .stream_frames(stream::iter([
            Ok(SseFrame::Event(envelope)),
            Ok(SseFrame::Comment(
                "replay\r\ndata: injected\r\nboundary".into(),
            )),
        ]));
    assert_eq!(response.headers()["content-type"], "text/event-stream");
    assert_eq!(
        response.headers()["cache-control"],
        "no-cache, no-store, no-transform"
    );
    assert_eq!(response.headers()["x-accel-buffering"], "no");
    assert_eq!(response.headers()["vary"], "accept");
    let actual = body(response).await;
    assert!(actual.starts_with("data: {"));
    assert!(actual.contains("\"hostProof\":true"));
    assert!(actual.ends_with(": replay\n: data: injected\n: boundary\n\n"));
    assert_eq!(actual.matches("\ndata:").count(), 0);
}

#[tokio::test]
async fn native_and_framed_event_paths_produce_identical_bytes() {
    let event = Event::text_message_content("m", "one\ntwo\rthree");
    let native = SseResponse::negotiate(None)
        .unwrap()
        .stream(stream::iter([Ok(event.clone())]));
    let framed = SseResponse::negotiate(None)
        .unwrap()
        .stream_frames(stream::iter([Ok(SseFrame::Event(event))]));
    assert_eq!(body(native).await, body(framed).await);
}

#[tokio::test]
async fn explicit_legacy_value_does_not_normalize_omissions_or_extra_fields() {
    let stored = json!({"type":"ACTIVITY_SNAPSHOT","messageId":"m","activityType":"surface","content":{"legacy":null},"hostProof":false});
    let response = SseResponse::negotiate(None)
        .unwrap()
        .stream_frames(stream::iter([Ok(SseFrame::Event(stored.clone()))]));
    let actual = body(response).await;
    assert_eq!(
        serde_json::from_str::<serde_json::Value>(actual.trim().strip_prefix("data: ").unwrap())
            .unwrap(),
        stored
    );
}

struct Bad;
impl Serialize for Bad {
    fn serialize<S: serde::Serializer>(&self, _: S) -> Result<S::Ok, S::Error> {
        Err(serde::ser::Error::custom("secret-provider-payload"))
    }
}

#[tokio::test]
async fn serialization_error_ends_the_stream_before_next_frame() {
    let polled = Arc::new(AtomicUsize::new(0));
    let counter = polled.clone();
    let source = stream::iter([
        Ok(SseFrame::Event(Bad)),
        Ok(SseFrame::Comment("must not be sent".into())),
    ])
    .inspect(move |_| {
        counter.fetch_add(1, Ordering::Relaxed);
    });
    let response = SseResponse::negotiate(None).unwrap().stream_frames(source);
    let actual = body(response).await;
    assert_eq!(actual.matches("RUN_ERROR").count(), 1);
    assert!(actual.contains("SERIALIZATION"));
    assert!(!actual.contains("must not be sent"));
    assert!(!actual.contains("secret-provider-payload"));
    assert_eq!(polled.load(Ordering::Relaxed), 1);
}

struct OwnedSubscription {
    dropped: Arc<AtomicUsize>,
}
impl Drop for OwnedSubscription {
    fn drop(&mut self) {
        self.dropped.fetch_add(1, Ordering::Relaxed);
    }
}
impl futures_util::Stream for OwnedSubscription {
    type Item = ag_ui::server::Result<SseFrame<Event>>;
    fn poll_next(
        self: std::pin::Pin<&mut Self>,
        _: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Option<Self::Item>> {
        std::task::Poll::Pending
    }
}

#[tokio::test]
async fn dropping_external_subscription_cleans_up_only_the_subscription() {
    let execution = ag_ui::server::CancellationToken::new();
    let dropped = Arc::new(AtomicUsize::new(0));
    let response = SseResponse::negotiate(None)
        .unwrap()
        .stream_frames(OwnedSubscription {
            dropped: dropped.clone(),
        });
    drop(response);
    assert_eq!(dropped.load(Ordering::Relaxed), 1);
    assert!(!execution.is_cancelled());
}

#[tokio::test]
async fn channel_failure_stops_polling_after_one_terminal() {
    let polled = Arc::new(AtomicUsize::new(0));
    let counter = polled.clone();
    let source = stream::iter([
        Err(ag_ui::server::Error::agent("channel unavailable")),
        Ok(SseFrame::<Event>::Comment("must not poll".into())),
    ])
    .inspect(move |_| {
        counter.fetch_add(1, Ordering::Relaxed);
    });
    let response = SseResponse::negotiate(None).unwrap().stream_frames(source);
    let actual = body(response).await;
    assert_eq!(actual.matches("RUN_ERROR").count(), 1);
    assert_eq!(polled.load(Ordering::Relaxed), 1);
}

#[test]
fn comment_newlines_cannot_inject_data_or_event_fields() {
    assert_eq!(
        ag_ui::encode::sse::comment("a\rb\nc\r\nd"),
        ": a\n: b\n: c\n: d\n\n"
    );
    assert_eq!(
        ag_ui::encode::sse::comment("\ndata: injected\n\n"),
        ": \n: data: injected\n: \n: \n\n"
    );
}

#[tokio::test]
async fn explicitly_attached_cancellation_trips_when_framed_body_is_dropped() {
    let token = ag_ui::server::CancellationToken::new();
    let response = SseResponse::negotiate(None)
        .unwrap()
        .cancellation(token.clone())
        .stream_frames(stream::pending::<ag_ui::server::Result<SseFrame<Event>>>());
    drop(response);
    assert!(token.is_cancelled());
}

#[tokio::test]
async fn serialization_failure_cancels_explicitly_owned_request_work() {
    let token = ag_ui::server::CancellationToken::new();
    let response = SseResponse::negotiate(None)
        .unwrap()
        .cancellation(token.clone())
        .stream_frames(stream::iter([Ok(SseFrame::Event(Bad))]));
    let actual = body(response).await;
    assert_eq!(actual.matches("RUN_ERROR").count(), 1);
    assert!(token.is_cancelled());
}

#[tokio::test]
async fn channel_failure_cancels_explicitly_owned_request_work() {
    let token = ag_ui::server::CancellationToken::new();
    let response = SseResponse::negotiate(None)
        .unwrap()
        .cancellation(token.clone())
        .stream_frames(stream::iter([Err::<SseFrame<Event>, _>(
            ag_ui::server::Error::agent("channel unavailable"),
        )]));
    let actual = body(response).await;
    assert_eq!(actual.matches("RUN_ERROR").count(), 1);
    assert!(token.is_cancelled());
}

#[tokio::test]
async fn clean_framed_eof_does_not_cancel_completed_request_work() {
    let token = ag_ui::server::CancellationToken::new();
    let response = SseResponse::negotiate(None)
        .unwrap()
        .cancellation(token.clone())
        .stream_frames(stream::iter([Ok(SseFrame::Event(Event::run_finished(
            "thread", "run",
        )))]));
    let actual = body(response).await;
    assert!(actual.contains("RUN_FINISHED"));
    assert!(!token.is_cancelled());
}
