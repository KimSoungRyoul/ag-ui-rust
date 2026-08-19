//! What the client does with the streams a real producer sends.
//!
//! Every test drives [`board_watch::watch`] — the same function the binary
//! runs — with a scripted script and a `Vec<u8>` for a screen, so the
//! transcripts in `README.md` are assertions rather than illustrations.
//!
//! Two backends, on real loopback sockets. [`board_watch::fake`] is the
//! awkward one, written to produce the shapes provider adapters produce.
//! `task-board` is the round-one example, unmodified: a client that has only
//! ever talked to its own fake server has not been tested against anything.

use std::time::Duration;

use ag_ui::client::transport::HttpTransport;
use ag_ui::client::{HttpAgent, Session};
use ag_ui::{Message, Tool};
use board_watch::view::message_count;
use board_watch::watch::{Console, Policy, Watch};
use board_watch::{Board, fake, load_tools, replay_fixture, trace, watch};
use serde_json::Value;
use tokio::net::TcpListener;
use tokio::sync::mpsc::{UnboundedReceiver, unbounded_channel};

/// A hang is one of the things being ruled out, so every wait has a deadline.
const DEADLINE: Duration = Duration::from_secs(10);

/// Mounts the awkward agent on a free loopback port and returns its base URL.
async fn serve_fake() -> String {
    serve(fake::router(fake::Awkward::new())).await
}

/// The same, reporting each run's cancellation state as its future exits.
async fn serve_reporting() -> (String, UnboundedReceiver<bool>) {
    let (tx, rx) = unbounded_channel();
    (serve(fake::router(fake::Awkward::reporting(tx))).await, rx)
}

/// The round-one example's agent, unmodified.
async fn serve_task_board() -> String {
    let url = serve(task_board::router(task_board::TaskBoard::scripted())).await;
    format!("{url}{}", task_board::ROUTE)
}

/// Binds port 0 so tests run concurrently, and returns `http://addr`.
async fn serve(app: axum::Router) -> String {
    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .expect("a free port on loopback");
    let addr = listener.local_addr().expect("the bound address");

    tokio::spawn(async move {
        axum::serve(listener, app).await.expect("the server to run");
    });
    format!("http://{addr}")
}

/// A session over the client's real HTTP transport.
fn session(url: &str, thread: &str) -> Session<HttpTransport, Board> {
    Session::builder(
        HttpTransport::new(url).expect("a valid endpoint URL"),
        thread,
    )
    .build()
}

/// The same, offering tools and with verification off if asked.
fn configured(
    url: &str,
    thread: &str,
    tools: Vec<Tool>,
    verify: bool,
) -> Session<HttpTransport, Board> {
    Session::builder(
        HttpTransport::new(url).expect("a valid endpoint URL"),
        thread,
    )
    .tools(tools)
    .verify(verify)
    .build()
}

/// Runs `script` through the real client and returns what it printed.
async fn transcript<T: ag_ui::client::Transport>(
    session: &mut Session<T, Board>,
    settings: Watch,
    script: &str,
) -> String {
    let mut console = Console::new(script.as_bytes(), Vec::new()).echoing();
    watch::watch(session, settings, &mut console)
        .await
        .expect("a Vec never fails to be written to");
    String::from_utf8(console.into_output()).expect("the client prints UTF-8")
}

/// Every tool call the conversation holds, as `(name, arguments)`.
///
/// No `T: Transport`: reading a session is not making a request, and the bound
/// lives on the impl blocks that are.
fn tool_calls<T, S>(session: &Session<T, S>) -> Vec<(String, String)> {
    session
        .messages()
        .iter()
        .filter_map(|message| match message {
            Message::Assistant(assistant) => assistant.tool_calls.as_ref(),
            _ => None,
        })
        .flatten()
        .map(|call| (call.function.name.clone(), call.function.arguments.clone()))
        .collect()
}

/// The assistant text the conversation holds.
fn said<T, S>(session: &Session<T, S>) -> Vec<String> {
    session
        .messages()
        .iter()
        .filter_map(|message| match message {
            Message::Assistant(assistant) => assistant.content.clone(),
            _ => None,
        })
        .collect()
}

