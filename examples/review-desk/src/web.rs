use std::convert::Infallible;
use std::sync::Arc;

use axum::extract::State;
use axum::response::Html;
use axum::response::sse::{Event as SseEvent, Sse};
use axum::routing::get;
use futures_util::Stream;
use serde_json::json;

use crate::client::workflow_observed;

pub fn router(agent_url: String) -> axum::Router {
    axum::Router::new()
        .route(
            "/",
            get(|| async { Html(include_str!("../web/index.html")) }),
        )
        .route("/demo-events", get(demo))
        .with_state(agent_url)
}

async fn demo(
    State(agent_url): State<String>,
) -> Sse<impl Stream<Item = Result<SseEvent, Infallible>>> {
    let (sender, receiver) = tokio::sync::mpsc::unbounded_channel::<SseEvent>();
    let job = tokio::spawn(async move {
        let events = sender.clone();
        let observer = Arc::new(move |event: &ag_ui::Event| {
            if let Ok(event) = SseEvent::default().event("agui").json_data(event) {
                let _ = events.send(event);
            }
        });
        let mut transcript = Vec::new();
        let result = workflow_observed(&agent_url, &mut transcript, Some(observer)).await;
        let value = match result {
            Ok((evidence, _snapshot)) => json!({
                "evidence": evidence,
                "transcript": String::from_utf8_lossy(&transcript),
            }),
            Err(error) => json!({"error": error.to_string()}),
        };
        if let Ok(event) = SseEvent::default().event("done").json_data(value) {
            let _ = sender.send(event);
        }
    });
    // Closing a browser tab drops this guard and stops its local client job.
    struct CancelOnDrop(tokio::task::JoinHandle<()>);
    impl Drop for CancelOnDrop {
        fn drop(&mut self) {
            self.0.abort();
        }
    }
    Sse::new(futures_util::stream::unfold(
        (receiver, CancelOnDrop(job)),
        |(mut receiver, guard)| async move {
            receiver
                .recv()
                .await
                .map(|event| (Ok(event), (receiver, guard)))
        },
    ))
}
