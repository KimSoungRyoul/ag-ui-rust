//! The three flows the README shows, run against a real server.
//!
//! Nothing here is mocked: the agent is mounted with `route_agui` on a loopback
//! port, and the client is `ag-ui-client`'s own HTTP transport. The terminal
//! flows drive [`task_board::chat::converse`] — the same code the binary runs —
//! with a scripted stdin and a `Vec<u8>` for a screen, so the transcripts in
//! `README.md` are assertions rather than illustrations.

use ag_ui_a2ui::message::AgentPayload;
use ag_ui_a2ui::toolkit::envelope::{is_operations_envelope, unwrap_operations_envelope};
use ag_ui_client::transport::HttpTransport;
use ag_ui_client::{HttpAgent, RunEnd, RunParams, Session, Update};
use ag_ui_core::{Event, EventType, Message};
use futures_util::StreamExt as _;
use serde_json::Value;
use task_board::board::{self, Board};
use task_board::chat::Terminal;
use task_board::{ROUTE, TaskBoard, chat, router};
use tokio::net::TcpListener;

/// Mounts the scripted agent on a free loopback port and returns its URL.
async fn serve() -> String {
    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .expect("a free port on loopback");
    let addr = listener.local_addr().expect("the bound address");

    tokio::spawn(async move {
        axum::serve(listener, router(TaskBoard::scripted()))
            .await
            .expect("the server to run");
    });

    format!("http://{addr}{ROUTE}")
}

/// A session offering the tools the agent expects, as the binary's `chat` does.
fn session(url: &str, thread: &str) -> Session<HttpTransport, Board> {
    let transport = HttpTransport::new(url).expect("a valid endpoint URL");
    Session::builder(transport, thread)
        .tools(board::tools())
        .build()
}

/// Runs `script` through the real terminal client and returns what it printed.
async fn transcript(session: &mut Session<HttpTransport, Board>, script: &str) -> String {
    let mut terminal = Terminal::new(script.as_bytes(), Vec::new()).echoing();
    chat::converse(session, &mut terminal)
        .await
        .expect("a Vec never fails to be written to");

    let printed = String::from_utf8(terminal.into_output()).expect("the client prints UTF-8");
    assert!(
        !printed.contains("  !!"),
        "the run reported an error:\n{printed}"
    );
    printed
}

/// The A2UI operations from the last surface the agent shipped.
fn last_surface(session: &Session<HttpTransport, Board>) -> Vec<ag_ui_a2ui::AgentMessage> {
    let envelope = session
        .messages()
        .iter()
        .rev()
        .find_map(|message| match message {
            Message::Tool(tool) => serde_json::from_str::<Value>(&tool.content)
                .ok()
                .filter(is_operations_envelope),
            _ => None,
        })
        .expect("the agent should have shipped a surface");

    unwrap_operations_envelope(&envelope).expect("the envelope should unwrap")
}

/// Every line the surface draws as, read out of the transcript's box.
fn drawn(printed: &str) -> Vec<&str> {
    printed
        .lines()
        .filter_map(|line| line.trim().strip_prefix("│ "))
        .collect()
}

