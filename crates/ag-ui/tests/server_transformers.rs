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
    // The first writer owns the id for the run, as the verifier reads it: an
    // untagged re-open of m1 is still the subagent's, and a fresh id is the
    // parent's.
    assert!(
        filter
            .transform(Event::text_message_start("m1", Default::default()))
            .is_empty()
    );
    assert_eq!(
        filter
            .transform(Event::text_message_start("m2", Default::default()))
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
        // The parent moves it again, so a consumer that missed the first
        // publish has a second one to fail on.
        ctx.set_state(&json!({"done": 2}))?;
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
            // The parent's own publish: a snapshot, being smaller than the
            // patch on a state this size.
            EventType::StateSnapshot,
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

/// Hand-built streams the sink would have tagged: the filter judges an
/// untagged event by who owns the id it continues, as the verifier does.
#[test]
fn hidden_drops_untagged_continuations_of_a_subagents_entities() {
    use ag_ui::server::StreamTransformer as _;
    use ag_ui::{AssistantMessage, Message, ToolCall, ToolMessage};

    let tagged = |event: Event, id: &str| event.with_subagent_run_id(id);
    let mut filter = SubagentVisibility::hidden();
    let mut kept = |event: Event| !filter.transform(event).is_empty();

    // A chunk stream: the first chunk names the id and the owner, the rest
    // name neither.
    assert!(!kept(tagged(
        Event::text_message_chunk(Some("m1".into()), Some("Hel".into())),
        "s1"
    )));
    assert!(!kept(Event::text_message_chunk(None, Some("lo".into()))));
    assert!(!kept(Event::text_message_chunk(
        Some("m1".into()),
        Some("!".into())
    )));
    // The parent's own chunk stream is untouched.
    assert!(kept(Event::text_message_chunk(
        Some("m2".into()),
        Some("mine".into())
    )));
    assert!(kept(Event::text_message_chunk(None, Some(" too".into()))));

    // A call carried by a subagent's message is the subagent's, tag or no tag
    // — and so is its end and its result.
    assert!(!kept(tagged(
        Event::text_message_start("m3", ag_ui::TextMessageRole::Assistant),
        "s1"
    )));
    let mut start = ag_ui::ToolCallStartEvent::new("c1", "search");
    start.parent_message_id = Some("m3".into());
    assert!(!kept(Event::ToolCallStart(start)));
    assert!(!kept(Event::tool_call_args("c1", "{}")));
    assert!(!kept(tagged(Event::tool_call_end("c1"), "s1")));
    assert!(!kept(Event::tool_call_result("m4", "c1", "hit")));
    // An untagged re-open after the close is still the subagent's.
    assert!(!kept(tagged(Event::text_message_end("m3"), "s1")));
    assert!(!kept(Event::text_message_start(
        "m3",
        ag_ui::TextMessageRole::Assistant
    )));
    assert!(!kept(Event::text_message_end("m3")));

    // A snapshot restates the conversation: the subagent's messages go, and
    // so does the tool message answering a call inside one of them.
    let mut filter = SubagentVisibility::hidden();
    let out = filter.transform(Event::messages_snapshot(vec![
        Message::assistant("h1", "mine"),
        Message::Assistant(AssistantMessage {
            id: "h2".into(),
            tool_calls: Some(vec![ToolCall::new("hc1", "search", "{}")]),
            subagent_run_id: Some("s1".into()),
            ..Default::default()
        }),
        Message::Tool(ToolMessage {
            id: "h3".into(),
            content: "hit".into(),
            tool_call_id: "hc1".into(),
            ..Default::default()
        }),
    ]));
    let Some(Event::MessagesSnapshot(snapshot)) = out.first() else {
        panic!("the snapshot is kept: {out:?}");
    };
    let ids: Vec<&str> = snapshot.messages.iter().map(|m| m.id().as_str()).collect();
    assert_eq!(ids, ["h1"]);
    // And the ids it named stay hidden afterwards.
    assert!(
        filter
            .transform(Event::tool_call_result("h4", "hc1", "again"))
            .is_empty()
    );
}

/// The round trip a consumer makes: the subagent's publish and the parent's
/// later one both apply, in hidden mode as in the others — which is the whole
/// reason the state is kept.
#[cfg(feature = "client")]
#[tokio::test]
async fn hidden_visibility_state_applies_on_the_client_including_a_later_parent_publish() {
    use ag_ui::client::Applier;

    for filter in [
        SubagentVisibility::Attributed.filter(),
        SubagentVisibility::inline(),
        SubagentVisibility::hidden(),
    ] {
        let mode = filter.mode();
        let events = collect(Runner::new(Publishing).transformer(filter)).await;
        let mut applier = Applier::new();
        for event in &events {
            applier
                .apply(event)
                .unwrap_or_else(|error| panic!("{mode:?}: {event:?} should apply: {error}"));
        }
        assert_eq!(applier.state(), &json!({"done": 2}), "{mode:?}");
    }
}

/// Agents as tools, with the result reported from inside the subagent: the
/// parent's call, answered by the child.
struct AsTool;

impl Agent for AsTool {
    type State = serde_json::Value;

    async fn run(&self, ctx: &mut RunContext<serde_json::Value>) -> Result<RunOutcome> {
        let mut call = ctx.tool_call("task")?;
        call.args("{}")?;
        let (call_id, result_id) = (call.id().clone(), call.result_message_id().clone());
        call.end()?;
        {
            let announce = ag_ui::SubagentStartedEvent::new("s-1", "researcher")
                .with_parent_tool_call(call_id.clone());
            let mut researcher = ctx.subagent_with(announce)?;
            researcher.say("child")?;
            researcher.emit(Event::tool_call_result(result_id, call_id, "3 sources"))?;
        }
        ctx.say("done")?;
        Ok(RunOutcome::Success)
    }
}

/// The consumer saw the parent's call, so it is owed the answer whoever
/// wrote it — a call left unanswered in the history is what the next request
/// would carry back.
#[tokio::test]
async fn hidden_keeps_the_answer_to_the_parents_call_whoever_executed_it() {
    let attributed = collect(Runner::new(AsTool)).await;
    let result = attributed
        .iter()
        .find(|event| event.event_type() == EventType::ToolCallResult)
        .expect("a result");
    assert_eq!(
        result.subagent_run_id().map(|id| id.as_str()),
        Some("s-1"),
        "the sink tagged the result with the executor"
    );

    let hidden = collect(Runner::new(AsTool).transformer(SubagentVisibility::hidden())).await;
    let types: Vec<_> = hidden.iter().map(Event::event_type).collect();
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
    assert_eq!(hidden[4].subagent_run_id(), None, "as the parent's");

    // The same in a replay: a tool message answering a visible call stays,
    // untagged; one answering a hidden call goes with the call.
    use ag_ui::server::StreamTransformer as _;
    use ag_ui::{AssistantMessage, Message, ToolCall, ToolMessage};
    let mut filter = SubagentVisibility::hidden();
    let out = filter.transform(Event::messages_snapshot(vec![
        Message::Assistant(AssistantMessage {
            id: "h1".into(),
            tool_calls: Some(vec![ToolCall::new("c-parent", "task", "{}")]),
            ..Default::default()
        }),
        Message::Tool(ToolMessage {
            id: "h2".into(),
            content: "3 sources".into(),
            tool_call_id: "c-parent".into(),
            subagent_run_id: Some("s-1".into()),
            ..Default::default()
        }),
        Message::Assistant(AssistantMessage {
            id: "h3".into(),
            tool_calls: Some(vec![ToolCall::new("c-child", "search", "{}")]),
            subagent_run_id: Some("s-1".into()),
            ..Default::default()
        }),
        Message::Tool(ToolMessage {
            id: "h4".into(),
            content: "hit".into(),
            tool_call_id: "c-child".into(),
            ..Default::default()
        }),
    ]));
    let Some(Event::MessagesSnapshot(snapshot)) = out.first() else {
        panic!("the snapshot is kept: {out:?}");
    };
    let ids: Vec<&str> = snapshot.messages.iter().map(|m| m.id().as_str()).collect();
    assert_eq!(ids, ["h1", "h2"]);
    assert_eq!(snapshot.messages[1].subagent_run_id(), None);
}

/// A subagent running a step of the same name as the parent's open step —
/// legal attributed, since steps are keyed by owner.
struct SameStep;

impl Agent for SameStep {
    type State = serde_json::Value;

    async fn run(&self, ctx: &mut RunContext<serde_json::Value>) -> Result<RunOutcome> {
        let mut outer = ctx.step("board")?;
        {
            let mut worker = outer.subagent("worker")?;
            let mut inner = worker.step("board")?;
            inner.say("nested")?;
        }
        outer.say("done")?;
        Ok(RunOutcome::Success)
    }
}

/// The flattened shape cannot express it — with the tags gone the two
/// steps would collide — so Inline drops a subagent's steps, as it drops the
/// lifecycle events, and the run finishes. Hidden drops them with the rest.
#[tokio::test]
async fn inline_drops_a_subagents_steps_rather_than_colliding_them() {
    let attributed = collect(Runner::new(SameStep)).await;
    assert_eq!(
        attributed
            .iter()
            .filter(|e| e.event_type() == EventType::StepStarted)
            .count(),
        2
    );

    for filter in [SubagentVisibility::inline(), SubagentVisibility::hidden()] {
        let mode = filter.mode();
        let events = collect(Runner::new(SameStep).transformer(filter)).await;
        let steps: Vec<_> = events
            .iter()
            .filter(|e| matches!(e, Event::StepStarted(_) | Event::StepFinished(_)))
            .map(|e| (e.event_type(), e.subagent_run_id().is_some()))
            .collect();
        assert_eq!(
            steps,
            [
                (EventType::StepStarted, false),
                (EventType::StepFinished, false)
            ],
            "{mode:?}: {events:?}"
        );
        assert_eq!(
            events.last().map(Event::event_type),
            Some(EventType::RunFinished),
            "{mode:?}: {events:?}"
        );
    }
    // Inline still carries the nested say, as the parent's.
    let inline = collect(Runner::new(SameStep).transformer(SubagentVisibility::inline())).await;
    assert_eq!(
        inline
            .iter()
            .filter(|e| e.event_type() == EventType::TextMessageEnd)
            .count(),
        2
    );
}

/// The filter's reading of an untagged, id-less chunk is the consuming
/// normalizer's: the parent's open stream when there is one, otherwise the
/// only open stream — which is a subagent's if any is.
#[test]
fn hidden_routes_an_untagged_idless_chunk_like_the_normalizer() {
    use ag_ui::server::StreamTransformer as _;
    let tagged = |event: Event, id: &str| event.with_subagent_run_id(id);
    let chunk = |id: Option<&str>, delta: &str| {
        Event::text_message_chunk(id.map(ag_ui::MessageId::new), Some(delta.to_owned()))
    };
    let mut filter = SubagentVisibility::hidden();
    let mut kept = |event: Event| !filter.transform(event).is_empty();

    // The parent opened m1 by chunk; a subagent's chunk in between does not
    // take the parent's continuation away from it.
    assert!(kept(chunk(Some("m1"), "a")));
    assert!(!kept(tagged(chunk(Some("m2"), "x"), "s1")));
    assert!(kept(chunk(None, "b")));

    // An explicit start opens the parent's stream too.
    assert!(kept(Event::text_message_start("m3", Default::default())));
    assert!(kept(chunk(None, "c")));
    assert!(kept(Event::text_message_end("m3")));

    // With the parent's stream closed, the only open one is the subagent's.
    assert!(!kept(tagged(chunk(Some("m4"), "y"), "s1")));
    assert!(!kept(chunk(None, "d")));

    // The subagent's stream closed, nothing is open: the chunk passes and
    // the consumer, not the filter, is the one to complain.
    assert!(!kept(tagged(Event::text_message_end("m4"), "s1")));
    assert!(kept(chunk(None, "e")));
}

/// An activity is owned by the snapshot that minted it, and only a replacing
/// snapshot re-mints it — the verifiers' rule, applied to what the consumer
/// gets to see.
#[test]
fn hidden_tracks_activities_by_their_owner() {
    use ag_ui::server::StreamTransformer as _;
    use ag_ui::{JsonObject, PatchOperation};
    let tagged = |event: Event, id: &str| event.with_subagent_run_id(id);
    let snapshot = |id: &str, replace: bool| {
        let mut event = ag_ui::ActivitySnapshotEvent::new(id, "progress", JsonObject::new());
        event.replace = replace;
        Event::ActivitySnapshot(event)
    };
    let delta =
        |id: &str| Event::activity_delta(id, "progress", vec![PatchOperation::add("/s", 1)]);
    let mut filter = SubagentVisibility::hidden();

    // A subagent's activity, and the parent's untagged patch of it, which
    // the consumer could not apply to something it never saw.
    assert!(
        filter
            .transform(tagged(snapshot("a1", true), "s1"))
            .is_empty()
    );
    assert!(filter.transform(delta("a1")).is_empty());
    assert!(
        filter.transform(snapshot("a1", false)).is_empty(),
        "a merge keeps the owner"
    );

    // A replacing snapshot from the parent re-mints it for the consumer.
    assert_eq!(filter.transform(snapshot("a1", true)).len(), 1);
    assert_eq!(filter.transform(delta("a1")).len(), 1);

    // A subagent merging into the parent's activity: the entity is visible,
    // so the change reaches the consumer, untagged.
    let out = filter.transform(tagged(snapshot("a1", false), "s1"));
    assert_eq!(out.len(), 1);
    assert_eq!(out[0].subagent_run_id(), None);
}

/// An opaque blob for an entity the consumer never saw goes with the entity.
#[test]
fn hidden_drops_an_encrypted_value_for_a_hidden_entity() {
    use ag_ui::ReasoningEncryptedValueSubtype;
    use ag_ui::server::StreamTransformer as _;
    let tagged = |event: Event, id: &str| event.with_subagent_run_id(id);
    let mut filter = SubagentVisibility::hidden();
    let mut kept = |event: Event| !filter.transform(event).is_empty();

    assert!(!kept(tagged(Event::tool_call_start("c1", "search"), "s1")));
    assert!(!kept(tagged(Event::reasoning_message_start("r1"), "s1")));
    assert!(kept(Event::tool_call_start("c2", "search")));

    let blob = |subtype, id: &str| Event::reasoning_encrypted_value(subtype, id, "opaque");
    assert!(!kept(blob(ReasoningEncryptedValueSubtype::ToolCall, "c1")));
    assert!(!kept(blob(ReasoningEncryptedValueSubtype::Message, "r1")));
    assert!(kept(blob(ReasoningEncryptedValueSubtype::ToolCall, "c2")));
    assert!(!kept(tagged(
        blob(ReasoningEncryptedValueSubtype::ToolCall, "c2"),
        "s1"
    )));
}

/// A snapshot restates what it carries and leaves the rest as the run
/// established it — the verifiers' reading of an authoritative seed.
#[test]
fn hidden_keeps_what_a_snapshot_did_not_restate() {
    use ag_ui::Message;
    use ag_ui::server::StreamTransformer as _;
    let tagged = |event: Event, id: &str| event.with_subagent_run_id(id);
    let mut filter = SubagentVisibility::hidden();
    let mut kept = |event: Event| !filter.transform(event).is_empty();

    assert!(!kept(tagged(
        Event::text_message_start("m1", Default::default()),
        "s1"
    )));
    assert!(!kept(tagged(Event::text_message_end("m1"), "s1")));

    // A snapshot that does not mention m1 does not hand it to the parent…
    assert!(kept(Event::messages_snapshot(vec![Message::user(
        "u1", "hi"
    )])));
    assert!(!kept(Event::text_message_start("m1", Default::default())));
    assert!(!kept(tagged(
        Event::text_message_content("m1", "more"),
        "s1"
    )));
    assert!(!kept(tagged(Event::text_message_end("m1"), "s1")));

    // …one that restates it as the parent's does.
    assert!(kept(Event::messages_snapshot(vec![Message::assistant(
        "m1", "mine now"
    )])));
    assert!(kept(Event::text_message_start("m1", Default::default())));
    assert!(kept(Event::text_message_end("m1")));
}