// ---- chunk normalization ------------------------------------------------

/// Five `TEXT_MESSAGE_CHUNK` events, the id on the first only, become one
/// message — including a grapheme split across three of them.
#[tokio::test(flavor = "multi_thread")]
async fn chunked_text_rejoins_into_one_message() {
    let url = serve_fake().await;
    let mut session = session(&format!("{url}{}", fake::ROUTE), "chunks");

    let printed = transcript(
        &mut session,
        Watch {
            fragments: true,
            ..Watch::default()
        },
        "chunks\n",
    )
    .await;

    // The transcript shows the fragmentation…
    assert!(
        printed.contains("[Chunked text arrives in frag][ments, and the client rejoins ]"),
        "{printed}"
    );
    // …and the assembled message shows it did not matter.
    assert_eq!(
        said(&session),
        [
            "Chunked text arrives in fragments, and the client rejoins them — emoji included: 👩\u{200d}💻."
        ],
    );
    assert!(!printed.contains("  error"), "{printed}");
}

/// Tool arguments split at hostile offsets — including between a backslash and
/// the character it escapes — reassemble into JSON that parses.
#[tokio::test(flavor = "multi_thread")]
async fn tool_arguments_split_mid_escape_reassemble_into_valid_json() {
    let url = serve_fake().await;
    let mut session = session(&format!("{url}{}", fake::ROUTE), "call");

    let printed = transcript(&mut session, Watch::default(), "call\n").await;
    assert!(!printed.contains("  error"), "{printed}");

    let calls = tool_calls(&session);
    assert_eq!(calls.len(), 1, "{calls:?}");
    let (name, arguments) = &calls[0];
    assert_eq!(name, "add_task");

    // The whole point: no fragment of this was valid JSON on its own.
    let parsed: Value = serde_json::from_str(arguments).expect("the rejoined arguments");
    assert_eq!(parsed["title"], "ship the SDK");
    assert_eq!(parsed["depth"], 3);
    // The `\` and the `n` it escapes arrived in different events.
    assert_eq!(parsed["note"], "line\nbreak");
}

