//! What the client sends is what the agent receives, and back again.
//!
//! The other files here assert on *behaviour*; this one asserts on the payloads
//! themselves, in both directions. The request the client built has to arrive
//! field for field, a `MESSAGES_SNAPSHOT` has to carry every message variant the
//! protocol has without losing a field on the way, and state published in one
//! run has to be the state the next run starts from.

mod common;

use ag_ui_axum::AgentEndpoint;
use ag_ui_client::{Agent as ClientAgent, Session, Update};
use ag_ui_core::{
    ActivityMessage, AssistantMessage, Context, Event, InputContent, InputContentSource,
    MediaInputContent, Message, PatchOperation, RunAgentInput, RunOutcome, TextInputContent, Tool,
    ToolCall, ToolMessage, UserContent, UserMessage,
};
use ag_ui_server::{Agent, Result, RunContext};
use common::{serve, serve_endpoint, transport};
use futures_util::StreamExt as _;
use serde::{Deserialize, Serialize};
use serde_json::json;

// ---------------------------------------------------------------- request ----

/// Does nothing but let the endpoint echo the request back.
struct Quiet;

impl Agent for Quiet {
    type State = ();

    async fn run(&self, _ctx: &mut RunContext<()>) -> Result<RunOutcome> {
        Ok(RunOutcome::Success)
    }
}

/// A request with every field populated, including the awkward ones.
fn request() -> RunAgentInput {
    RunAgentInput {
        thread_id: "thread-1".into(),
        run_id: "run-1".into(),
        parent_run_id: Some("parent-run".into()),
        state: json!({"cart": {"items": 2}, "flags": ["a", "b"]}),
        messages: conversation(),
        tools: vec![Tool::new(
            "get_weather",
            "Looks up the weather.",
            json!({"type": "object", "properties": {"city": {"type": "string"}}}),
        )],
        context: vec![Context::new("current page", "/checkout")],
        forwarded_props: json!({"tenant": "acme", "locale": "ko-KR"}),
        resume: None,
    }
}

/// One message of every variant the protocol defines, with the optional fields
/// set — anything dropped in serialization shows up as an inequality.
fn conversation() -> Vec<Message> {
    vec![
        Message::system("m-1", "You are terse."),
        Message::developer("m-2", "Debug mode is on."),
        Message::User(UserMessage {
            id: "m-3".into(),
            content: UserContent::Parts(vec![
                InputContent::Text(TextInputContent {
                    text: "what is this?".to_owned(),
                }),
                InputContent::Image(MediaInputContent {
                    source: InputContentSource::Url {
                        value: "https://example.invalid/cat.png".to_owned(),
                        mime_type: Some("image/png".to_owned()),
                    },
                    metadata: Some(json!({"width": 640})),
                }),
            ]),
            name: Some("ada".to_owned()),
            encrypted_value: Some("opaque".to_owned()),
        }),
        Message::Assistant(AssistantMessage {
            id: "m-4".into(),
            content: Some("Looking it up.".to_owned()),
            name: Some("assistant".to_owned()),
            encrypted_value: Some("blob".to_owned()),
            tool_calls: Some(vec![ToolCall::new(
                "call-1",
                "get_weather",
                r#"{"city":"Seoul"}"#,
            )]),
        }),
        Message::Tool(ToolMessage {
            id: "m-5".into(),
            content: r#"{"tempC":21}"#.to_owned(),
            tool_call_id: "call-1".into(),
            error: Some("partial".to_owned()),
            encrypted_value: None,
        }),
        Message::Activity(ActivityMessage {
            id: "m-6".into(),
            activity_type: "web_search".to_owned(),
            content: json!({"query": "weather seoul", "hits": 3})
                .as_object()
                .expect("an object")
                .clone(),
        }),
    ]
}

#[tokio::test(flavor = "multi_thread")]
async fn the_request_the_client_sent_is_the_request_the_agent_received() {
    let url = serve_endpoint(AgentEndpoint::new(Quiet).echo_input(true)).await;
    let client = ClientAgent::new(transport(&url));

    let sent = request();
    let mut events = client.run(sent.clone());
    let first = events
        .next()
        .await
        .expect("a run starts")
        .expect("the stream should not break");

    let Event::RunStarted(started) = first else {
        panic!("the first event must be RUN_STARTED: {first:?}");
    };
    assert_eq!(started.thread_id, sent.thread_id);
    assert_eq!(started.run_id, sent.run_id);
    assert_eq!(started.parent_run_id, sent.parent_run_id);
    assert_eq!(
        started.input.as_deref(),
        Some(&sent),
        "the request must survive the round trip field for field"
    );
}

