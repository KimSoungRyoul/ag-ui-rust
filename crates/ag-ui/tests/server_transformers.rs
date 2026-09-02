//! Transformers, in a real run rather than in isolation.

#![cfg(feature = "server")]

use ag_ui::server::{
    Agent, FilterToolCalls, Result, RunContext, Runner, StreamTransformer, SubagentVisibility,
    ToolResultToState,
};
use ag_ui::{Event, EventType, RunAgentInput, RunOutcome};
use futures_util::StreamExt as _;
use serde_json::json;

/// Calls two tools: one the client is allowed to see, one it is not.
struct TwoTools;

impl Agent for TwoTools {
    type State = serde_json::Value;

    async fn run(&self, ctx: &mut RunContext<serde_json::Value>) -> Result<RunOutcome> {
        let mut public = ctx.tool_call("search")?;
        public.args(r#"{"q":"rust"}"#)?;
        public.result(r#"{"hits":1}"#)?;

        let mut private = ctx.tool_call("internal_debug")?;
        private.args(r#"{"dump":true}"#)?;
        private.result("everything")?;

        ctx.say("done")?;
        Ok(RunOutcome::Success)
    }
}

async fn collect(runner: Runner<impl Agent>) -> Vec<Event> {
    runner
        .run(RunAgentInput::new("t", "r"))
        .map(|event| event.expect("the run stream should not break"))
        .collect()
        .await
}

#[tokio::test]
async fn a_denied_tool_call_leaves_no_trace() {
    let events =
        collect(Runner::new(TwoTools).transformer(FilterToolCalls::deny(["internal_debug"]))).await;

    let types: Vec<_> = events.iter().map(Event::event_type).collect();
    assert_eq!(
        types,
        [
            EventType::RunStarted,
            EventType::ToolCallStart,
            EventType::ToolCallArgs,
            EventType::ToolCallEnd,
            EventType::ToolCallResult,
            EventType::TextMessageStart,
            EventType::TextMessageContent,
            EventType::TextMessageEnd,
            EventType::RunFinished,
        ]
    );
    assert!(
        !format!("{events:?}").contains("internal_debug"),
        "the filtered tool should not be mentioned at all"
    );
}

#[tokio::test]
async fn a_tool_result_can_become_the_state() {
    let events = collect(
        Runner::new(TwoTools)
            .transformer(FilterToolCalls::allow(["search"]))
            .transformer(ToolResultToState::snapshot("search").replacing()),
    )
    .await;

    let types: Vec<_> = events.iter().map(Event::event_type).collect();
    assert_eq!(
        types,
        [
            EventType::RunStarted,
            EventType::ToolCallStart,
            EventType::ToolCallArgs,
            EventType::ToolCallEnd,
            EventType::StateSnapshot,
            EventType::TextMessageStart,
            EventType::TextMessageContent,
            EventType::TextMessageEnd,
            EventType::RunFinished,
        ]
    );
    assert_eq!(events[4], Event::state_snapshot(json!({"hits": 1})));
}

#[tokio::test]
async fn a_custom_transformer_can_stamp_every_event() {
    struct Stamp(i64);

    impl StreamTransformer for Stamp {
        fn transform(&mut self, event: Event) -> Vec<Event> {
            vec![event.with_timestamp(self.0)]
        }
    }

    let events = collect(Runner::new(TwoTools).transformer(Stamp(1_700_000_000_000))).await;
    assert!(
        events
            .iter()
            .all(|event| event.base().timestamp == Some(1_700_000_000_000)),
        "every event, including the ones the driver emits, goes through the chain"
    );
}

// ---- subagent visibility --------------------------------------------------

/// Says something, delegates to a subagent that says something and calls a
/// tool, restates history with one attributed message, and says something
/// more.
struct Delegating;

impl Agent for Delegating {
    type State = serde_json::Value;

    async fn run(&self, ctx: &mut RunContext<serde_json::Value>) -> Result<RunOutcome> {
        ctx.say("parent first")?;
        {
            let mut researcher = ctx.subagent("researcher")?;
            researcher.say("child")?;
            let mut call = researcher.tool_call("search")?;
            call.args("{}")?;
            call.result("hit")?;
        }
        ctx.emit(Event::messages_snapshot(vec![
            ag_ui::Message::assistant("h1", "history"),
            ag_ui::Message::Assistant(ag_ui::AssistantMessage {
                id: "h2".into(),
                content: Some("theirs".into()),
                subagent_run_id: Some("s-old".into()),
                ..Default::default()
            }),
        ]))?;
        ctx.say("parent last")?;
        Ok(RunOutcome::Success)
    }
}

#[tokio::test]
async fn attributed_is_the_default_and_the_full_surface() {
    let events = collect(Runner::new(Delegating)).await;
    let types: Vec<_> = events.iter().map(Event::event_type).collect();
    assert_eq!(
        types,
        [
            EventType::RunStarted,
            EventType::TextMessageStart,
            EventType::TextMessageContent,
            EventType::TextMessageEnd,
            EventType::SubagentStarted,
            EventType::TextMessageStart,
            EventType::TextMessageContent,
            EventType::TextMessageEnd,
            EventType::ToolCallStart,
            EventType::ToolCallArgs,
            EventType::ToolCallEnd,
            EventType::ToolCallResult,
            EventType::SubagentFinished,
            EventType::MessagesSnapshot,
            EventType::TextMessageStart,
            EventType::TextMessageContent,
            EventType::TextMessageEnd,
            EventType::RunFinished,
        ]
    );
    assert!(events[5..12].iter().all(|e| e.subagent_run_id().is_some()));
}

#[tokio::test]
async fn inline_visibility_flattens_the_stream_to_the_pre_subagent_shape() {
    let events = collect(Runner::new(Delegating).transformer(SubagentVisibility::inline())).await;
    let types: Vec<_> = events.iter().map(Event::event_type).collect();
    assert_eq!(
        types,
        [
            EventType::RunStarted,
            EventType::TextMessageStart,
            EventType::TextMessageContent,
            EventType::TextMessageEnd,
            EventType::TextMessageStart,
            EventType::TextMessageContent,
            EventType::TextMessageEnd,
            EventType::ToolCallStart,
            EventType::ToolCallArgs,
            EventType::ToolCallEnd,
            EventType::ToolCallResult,
            EventType::MessagesSnapshot,
            EventType::TextMessageStart,
            EventType::TextMessageContent,
            EventType::TextMessageEnd,
            EventType::RunFinished,
        ]
    );
    assert!(
        events.iter().all(|e| e.subagent_run_id().is_none()),
        "nothing on the wire says subagent"
    );
    let Event::MessagesSnapshot(snapshot) = &events[11] else {
        panic!("wrong variant");
    };
    assert_eq!(snapshot.messages.len(), 2);
    assert!(
        snapshot
            .messages
            .iter()
            .all(|m| m.subagent_run_id().is_none()),
        "the history is stripped too"
    );
    assert!(
        !serde_json::to_string(&events).unwrap().contains("subagent"),
        "not even as a substring"
    );
}

#[tokio::test]
async fn hidden_visibility_keeps_only_the_parents_events() {
    let events = collect(Runner::new(Delegating).transformer(SubagentVisibility::hidden())).await;
    let types: Vec<_> = events.iter().map(Event::event_type).collect();
    assert_eq!(
        types,
        [
            EventType::RunStarted,
            EventType::TextMessageStart,
            EventType::TextMessageContent,
            EventType::TextMessageEnd,
            EventType::MessagesSnapshot,
            EventType::TextMessageStart,
            EventType::TextMessageContent,
            EventType::TextMessageEnd,
            EventType::RunFinished,
        ]
    );
    let Event::MessagesSnapshot(snapshot) = &events[4] else {
        panic!("wrong variant");
    };
    assert_eq!(
        snapshot.messages.len(),
        1,
        "the attributed history message went too"
    );
}

#[test]
fn hidden_drops_an_untagged_continuation_of_a_subagents_message() {
    let mut filter = SubagentVisibility::hidden();
    assert!(
        filter
            .transform(
                Event::text_message_start("m1", Default::default()).with_subagent_run_id("s1")
            )
            .is_empty()
    );
    assert!(
        filter
            .transform(Event::text_message_content("m1", "hi"))
            .is_empty(),
        "legal on the wire, but it belongs to the subagent"
    );
    assert!(filter.transform(Event::text_message_end("m1")).is_empty());
    // A later parent message under the same id is the parent's again.
    assert_eq!(
        filter
            .transform(Event::text_message_start("m1", Default::default()))
            .len(),
        1
    );
    assert_eq!(filter.mode(), SubagentVisibility::Hidden);
}

/// A subagent that moves the shared state, then says so.
struct Publishing;

impl Agent for Publishing {
    type State = serde_json::Value;

    async fn run(&self, ctx: &mut RunContext<serde_json::Value>) -> Result<RunOutcome> {
        {
            let mut worker = ctx.subagent("worker")?;
            worker.set_state(&json!({"done": 1}))?;
            worker.say("moved it")?;
        }
        ctx.say("noted")?;
        Ok(RunOutcome::Success)
    }
}

/// The state is the thread's, whoever published it: a client that never saw
/// the subagent's `STATE_SNAPSHOT` would mirror a stale board and send it
/// back on its next request.
#[tokio::test]
async fn hidden_visibility_keeps_the_state_a_subagent_published() {
    let events = collect(Runner::new(Publishing).transformer(SubagentVisibility::hidden())).await;
    let types: Vec<_> = events.iter().map(Event::event_type).collect();
    assert_eq!(
        types,
        [
            EventType::RunStarted,
            EventType::StateSnapshot,
            EventType::TextMessageStart,
            EventType::TextMessageContent,
            EventType::TextMessageEnd,
            EventType::RunFinished,
        ]
    );
    assert_eq!(
        events[1].subagent_run_id(),
        None,
        "the state goes out as the parent's"
    );
    let Event::StateSnapshot(snapshot) = &events[1] else {
        unreachable!("asserted above");
    };
    assert_eq!(snapshot.snapshot, json!({"done": 1}));

    // Attributed and inline agree on the payload; only the tag differs.
    let attributed = collect(Runner::new(Publishing)).await;
    assert_eq!(
        attributed[2].subagent_run_id().map(|id| id.as_str()),
        Some("r-sub-1")
    );
    let inline = collect(Runner::new(Publishing).transformer(SubagentVisibility::inline())).await;
    assert_eq!(inline[1].event_type(), EventType::StateSnapshot);
    assert_eq!(inline[1].subagent_run_id(), None);
}

/// A subagent that pauses the run on a question of its own.
struct Pausing;

impl Agent for Pausing {
    type State = serde_json::Value;

    async fn run(&self, ctx: &mut RunContext<serde_json::Value>) -> Result<RunOutcome> {
        let mut worker = ctx.subagent("worker")?;
        worker.say("May I?")?;
        let interrupt =
            ag_ui::Interrupt::new("ok", "tool_approval").with_subagent_run_id(worker.id().clone());
        worker.suspend(vec![interrupt.id.clone()])?;
        Ok(RunOutcome::interrupt(vec![interrupt]))
    }
}

/// The question still stands for a consumer that never saw the subagent, so
/// the interrupt stays and only its tag goes — with it, nothing on the wire
/// says subagent.
#[tokio::test]
async fn inline_and_hidden_strip_the_attribution_from_interrupts_too() {
    for filter in [SubagentVisibility::inline(), SubagentVisibility::hidden()] {
        let mode = filter.mode();
        let events = collect(Runner::new(Pausing).transformer(filter)).await;

        let Some(Event::RunFinished(finished)) = events.last() else {
            panic!("{mode:?}: the run must end with RUN_FINISHED: {events:?}");
        };
        let interrupts = finished.outcome.as_ref().expect("an outcome").interrupts();
        assert_eq!(interrupts.len(), 1, "{mode:?}: {events:?}");
        assert_eq!(interrupts[0].id, "ok");
        assert_eq!(interrupts[0].subagent_run_id, None, "{mode:?}");

        for event in &events {
            let json = serde_json::to_string(event).expect("serializes");
            assert!(!json.contains("subagentRunId"), "{mode:?}: {json}");
        }
    }
}
