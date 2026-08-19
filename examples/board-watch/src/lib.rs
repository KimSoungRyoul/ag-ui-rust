//! A terminal client for any AG-UI agent — and the awkward agent it is aimed
//! at.
//!
//! Round two of dogfooding this SDK, from the consuming side. `task-board` was
//! an agent with a client attached; this is a *client*, and the server here
//! exists only to give it something hostile to read.
//!
//! - [`watch`] is the application: send a line, render the run, answer what it
//!   pauses on, draw the board.
//! - [`view`] is what it prints, including a small A2UI renderer.
//! - [`trace`] is the same conversation one level down — events exactly as they
//!   arrived, and a resume built without a session.
//! - [`fake`] is the backend: chunked text, tool arguments split mid-escape,
//!   parallel calls, a run that never finishes, and hand-framed streams the
//!   protocol forbids.
//! - [`board`] is the client's own model of the agent's state, declared
//!   independently of the server's.
//!
//! Nothing needs a key or a network beyond loopback.

pub mod board;
pub mod fake;
pub mod trace;
pub mod view;
pub mod watch;

pub use board::Board;
pub use watch::{Console, Policy, Watch};

use ag_ui::Event;
use ag_ui::client::transport::ReplayTransport;

/// Reads the tools this client is willing to have called.
///
/// # Why a client needs this at all
///
/// In AG-UI the *client* offers the tools and the agent picks from them, so an
/// agent that executes `add_task` only sees it if the front-end sent it. There
/// is no handshake: nothing in the protocol lets a generic client ask an agent
/// what it needs, and an agent handed none simply fails the run. A client that
/// is not written against one specific agent therefore has to be *configured*
/// with the tool set, which is what this loads.
///
/// # Errors
///
/// The file's contents, if they are not a JSON array of tool definitions.
pub fn load_tools(json: &str) -> serde_json::Result<Vec<ag_ui::Tool>> {
    serde_json::from_str(json)
}

/// Reads a recorded run — a JSON array of events — into a transport.
///
/// The reason [`Transport`](ag_ui::client::Transport) is a trait: a fixture on
/// disk substitutes for a server, and nothing above it changes. `board-watch
/// replay` is the whole client with the network taken out.
///
/// # Errors
///
/// The file's contents, if they are not a JSON array of events this build
/// knows. An unknown event type is an error rather than a skipped line — see
/// `docs/DESIGN.md`.
pub fn replay_fixture(json: &str) -> serde_json::Result<ReplayTransport> {
    let runs: Vec<Vec<Event>> = serde_json::from_str(json)?;
    Ok(ReplayTransport::with_runs(runs))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_fixture_is_a_list_of_runs() {
        let transport = replay_fixture(
            r#"[[{"type":"RUN_STARTED","threadId":"t","runId":"r"}],
                [{"type":"RUN_STARTED","threadId":"t","runId":"r2"}]]"#,
        )
        .expect("two runs");
        assert_eq!(transport.remaining(), 2);
    }

    /// The protocol's own commitment, from the consuming side: an event this
    /// build does not know stops the load rather than being skipped.
    #[test]
    fn an_unknown_event_type_is_refused() {
        let error = replay_fixture(r#"[[{"type":"TELEPATHY","vibes":9}]]"#)
            .expect_err("an unknown event should not load");
        assert!(error.to_string().contains("TELEPATHY"), "{error}");
    }
}
