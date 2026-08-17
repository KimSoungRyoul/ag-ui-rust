//! State published as snapshots and as RFC 6902 patches, across the wire.
//!
//! The server decides per publish whether to send `STATE_SNAPSHOT` or
//! `STATE_DELTA`, and the client has to end up in the same place either way.
//! The agent here forces both decisions, and two of its keys contain the
//! characters RFC 6901 escapes (`/` and `~`) — an unescaped pointer applies the
//! patch to a *nested* key instead, which nothing but a round trip catches.

mod common;

use std::collections::BTreeMap;

use ag_ui_client::{Agent as ClientAgent, RunParams, Session, Update};
use ag_ui_core::{Event, EventType, RunOutcome};
use ag_ui_server::{Agent, Result, RunContext};
use common::{serve, transport};
use futures_util::StreamExt as _;
use serde::{Deserialize, Serialize};

/// Long enough that a one-field change is cheaper to patch than to resend.
const NOTE: &str = "the document the user is editing, at a length that makes resending it wasteful";

/// The shared document.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Board {
    revision: u32,
    title: String,
    notes: Vec<String>,
    tags: Vec<String>,
    /// Keys deliberately contain `/` and `~`, the two characters a JSON Pointer
    /// has to escape.
    counts: BTreeMap<String, u32>,
}

/// Publishes seven times, sized so the server picks each encoding at least
/// once.
struct Editor;

impl Agent for Editor {
    type State = Board;

    async fn run(&self, ctx: &mut RunContext<Board>) -> Result<RunOutcome> {
        // 1 — the first publish is a snapshot whatever its size.
        ctx.update_state(|board| {
            board.revision = 1;
            board.title = "Draft".to_owned();
            board.notes = vec![NOTE.to_owned()];
        })?;
        // 2 — one small field of a large document: a patch is smaller.
        ctx.update_state(|board| board.revision = 2)?;
        // 3 — the document shrinks to almost nothing: the patch describing that
        //     costs more than the document, so this falls back to a snapshot.
        ctx.update_state(|board| {
            board.title = "Shipped".to_owned();
            board.notes.clear();
        })?;
        // 4-7 — small additive changes, including two escaped pointers.
        ctx.update_state(|board| {
            board.counts.insert("a/b".to_owned(), 7);
        })?;
        ctx.update_state(|board| {
            board.counts.insert("c~d".to_owned(), 9);
        })?;
        ctx.update_state(|board| {
            board.counts.insert("a/b".to_owned(), 8);
        })?;
        ctx.update_state(|board| board.tags.push("urgent".to_owned()))?;

        Ok(RunOutcome::Success)
    }
}

/// What the agent's state looked like after each publish, in order.
fn published() -> Vec<Board> {
    let mut board = Board {
        revision: 1,
        title: "Draft".to_owned(),
        notes: vec![NOTE.to_owned()],
        ..Default::default()
    };
    let mut history = vec![board.clone()];

    board.revision = 2;
    history.push(board.clone());

    board.title = "Shipped".to_owned();
    board.notes.clear();
    history.push(board.clone());

    for (key, value) in [("a/b", 7), ("c~d", 9), ("a/b", 8)] {
        board.counts.insert(key.to_owned(), value);
        history.push(board.clone());
    }

    board.tags.push("urgent".to_owned());
    history.push(board);
    history
}

/// The encoding the server chose for each publish, read off the raw stream.
#[tokio::test(flavor = "multi_thread")]
async fn the_server_picks_snapshots_and_deltas_and_both_reach_the_client() {
    let url = serve(Editor).await;
    let agent = ClientAgent::new(transport(&url));

    let mut encodings = Vec::new();
    let mut events = agent.run(RunParams::new("board", "board-run-1"));
    while let Some(event) = events.next().await {
        let event = event.expect("the stream should not break");
        if matches!(event, Event::StateSnapshot(_) | Event::StateDelta(_)) {
            encodings.push(event.event_type());
        }
    }

    use EventType::{StateDelta, StateSnapshot};
    assert_eq!(
        encodings,
        [
            StateSnapshot, // first publish
            StateDelta,    // one field of a large document
            StateSnapshot, // the document shrank; the patch would cost more
            StateDelta,
            StateDelta,
            StateDelta,
            StateDelta,
        ],
        "this scenario is only meaningful if both encodings are exercised"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn the_clients_typed_state_matches_the_agents_after_every_publish() {
    let url = serve(Editor).await;
    let mut session = Session::<_, Board>::new(transport(&url), "board");

    let mut states = Vec::new();
    {
        let mut run = session.send("tidy the board");
        while let Some(update) = run.next().await {
            match update {
                Update::State(state) => states.push(state),
                Update::Error(error) => panic!("state should apply cleanly: {error}"),
                _ => {}
            }
        }
    }

    let expected = published();
    assert_eq!(states, expected, "every intermediate state must agree");
    assert_eq!(session.state(), expected.last());
}

/// The delta path specifically: a pointer into a key containing `/` or `~` has
/// to come back out as that key, not as a nested object.
#[tokio::test(flavor = "multi_thread")]
async fn escaped_json_pointers_patch_the_key_they_name() {
    let url = serve(Editor).await;
    let mut session = Session::<_, Board>::new(transport(&url), "board");

    {
        let mut run = session.send("tidy the board");
        while run.next().await.is_some() {}
    }

    let counts = &session.raw_state()["counts"];
    assert_eq!(
        counts,
        &serde_json::json!({"a/b": 8, "c~d": 9}),
        "a mis-escaped pointer nests the key instead of naming it"
    );
    assert!(
        counts.get("a").is_none() && counts.get("c").is_none(),
        "an unescaped pointer would have created these: {counts}"
    );
}