// --------------------------------------------------------------- messages ----

/// Replaces the whole conversation with one of every message variant.
struct Historian;

impl Agent for Historian {
    type State = ();

    async fn run(&self, ctx: &mut RunContext<()>) -> Result<RunOutcome> {
        ctx.emit(Event::messages_snapshot(conversation()))?;
        Ok(RunOutcome::Success)
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn a_messages_snapshot_replaces_the_conversation_without_losing_a_field() {
    let url = serve(Historian).await;
    let mut session = Session::<_>::new(transport(&url), "history");

    let mut replaced = None;
    {
        let mut run = session.send("start over");
        while let Some(update) = run.next().await {
            match update {
                Update::Messages(messages) => replaced = Some(messages),
                Update::Error(error) => panic!("a snapshot should apply cleanly: {error}"),
                _ => {}
            }
        }
    }

    assert_eq!(replaced.as_deref(), Some(conversation().as_slice()));
    // The user's own turn is gone: a snapshot is a replacement, not a merge.
    assert_eq!(session.messages(), conversation().as_slice());
}

// ------------------------------------------------------------- activities ----

/// Reports progress as an activity, then patches it.
struct Searcher;

impl Agent for Searcher {
    type State = ();

    async fn run(&self, ctx: &mut RunContext<()>) -> Result<RunOutcome> {
        let content = json!({"query": "rust", "hits": 0})
            .as_object()
            .expect("an object")
            .clone();
        ctx.emit(Event::activity_snapshot("act-1", "web_search", content))?;
        ctx.emit(Event::activity_delta(
            "act-1",
            "web_search",
            vec![
                PatchOperation::replace("/hits", 12),
                PatchOperation::add("/done", true),
            ],
        ))?;
        Ok(RunOutcome::Success)
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn an_activity_is_published_and_then_patched_in_place() {
    let url = serve(Searcher).await;
    let mut session = Session::<_>::new(transport(&url), "search");

    {
        let mut run = session.send("look it up");
        while let Some(update) = run.next().await {
            if let Update::Error(error) = update {
                panic!("an activity patch should apply cleanly: {error}");
            }
        }
    }

    assert_eq!(
        session.messages().last(),
        Some(&Message::Activity(ActivityMessage {
            id: "act-1".into(),
            activity_type: "web_search".to_owned(),
            content: json!({"query": "rust", "hits": 12, "done": true})
                .as_object()
                .expect("an object")
                .clone(),
        }))
    );
}

// ------------------------------------------------------------------ state ----

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
struct Counter {
    clicks: u32,
    seen: Vec<String>,
}

/// Starts from whatever state it was handed and adds to it.
struct Accumulator;

impl Agent for Accumulator {
    type State = Counter;

    async fn run(&self, ctx: &mut RunContext<Counter>) -> Result<RunOutcome> {
        let run = ctx.run_id().to_string();
        ctx.update_state(|counter| {
            counter.clicks += 1;
            counter.seen.push(run);
        })?;
        Ok(RunOutcome::Success)
    }
}

/// The client sends its state back on the next run, so an agent that keeps no
/// storage of its own still accumulates across a conversation.
#[tokio::test(flavor = "multi_thread")]
async fn state_published_by_one_run_is_the_state_the_next_run_starts_from() {
    let url = serve(Accumulator).await;
    let mut session = Session::<_, Counter>::new(transport(&url), "counter");

    for _ in 0..3 {
        let mut run = session.send("again");
        while let Some(update) = run.next().await {
            if let Update::Error(error) = update {
                panic!("state should carry cleanly: {error}");
            }
        }
    }

    assert_eq!(
        session.state(),
        Some(&Counter {
            clicks: 3,
            seen: vec![
                "counter-run-1".to_owned(),
                "counter-run-2".to_owned(),
                "counter-run-3".to_owned(),
            ],
        })
    );
}
