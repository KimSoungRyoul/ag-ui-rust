//! A local review assistant built only with public SDK APIs.
//!
//! The application owns its checklist and decisions. The SDK owns streaming,
//! conversation state, approval bookkeeping and A2UI validation/recovery.

pub mod agent;
pub mod client;
pub mod interop;
mod web;

pub use agent::{ReviewDesk, ReviewState};

use ag_ui::axum::RouterExt;

/// An ordinary HTTP application with one AG-UI route.
pub fn router(agent_url: String) -> axum::Router {
    web::router(agent_url).route_agui("/agent", ReviewDesk)
}

/// Runs the example on loopback until the returned task is aborted.
pub async fn serve_local() -> std::io::Result<(String, tokio::task::JoinHandle<()>)> {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
    let url = format!("http://{}/agent", listener.local_addr()?);
    let app = router(url.clone());
    let task = tokio::spawn(async move {
        if let Err(error) = axum::serve(listener, app).await {
            eprintln!("review-desk server: {error}");
        }
    });
    Ok((url, task))
}