/// Two calls in flight at once do not splice into each other.
///
/// The events interleave — `args(a) args(b) args(a) end(a) end(b)` — so a
/// client keyed on "the open call" instead of on the id produces one call with
/// both sets of arguments concatenated.
#[tokio::test(flavor = "multi_thread")]
async fn two_calls_in_flight_keep_their_arguments_apart() {
    let url = serve_fake().await;
    let mut session = session(&format!("{url}{}", fake::ROUTE), "parallel");

    let printed = transcript(&mut session, Watch::default(), "parallel\n").await;
    assert!(!printed.contains("  error"), "{printed}");

    let calls = tool_calls(&session);
    assert_eq!(calls.len(), 2, "{calls:?}");
    assert_eq!(calls[0].1, r#"{"title":"write it down"}"#);
    assert_eq!(calls[1].1, r#"{"title":"read it back"}"#);

    // And the renderer keeps them on separate lines, which the naive one does
    // not: it prints a prefix on start and a newline on end.
    assert!(
        printed.contains("  call   add_task {\"title\":\"write it down\"}\n"),
        "{printed}"
    );
    assert!(
        printed.contains("  call   add_task {\"title\":\"read it back\"}\n"),
        "{printed}"
    );
}

/// Reasoning, text and a call, none of them bracketed: each stream is closed
/// only by the next one starting, and the last by the end of the run.
#[tokio::test(flavor = "multi_thread")]
async fn unbracketed_streams_are_closed_by_what_follows_them() {
    let url = serve_fake().await;
    let mut session = session(&format!("{url}{}", fake::ROUTE), "mixed");

    let printed = transcript(&mut session, Watch::default(), "mixed\n").await;
    assert!(!printed.contains("  error"), "{printed}");

    assert!(
        printed.contains("  think  three streams, no brackets"),
        "{printed}"
    );
    assert_eq!(said(&session), ["Reading the board, then adding to it."]);
    // The final call had no terminator of its own; the end of the run is what
    // closed it, and the client still assembled it whole.
    assert_eq!(
        tool_calls(&session),
        [(
            "add_task".to_owned(),
            r#"{"title":"unbracketed"}"#.to_owned()
        )]
    );
}

// ---- interrupts ---------------------------------------------------------

/// A run paused on two decisions is answered in **one** request, and the
/// answers reach the agent as it branches on them.
#[tokio::test(flavor = "multi_thread")]
async fn a_pause_on_two_decisions_is_answered_in_one_request() {
    let url = serve_fake().await;
    let endpoint = format!("{url}{}", fake::ROUTE);

    for (policy, expected) in [
        (Policy::Approve, "Both approved. Booked."),
        (Policy::Decline, "Both declined. Nothing booked."),
    ] {
        let mut session = session(&endpoint, "pause");
        let printed = transcript(
            &mut session,
            Watch {
                policy,
                ..Watch::default()
            },
            "approve\n",
        )
        .await;

        assert!(printed.contains("  done   interrupted on 2"), "{printed}");
        assert_eq!(
            printed.matches("  pause  ").count(),
            2,
            "one update per pending decision:\n{printed}"
        );
        assert_eq!(said(&session).last().map(String::as_str), Some(expected));
        // Two decisions, two runs — not three. Answering one per request never
        // terminates, because the agent only sees what this request carries.
        assert_eq!(
            session.applier().run_id().map(|id| id.as_str()),
            Some("pause-run-2"),
            "{printed}"
        );
        assert!(session.interrupts().is_empty());
    }
}

/// Answering one yes and one no reaches the agent as a mixed batch.
#[tokio::test(flavor = "multi_thread")]
async fn a_mixed_answer_reaches_the_agent_as_one_batch() {
    let url = serve_fake().await;
    let mut session = session(&format!("{url}{}", fake::ROUTE), "mixed-answer");

    // `Ask` reads the answers off the script, one line per decision.
    let printed = transcript(&mut session, Watch::default(), "approve\ny\nn\n").await;

    assert!(
        printed.contains("  answer approve-budget · approved"),
        "{printed}"
    );
    assert!(
        printed.contains("  answer confirm-date · declined"),
        "{printed}"
    );
    assert_eq!(
        said(&session).last().map(String::as_str),
        Some("Declined: confirm-date. Nothing booked."),
    );
}

// ---- cancellation -------------------------------------------------------

/// Letting go of the stream stops the run at the far end.
///
/// Polling is what pulls bytes, so dropping the `RunStream` is the whole of
/// client-side cancellation — but that is invisible from here, which is why the
/// agent reports its own cancellation state as its future exits.
#[tokio::test(flavor = "multi_thread")]
async fn dropping_the_stream_mid_run_cancels_the_agent() {
    let (url, mut exits) = serve_reporting().await;
    let mut session = session(&format!("{url}{}", fake::ROUTE), "stop");

    let printed = transcript(
        &mut session,
        Watch {
            stop_after: Some(3),
            ..Watch::default()
        },
        "slow\n",
    )
    .await;

    assert!(
        printed.contains("  stop   dropped the stream after 3 updates"),
        "{printed}"
    );
    // The agent never finished, so the client never saw a terminal update.
    assert!(!printed.contains("  done   "), "{printed}");

    let cancelled = tokio::time::timeout(DEADLINE, exits.recv())
        .await
        .expect("the agent's future should exit once the client goes away");
    assert_eq!(
        cancelled,
        Some(true),
        "the run should have been cancelled, not merely dropped"
    );

    // And the session is still usable: the next run is a run like any other.
    let printed = transcript(&mut session, Watch::default(), "chunks\n").await;
    assert!(printed.contains("  done   success"), "{printed}");
}

// ---- verification -------------------------------------------------------

/// A stream the protocol forbids is reported, and the offending event is not
/// applied. Turning verification off applies it anyway.
#[tokio::test(flavor = "multi_thread")]
async fn a_malformed_stream_is_diagnosed_and_the_diagnosis_can_be_declined() {
    let url = serve_fake().await;
    let endpoint = format!("{url}/raw/unbracketed");

    let mut verified = configured(&endpoint, "raw", Vec::new(), true);
    let printed = transcript(&mut verified, Watch::default(), "go\n").await;
    assert!(
        printed.contains(r#"protocol violation: TEXT_MESSAGE_CONTENT for message "ghost""#),
        "{printed}"
    );
    assert!(
        said(&verified).is_empty(),
        "an event the producer should not have sent must not be applied"
    );

    let mut tolerant = configured(&endpoint, "raw", Vec::new(), false);
    let printed = transcript(&mut tolerant, Watch::default(), "go\n").await;
    assert!(!printed.contains("protocol violation"), "{printed}");
    assert_eq!(
        said(&tolerant),
        ["text nobody opened"],
        "the applier stays tolerant; what is lost is the diagnosis"
    );
}

/// A run that simply stops is reported as a failure, not as silence.
#[tokio::test(flavor = "multi_thread")]
async fn a_truncated_stream_ends_the_run_rather_than_hanging() {
    let url = serve_fake().await;
    let mut session = session(&format!("{url}/raw/truncated"), "cut");

    let printed = transcript(&mut session, Watch::default(), "go\n").await;
    assert!(printed.contains("  error"), "{printed}");
    assert!(printed.contains("  done   failed"), "{printed}");
    // The message the producer left open was closed on the way out, so a view
    // that hides its spinner on `Ended` is not left spinning.
    assert_eq!(said(&session), ["half a sen"]);
}

// ---- a second agent, and the lifecycle ----------------------------------

/// The client against an agent it did not write, over real HTTP.
#[tokio::test(flavor = "multi_thread")]
async fn the_client_drives_an_agent_it_did_not_write() {
    let url = serve_task_board().await;
    let mut session = configured(&url, "board", task_board::board::tools(), true);

    let printed = transcript(
        &mut session,
        Watch::default(),
        "add draft the agenda, book the room\ncomplete 1\n",
    )
    .await;
    assert!(!printed.contains("  error"), "{printed}");

    // The state deserialized into *this* client's view model, which was
    // declared independently of the agent's.
    let board = session.state().expect("a board");
    assert_eq!(board.tasks.len(), 2);
    assert_eq!(board.tasks[0].line(), "[x] #1 draft the agenda");
    assert_eq!(board.summary(), "1 open · 1 done");

    // The A2UI surface the agent shipped, drawn from the conversation rather
    // than from the tool result that carried it.
    assert!(printed.contains("  surface"), "{printed}");
    assert!(printed.contains("    [x] #1 draft the agenda"), "{printed}");
    assert!(
        printed.contains("· surface task-board (6)"),
        "the panel names the surface recovered from history:\n{printed}"
    );
}

/// The conversation and the board carry from one run to the next.
#[tokio::test(flavor = "multi_thread")]
async fn a_second_run_in_the_same_thread_carries_what_the_first_established() {
    let url = serve_task_board().await;
    let mut session = configured(&url, "carry", task_board::board::tools(), true);

    transcript(&mut session, Watch::default(), "add draft the agenda\n").await;
    let after_first = session.messages().len();

    let printed = transcript(&mut session, Watch::default(), "add book the room\n").await;
    // Id 2, which is only reachable if the first run's state came back in the
    // second run's request.
    assert!(printed.contains("[ ] #2 book the room"), "{printed}");
    assert!(session.messages().len() > after_first);
    assert_eq!(
        session.applier().run_id().map(|id| id.as_str()),
        Some("carry-run-2")
    );
}

/// An event emitted *inside* an open tool call loses its nesting on the way to
/// the client.
///
/// `task-board` publishes state from inside the call — which the server's
/// handles now allow — so the wire is `START ARGS STATE END RESULT`. An
/// [`Update::State`](ag_ui::client::Update::State) carries no association with
/// the call it arrived during, so a renderer that groups a call onto one line
/// at `ToolCallEnded` prints the state *before* the call. Pinned in both
/// directions: what the wire says, and what the client can say about it.
#[tokio::test(flavor = "multi_thread")]
async fn an_event_published_inside_a_call_loses_its_nesting() {
    let url = serve_task_board().await;

    // What the wire carries: the call opens, then the state, then the close.
    let agent = HttpAgent::http(&url).expect("a valid endpoint URL");
    let mut raw = Vec::new();
    trace::trace(
        &agent,
        "nested",
        "add one thing",
        task_board::board::tools(),
        false,
        &mut raw,
    )
    .await
    .expect("a Vec never fails to be written to");
    let raw = String::from_utf8(raw).expect("UTF-8");

    let position = |needle: &str| {
        raw.find(needle)
            .unwrap_or_else(|| panic!("{needle}:\n{raw}"))
    };
    assert!(
        position("TOOL_CALL_START") < position("STATE_SNAPSHOT"),
        "the state is published inside the open call:\n{raw}"
    );
    assert!(
        position("STATE_SNAPSHOT") < position("TOOL_CALL_END"),
        "…and before it closes:\n{raw}"
    );

    // What the assembled view can say: the call is one line, printed when it
    // closes, so the state that happened during it comes first.
    let mut session = configured(&url, "nested-watch", task_board::board::tools(), true);
    let printed = transcript(&mut session, Watch::default(), "add one thing\n").await;
    let seen = |needle: &str| {
        printed
            .find(needle)
            .unwrap_or_else(|| panic!("{needle}:\n{printed}"))
    };
    assert!(
        seen("  state  ") < seen("  call   add_task"),
        "grouping a call onto one line reorders what happened inside it:\n{printed}"
    );
}

/// Drawing in arrival order shows the nesting that grouping hides.
///
/// Round two reported that a call published-into cannot be drawn with its state
/// inside it. That was too strong, and this is the correction: the nesting *is*
/// recoverable, because arrival order carries it — what is not recoverable is
/// drawing a call as one line *and* keeping the order, since the line cannot be
/// written until the call closes. Tagging each line with the call id is what
/// makes the faithful rendering legible under parallel calls, and without the
/// tag it is not.
#[tokio::test(flavor = "multi_thread")]
async fn arrival_order_rendering_shows_the_nesting_that_grouping_hides() {
    let url = serve_task_board().await;
    let tools = task_board::board::tools();

    let mut grouped = configured(&url, "grouped", tools.clone(), true);
    let grouped = transcript(&mut grouped, Watch::default(), "add draft the agenda\n").await;

    let mut ordered = configured(&url, "ordered", tools, true);
    let ordered = transcript(
        &mut ordered,
        Watch {
            in_order: true,
            ..Watch::default()
        },
        "add draft the agenda\n",
    )
    .await;

    let at = |text: &str, needle: &str| {
        text.find(needle)
            .unwrap_or_else(|| panic!("{needle}:\n{text}"))
    };

    // Grouped: the call is one line, written when it closes, so the state that
    // was published during it is already on screen.
    assert!(
        at(&grouped, "  state  ") < at(&grouped, "  call   add_task"),
        "{grouped}"
    );

    // In arrival order: the state lands between the call's arguments and its
    // end, which is where the wire put it.
    assert!(
        at(&ordered, "  args   ") < at(&ordered, "  state  "),
        "{ordered}"
    );
    assert!(
        at(&ordered, "  state  ") < at(&ordered, "  end    add_task"),
        "{ordered}"
    );

    // Both agree about the board itself; only the drawing differs.
    assert!(grouped.contains("[ ] #1 draft the agenda"), "{grouped}");
    assert!(ordered.contains("[ ] #1 draft the agenda"), "{ordered}");
}

/// Two calls in flight stay legible in arrival order, because each line names
/// its call.
#[tokio::test(flavor = "multi_thread")]
async fn arrival_order_keeps_parallel_calls_apart_by_naming_them() {
    let url = serve_fake().await;
    let mut session = session(&format!("{url}{}", fake::ROUTE), "ordered-parallel");

    let printed = transcript(
        &mut session,
        Watch {
            in_order: true,
            ..Watch::default()
        },
        "parallel\n",
    )
    .await;

    // Both calls open before either closes — the interleaving, on screen.
    let lines: Vec<&str> = printed.lines().map(str::trim_end).collect();
    let calls: Vec<&&str> = lines
        .iter()
        .filter(|line| line.trim_start().starts_with("call   add_task"))
        .collect();
    assert_eq!(calls.len(), 2, "{printed}");
    assert_ne!(calls[0], calls[1], "the id is what separates them");

    // And every argument fragment says which call it belongs to.
    let args: Vec<&&str> = lines
        .iter()
        .filter(|line| line.trim_start().starts_with("args   "))
        .collect();
    assert_eq!(args.len(), 4, "{printed}");
    assert!(args.iter().all(|line| line.contains('(')), "{printed}");
}

/// A run can do work and *then* pause, and resuming carries what the first half
/// produced.
#[tokio::test(flavor = "multi_thread")]
async fn a_run_that_has_already_done_work_can_still_pause() {
    let url = serve_fake().await;
    let mut session = session(&format!("{url}{}", fake::ROUTE), "busy");

    let printed = transcript(
        &mut session,
        Watch {
            policy: Policy::Approve,
            ..Watch::default()
        },
        "busy\n",
    )
    .await;
    assert!(!printed.contains("  error"), "{printed}");

    // Two calls and a state publish before the pause, one more after it.
    assert!(printed.contains("  done   interrupted on 1"), "{printed}");
    assert_eq!(tool_calls(&session).len(), 3, "{printed}");
    assert_eq!(
        session.state().expect("a board").tasks.len(),
        3,
        "the resumed run built on the first half's state:\n{printed}"
    );
    // The first half's tool messages are still in the conversation.
    assert!(
        session.messages().iter().any(|message| matches!(
            message,
            Message::Tool(tool) if tool.content.contains("write it down")
        )),
        "{:?}",
        session.messages()
    );
}

/// The tools a client does not send are tools the agent does not have.
///
/// There is no discovery in AG-UI: an agent cannot ask for a tool, so a generic
/// client that offers none fails against an agent that needs one — loudly, and
/// with the agent's own words.
#[tokio::test(flavor = "multi_thread")]
async fn an_agent_that_needs_a_tool_says_so_when_the_client_offers_none() {
    let url = serve_task_board().await;
    let mut session = configured(&url, "bare", Vec::new(), true);

    let printed = transcript(&mut session, Watch::default(), "add anything\n").await;
    assert!(
        printed.contains("the client offered no add_task tool"),
        "{printed}"
    );
    assert!(printed.contains("  done   failed"), "{printed}");
}

/// A client that wants to be sent A2UI surfaces offers the toolkit's own tool
/// definition, converted rather than retyped.
///
/// Round one found `ToolDefinition::to_value` emitting `input_schema` —
/// Anthropic's key — where `ag_ui::Tool` wants `parameters`, so the
/// obvious `from_value(def.to_value())` produced a tool with an empty schema.
/// The conversion is now a `From` impl and the old name says whose shape it is.
/// This is the regression guard for both halves.
#[test]
fn a_toolkit_tool_definition_can_be_offered_on_a_run() {
    use ag_ui_a2ui::catalog::Catalog;
    use ag_ui_a2ui::toolkit::tools::render_a2ui_tool;

    let definition = render_a2ui_tool(Some(&Catalog::basic()));
    let anthropic = definition.to_anthropic_value();
    assert!(anthropic["input_schema"].is_object(), "{anthropic}");
    assert!(
        anthropic.get("parameters").is_none(),
        "the Anthropic shape is the one that says so in its name: {anthropic}"
    );

    // The AG-UI shape, which is what a run carries.
    let tool = Tool::from(definition.clone());
    assert_eq!(tool.name, ag_ui_a2ui::constants::RENDER_A2UI_TOOL_NAME);
    assert_eq!(tool.parameters, definition.parameters);
    assert!(
        tool.parameters["properties"].is_object(),
        "the schema survives the conversion: {}",
        tool.parameters
    );

    // And it is offerable: a session takes it like any other tool.
    let session: Session<HttpTransport, Board> = Session::builder(
        HttpTransport::new("http://127.0.0.1:1/agent").expect("a valid endpoint URL"),
        "surfaces",
    )
    .tools(vec![tool])
    .build();
    assert_eq!(message_count(&session), 0);
}

/// The tool list shipped for pointing this client at `task-board` is the one
/// that agent actually offers.
#[test]
fn the_bundled_tool_fixture_matches_the_agent_it_is_for() {
    let json = include_str!("../fixtures/task-board-tools.json");
    let tools = load_tools(json).expect("a tool list");
    assert_eq!(tools, task_board::board::tools());
}

// ---- the low level ------------------------------------------------------

/// Pausing and resuming with no session at all: `interrupts_of` reads what the
/// run paused on and `resume_run` builds the request that answers it.
#[tokio::test(flavor = "multi_thread")]
async fn the_low_level_stream_pauses_and_resumes_without_a_session() {
    let url = serve_fake().await;
    let agent = HttpAgent::builder(format!("{url}{}", fake::ROUTE))
        .header("x-board-watch", "test")
        .build()
        .expect("a valid endpoint URL");

    let mut out = Vec::new();
    let count = trace::trace(&agent, "low", "approve", Vec::new(), true, &mut out)
        .await
        .expect("a Vec never fails to be written to");
    let printed = String::from_utf8(out).expect("UTF-8");

    // Two runs, the second built from the first's request plus the answers.
    assert!(printed.contains("--- run 1 · low-run-1"), "{printed}");
    assert!(printed.contains("--- run 2 · low-run-2"), "{printed}");
    assert!(printed.contains("RUN_FINISHED"), "{printed}");
    // Unassembled: the chunk events arrive as chunk events.
    assert!(printed.contains("TEXT_MESSAGE_START"), "{printed}");
    assert!(count >= 6, "{count} events:\n{printed}");
}

/// The raw stream is what a proxy sees: chunk events, not messages.
#[tokio::test(flavor = "multi_thread")]
async fn the_low_level_stream_does_not_assemble_chunks() {
    let url = serve_fake().await;
    let agent = HttpAgent::http(format!("{url}{}", fake::ROUTE)).expect("a valid endpoint URL");

    let mut out = Vec::new();
    trace::trace(&agent, "raw-chunks", "chunks", Vec::new(), false, &mut out)
        .await
        .expect("a Vec never fails to be written to");
    let printed = String::from_utf8(out).expect("UTF-8");

    assert_eq!(
        printed.matches("TEXT_MESSAGE_CHUNK").count(),
        5,
        "five chunks, unassembled:\n{printed}"
    );
    assert!(!printed.contains("TEXT_MESSAGE_START"), "{printed}");
}

// ---- offline ------------------------------------------------------------

/// A recorded run replays through the whole client with no network at all.
#[tokio::test(flavor = "multi_thread")]
async fn a_recorded_run_replays_through_the_same_client() {
    let json = include_str!("../fixtures/chunked-run.json");
    let transport = replay_fixture(json).expect("the fixture");
    let mut session: Session<_, Board> = Session::new(transport, "replay");

    let printed = transcript(
        &mut session,
        Watch {
            fragments: true,
            ..Watch::default()
        },
        "first\nsecond\n",
    )
    .await;

    assert!(!printed.contains("  error"), "{printed}");
    assert!(
        printed.contains(r#"[te":"line\][nbreak","ti]"#),
        "{printed}"
    );
    assert_eq!(
        tool_calls(&session)[0].1,
        r#"{"note":"line\nbreak","title":"ship the SDK","depth":3}"#
    );
    assert_eq!(session.messages().len(), 5, "{:?}", session.messages());
}
