//! Transformers, in a real run rather than in isolation.

use ag_ui_core::{Event, EventType, RunAgentInput, RunOutcome};
use ag_ui_server::{
    Agent, FilterToolCalls, Result, RunContext, Runner, StreamTransformer, ToolResultToState,
};
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
