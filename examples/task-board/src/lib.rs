//! A workshop task board, spoken over AG-UI.
//!
//! Two halves of one protocol in one crate, built the way an outside consumer
//! builds them — against the published crates, with nothing reached into:
//!
//! - [`agent`] is the server half: an `impl Agent` that streams a reply,
//!   executes tools, publishes the board as state, ships it as an A2UI surface,
//!   and pauses for a human before the one destructive command.
//! - [`chat`] is the client half: a terminal that folds the run back into
//!   messages, state and a drawn surface, and answers the pause.
//!
//! `src/main.rs` is the CLI over both. The tests under `tests/` drive the same
//! code paths without a terminal, so what the README shows is what CI runs.

pub mod agent;
pub mod board;
pub mod chat;
pub mod llm;

pub use agent::TaskBoard;
pub use board::Board;

use ag_ui_axum::RouterExt;
use axum::Router;
use axum::routing::get;

/// Where the agent is mounted.
pub const ROUTE: &str = "/agent";

/// The whole server: an AG-UI endpoint and a health check.
///
/// `route_agui` returns an ordinary [`Router`], which is the point — an AG-UI
/// agent is one route on an application that has others.
pub fn router(agent: TaskBoard) -> Router {
    Router::new()
        .route("/health", get(|| async { "ok" }))
        .route_agui(ROUTE, agent)
}