#[tokio::test(flavor = "multi_thread")]
async fn adding_listing_and_completing_moves_the_board_and_redraws_it() {
    let url = serve().await;
    let mut session = session(&url, "happy");

    let printed = transcript(
        &mut session,
        "add draft the agenda, book the room\nestimate 2 45\nlist\ncomplete 1\n",
    )
    .await;

    // Two adds, one tool call each, with the arguments the client can read.
    assert!(
        printed.contains(r#"· add_task({"title":"draft the agenda"})"#),
        "{printed}"
    );
    assert!(
        printed.contains(r#"· add_task({"title":"book the room"})"#),
        "{printed}"
    );
    assert!(
        printed.contains(r#"→ {"id":2,"title":"book the room"}"#),
        "{printed}"
    );

    // The state arrived and deserialized into `Board` — one publish per add,
    // so the client saw the board grow rather than appear.
    assert!(printed.contains("[state] 1 open · 0 done"), "{printed}");
    assert!(printed.contains("[state] 2 open · 0 done"), "{printed}");

    // The reply streamed as text, and the surface drew the finished board.
    assert!(
        printed.contains("agent> Added #1 draft the agenda, #2 book the room."),
        "{printed}"
    );
    assert_eq!(
        drawn(&printed).last(),
        Some(&"[ ] #2 book the room · 45m"),
        "{printed}"
    );
    assert!(
        printed.contains("│ [x] #1 draft the agenda"),
        "completing a task must reach the surface:\n{printed}"
    );

    // And the client's typed mirror of it agrees.
    let board = session.state().expect("a board");
    assert_eq!(board.tasks.len(), 2);
    assert_eq!(board.open(), 1);
    assert_eq!(board.remaining_minutes(), 45);
    assert_eq!(board.tasks[0].label(), "#1 draft the agenda");
}

#[tokio::test(flavor = "multi_thread")]
async fn clearing_the_board_asks_a_human_first_and_the_answer_decides() {
    let url = serve().await;
    let mut session = session(&url, "hitl");

    // Declined, then approved. The board has to survive the first and not the
    // second, which is the whole of the interrupt round trip.
    let printed = transcript(
        &mut session,
        "add write the retro notes\nclear\nn\nclear\ny\n",
    )
    .await;

    assert_eq!(
        printed
            .matches("?? Clear the board? 1 task(s) will be removed.")
            .count(),
        2,
        "both clears must pause:\n{printed}"
    );
    assert!(
        printed.contains("agent> Left the board alone."),
        "{printed}"
    );
    assert!(
        printed.contains(r#"· clear_board({})"#),
        "only the approved clear runs the tool:\n{printed}"
    );
    assert_eq!(
        printed.matches("· clear_board(").count(),
        1,
        "the declined clear must not run it:\n{printed}"
    );
    assert!(
        printed.contains("[state] nothing on the board"),
        "{printed}"
    );

    let board = session.state().expect("a board");
    assert!(board.tasks.is_empty(), "{board:?}");
    // Ids keep counting past a clear, so a stale reference cannot resolve.
    assert_eq!(board.next_id, 1);
}

/// The pause is a `RunFinished` with an interrupt outcome, and the answer
/// travels on a *second* request. Driven without the terminal so the shape is
/// visible.
#[tokio::test(flavor = "multi_thread")]
async fn a_paused_run_ends_as_interrupted_and_resumes_as_its_own_run() {
    let url = serve().await;
    let mut session = session(&url, "pause");

    drain(&mut session, "add ship the SDK").await;

    let updates = drain(&mut session, "clear").await;
    let interrupt = match updates.last() {
        Some(Update::Done(RunEnd::Interrupted { interrupts })) => {
            assert_eq!(interrupts.len(), 1, "{interrupts:?}");
            interrupts[0].clone()
        }
        other => panic!("a paused run must end as Interrupted, not {other:?}"),
    };
    assert_eq!(interrupt.id, task_board::agent::CLEAR_INTERRUPT);
    assert_eq!(interrupt.reason, "tool_approval");
    assert!(
        interrupt.response_schema.is_some(),
        "the client needs a schema to build a form from"
    );
    assert_eq!(session.interrupts(), std::slice::from_ref(&interrupt));

    let mut run = session.resume(&interrupt, serde_json::json!({"confirm": true}));
    let mut updates = Vec::new();
    while let Some(update) = run.next().await {
        updates.push(update);
    }
    drop(run);

    assert!(
        matches!(updates.last(), Some(Update::Done(RunEnd::Success { .. }))),
        "{updates:?}"
    );
    assert!(session.interrupts().is_empty());
    assert!(session.state().expect("a board").tasks.is_empty());
    // The resumed run is a run of its own, in the same thread.
    assert_eq!(
        session.applier().run_id().map(|id| id.as_str()),
        Some("pause-run-3")
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn a_second_run_in_the_same_thread_carries_the_conversation_and_the_board() {
    let url = serve().await;
    let mut session = session(&url, "carry");

    transcript(&mut session, "add draft the agenda\n").await;
    let after_first = session.messages().len();
    let created = last_surface(&session);
    assert!(
        matches!(created[0].payload, AgentPayload::CreateSurface(_)),
        "the first render creates the surface: {created:?}"
    );

    let printed = transcript(&mut session, "add book the room\n").await;

    // The board carried: the second task got id 2, which is only possible if
    // the first run's state came back in the second run's request.
    assert!(
        printed.contains("agent> Added #2 book the room."),
        "{printed}"
    );
    assert_eq!(
        drawn(&printed),
        [
            "Workshop board",
            "2 open · 0 done",
            "[ ] #1 draft the agenda",
            "[ ] #2 book the room"
        ]
    );

    // The conversation carried too, and the agent used it: a surface already on
    // screen is *updated*, never created again, and the only way it can know is
    // the history the client sent.
    assert!(session.messages().len() > after_first);
    let updated = last_surface(&session);
    assert!(
        updated
            .iter()
            .all(|op| !matches!(op.payload, AgentPayload::CreateSurface(_))),
        "a surface already on screen must not be re-created: {updated:?}"
    );
}

/// The low-level API, and the only test here that reads the wire itself:
/// [`HttpAgent`] hands the events over unassembled, exactly as sent.
#[tokio::test(flavor = "multi_thread")]
async fn the_event_stream_is_ordered_as_the_protocol_requires() {
    let url = serve().await;
    let agent = HttpAgent::builder(&url)
        .header("x-example", "task-board")
        .build()
        .expect("a valid endpoint URL");

    let params = RunParams::new("wire", "r1")
        .user("m1", "add ship the SDK")
        .tools(board::tools());

    let events: Vec<Event> = agent
        .run(params)
        .map(|event| event.expect("the stream should not break"))
        .collect()
        .await;

    // "Added #1 ship the SDK. 1 open · 0 done" — one delta per word.
    const REPLY_WORDS: usize = 10;

    let mut expected = vec![
        EventType::RunStarted,
        EventType::StepStarted,
        // One `ctx.think()`.
        EventType::ReasoningStart,
        EventType::ReasoningMessageStart,
        EventType::ReasoningMessageContent,
        EventType::ReasoningMessageEnd,
        EventType::ReasoningEnd,
        // `add_task`, executed here and answered here.
        EventType::ToolCallStart,
        EventType::ToolCallArgs,
        EventType::ToolCallEnd,
        EventType::ToolCallResult,
        // The board's first publish of a run is always a snapshot.
        EventType::StateSnapshot,
        EventType::TextMessageStart,
    ];
    expected.extend(std::iter::repeat_n(
        EventType::TextMessageContent,
        REPLY_WORDS,
    ));
    expected.extend([
        EventType::TextMessageEnd,
        // The A2UI surface, in a tool result envelope.
        EventType::ToolCallStart,
        EventType::ToolCallArgs,
        EventType::ToolCallEnd,
        EventType::ToolCallResult,
        EventType::StepFinished,
        EventType::RunFinished,
    ]);

    let types: Vec<EventType> = events.iter().map(Event::event_type).collect();
    assert_eq!(types, expected, "{types:?}");

    let Some(Event::RunFinished(finished)) = events.last() else {
        panic!("the last event must be RUN_FINISHED: {:?}", events.last());
    };
    assert_eq!(finished.thread_id.as_str(), "wire");
    assert_eq!(finished.run_id.as_str(), "r1");
}

/// The encoding of a state publish is a size decision, and both outcomes reach
/// the same client state. Which one goes out is not the agent's choice, so this
/// pins it at the size where it flips rather than asserting a fixed answer.
#[tokio::test(flavor = "multi_thread")]
async fn state_publishes_pick_the_smaller_of_a_snapshot_and_a_patch() {
    let url = serve().await;
    let agent = HttpAgent::http(&url).expect("a valid endpoint URL");

    // Two publishes on a board small enough that resending it beats patching
    // it, and two on a board where it does not.
    let tiny = state_events(&agent, "add a, b").await;
    assert_eq!(
        tiny,
        [EventType::StateSnapshot, EventType::StateSnapshot],
        "a two-word board is cheaper to resend than to patch"
    );

    let roomy = state_events(
        &agent,
        "add write the workshop agenda and circulate it, \
         book the large meeting room for thursday",
    )
    .await;
    assert_eq!(
        roomy,
        [EventType::StateSnapshot, EventType::StateDelta],
        "the first publish of a run is always a snapshot; the second is a patch \
         once the patch is the smaller of the two"
    );

    // Whichever went out, the client lands in the same place.
    let mut session = session(&url, "encodings");
    transcript(
        &mut session,
        "add write the workshop agenda and circulate it, book the large meeting room for thursday\n",
    )
    .await;
    let board = session.state().expect("a board");
    assert_eq!(board.tasks.len(), 2);
    assert_eq!(
        board.tasks[1].label(),
        "#2 book the large meeting room for thursday"
    );
}

/// The `STATE_*` events one run puts on the wire, in order.
async fn state_events(agent: &HttpAgent, said: &str) -> Vec<EventType> {
    let params = RunParams::new("encodings", "r1")
        .user("m1", said)
        .tools(board::tools());

    agent
        .run(params)
        .map(|event| event.expect("the stream should not break"))
        .map(|event| event.event_type())
        .filter(|kind| {
            std::future::ready(matches!(
                kind,
                EventType::StateSnapshot | EventType::StateDelta
            ))
        })
        .collect()
        .await
}

/// Drains one run and returns everything it reported.
async fn drain(session: &mut Session<HttpTransport, Board>, said: &str) -> Vec<Update<Board>> {
    let mut run = session.send(said);
    let mut updates = Vec::new();
    while let Some(update) = run.next().await {
        updates.push(update);
    }
    updates
}
