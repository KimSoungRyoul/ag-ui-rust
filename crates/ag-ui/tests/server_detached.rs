//! A run taken as its two halves, and a run carried on by a second driver:
//! everything `Runner::run` guarantees holds without the stream that joins
//! them.

#![cfg(feature = "server")]

use std::future::Future;
use std::num::NonZeroUsize;
use std::pin::Pin;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::task::{Context, Poll};
use std::time::Duration;

use ag_ui::server::{Agent, Error, EventReceiver, FilterToolCalls, Result, RunContext, Runner};
use ag_ui::{
    AssistantMessage, Event, EventType, Interrupt, Message, RunAgentInput, RunOutcome,
    TextMessageRole, ToolCall,
};
use futures_util::StreamExt as _;
use serde::{Deserialize, Serialize};
use serde_json::json;

fn input() -> RunAgentInput {
    RunAgentInput::new("t", "r")
}

fn types(events: &[Event]) -> Vec<EventType> {
    events.iter().map(Event::event_type).collect()
}

/// Polls `future` once, on no executor at all.
fn poll_once<F: Future>(future: Pin<&mut F>) -> Poll<F::Output> {
    let waker = futures_util::task::noop_waker();
    future.poll(&mut Context::from_waker(&waker))
}

/// Uses every typed emitter once.
struct Busy;

impl Agent for Busy {
    type State = ();

    async fn run(&self, ctx: &mut RunContext<()>) -> Result<RunOutcome> {
        let mut step = ctx.step("work")?;
        step.think("What is asked?")?;
        let said = step.say("Looking it up.")?;
        let start = ag_ui::ToolCallStartEvent::new(step.new_tool_call_id(), "search")
            .with_parent_message_id(said);
        step.tool_call_with(start)?.result("3 results")?;
        let mut helper = step.subagent("helper")?;
        helper.say("Summarised.")?;
        helper.finish()?;
        drop(step);
        ctx.set_result(json!({ "found": 3 }));
        Ok(RunOutcome::Success)
    }
}

struct Fails;

impl Agent for Fails {
    type State = ();

    async fn run(&self, ctx: &mut RunContext<()>) -> Result<RunOutcome> {
        let mut message = ctx.assistant_message()?;
        message.delta("about to fail")?;
        Err(Error::agent("the model went away"))
    }
}

struct Pauses;

impl Agent for Pauses {
    type State = ();

    async fn run(&self, _ctx: &mut RunContext<()>) -> Result<RunOutcome> {
        Ok(RunOutcome::interrupt(vec![Interrupt::new(
            "ok", "approval",
        )]))
    }
}

/// The run read through its halves, with the driver on another task.
async fn detached<A: Agent + 'static>(runner: Runner<A>) -> Vec<Event> {
    let (driver, events) = runner.into_parts(input());
    let run = tokio::spawn(driver);
    let events: Vec<Event> = events.collect().await;
    run.await.expect("the driver does not panic");
    events
}

#[tokio::test]
async fn the_halves_carry_the_run_the_stream_carries() {
    let whole: Vec<Event> = Runner::new(Busy).run(input()).collect().await;
    assert_eq!(detached(Runner::new(Busy)).await, whole);
    assert_eq!(whole.first().unwrap().event_type(), EventType::RunStarted);
    assert_eq!(whole.last().unwrap().event_type(), EventType::RunFinished);

    let failed: Vec<Event> = Runner::new(Fails).run(input()).collect().await;
    assert_eq!(detached(Runner::new(Fails)).await, failed);
    assert_eq!(
        types(&failed),
        [
            EventType::RunStarted,
            EventType::TextMessageStart,
            EventType::TextMessageContent,
            EventType::TextMessageEnd,
            EventType::RunError,
        ],
        "the handle still closed its message, and one terminal event followed"
    );

    let paused: Vec<Event> = Runner::new(Pauses).run(input()).collect().await;
    assert_eq!(detached(Runner::new(Pauses)).await, paused);

    let filter = || FilterToolCalls::deny(["search"]);
    let filtered: Vec<Event> = Runner::new(Busy)
        .transformer(filter())
        .run(input())
        .collect()
        .await;
    assert_eq!(
        detached(Runner::new(Busy).transformer(filter())).await,
        filtered
    );
    assert!(
        !types(&filtered).contains(&EventType::ToolCallStart),
        "the chain is the runner's, whichever way the run is taken"
    );
}

#[test]
fn the_agent_advances_with_nobody_reading_and_no_runtime() {
    let (driver, mut events) = Runner::new(Busy).into_parts(input());
    let mut driver = Box::pin(driver);
    assert_eq!(
        events.drain(),
        [],
        "nothing happens until the driver is polled"
    );

    // No executor, no reader: the driver is a plain future.
    assert_eq!(poll_once(driver.as_mut()), Poll::Ready(()));

    let events = events.drain();
    assert_eq!(events.first().unwrap().event_type(), EventType::RunStarted);
    let terminals = events
        .iter()
        .filter(|event| matches!(event, Event::RunFinished(_) | Event::RunError(_)))
        .count();
    assert_eq!(terminals, 1);
    let Some(Event::RunFinished(finished)) = events.last() else {
        panic!("{events:?}");
    };
    assert_eq!(finished.result, Some(json!({ "found": 3 })));
}

/// Opens a message, says one thing, and waits for something that never
/// comes: a process that stops in the middle of a run.
struct Stalls;

impl Agent for Stalls {
    type State = ();

    async fn run(&self, ctx: &mut RunContext<()>) -> Result<RunOutcome> {
        let mut message = ctx.assistant_message()?;
        message.delta("Hello")?;
        std::future::pending::<()>().await;
        message.end()?;
        Ok(RunOutcome::Success)
    }
}

#[tokio::test]
async fn cancelling_ends_a_detached_run_with_what_it_opened_closed() {
    /// Waits, mid-message, for something only cancellation interrupts.
    struct Listens;
    impl Agent for Listens {
        type State = ();
        async fn run(&self, ctx: &mut RunContext<()>) -> Result<RunOutcome> {
            let cancelled = ctx.cancelled();
            let mut message = ctx.assistant_message()?;
            message.delta("Hello")?;
            cancelled.await;
            message.delta("never sent")?;
            Ok(RunOutcome::Success)
        }
    }

    let runner = Runner::new(Listens);
    let token = runner.cancellation_token();
    let (driver, mut events) = runner.into_parts(input());
    let run = tokio::spawn(driver);
    let mut before = Vec::new();
    for _ in 0..3 {
        before.push(events.next().await.expect("the run is under way"));
    }
    assert_eq!(
        before.last().unwrap().event_type(),
        EventType::TextMessageContent
    );

    // Nobody is reading the response this run began in. The token is how it
    // is stopped.
    token.cancel();
    let rest: Vec<Event> = events.collect().await;
    run.await.expect("the driver ran to completion");
    assert_eq!(
        types(&rest),
        [EventType::TextMessageEnd, EventType::RunFinished]
    );
    let Some(Event::RunFinished(finished)) = rest.last() else {
        unreachable!()
    };
    assert_eq!(finished.outcome, Some(RunOutcome::Cancelled));
}

#[tokio::test]
async fn a_driver_dropped_before_it_finishes_leaves_no_terminal_event() {
    let (driver, mut events) = Runner::new(Stalls).into_parts(input());
    let mut driver = Box::pin(driver);
    assert!(poll_once(driver.as_mut()).is_pending());
    assert_eq!(events.drain().len(), 3);

    // The handle closes its message on the way out. Nothing is left to emit
    // the terminal event: that is the driver's, after the agent returns.
    drop(driver);
    let rest: Vec<Event> = events.collect().await;
    assert_eq!(types(&rest), [EventType::TextMessageEnd]);
}

#[tokio::test]
async fn an_overflow_ends_the_receiver_while_the_driver_is_still_pending() {
    struct IgnoresError(Arc<AtomicBool>);
    impl Drop for IgnoresError {
        fn drop(&mut self) {
            self.0.store(true, Ordering::SeqCst);
        }
    }
    impl Agent for IgnoresError {
        type State = ();
        async fn run(&self, ctx: &mut RunContext<()>) -> Result<RunOutcome> {
            assert!(ctx.say("fills the queue").is_err());
            std::future::pending().await
        }
    }

    let dropped = Arc::new(AtomicBool::new(false));
    let (driver, events) = Runner::new(IgnoresError(dropped.clone()))
        .event_buffer_capacity(NonZeroUsize::new(1).unwrap())
        .into_parts(input());
    let run = tokio::spawn(driver);

    let events: Vec<Event> = tokio::time::timeout(Duration::from_secs(1), events.collect())
        .await
        .expect("the overflow closes the receiver");
    assert_eq!(types(&events), [EventType::RunStarted, EventType::RunError]);
    let Some(Event::RunError(error)) = events.last() else {
        unreachable!()
    };
    assert_eq!(error.code.as_deref(), Some("EVENT_BUFFER_FULL"));

    // What the joined stream does on its own, and the caller of the halves
    // has to: the receiver ended, the driver has not.
    assert!(!run.is_finished());
    assert!(!dropped.load(Ordering::SeqCst));
    run.abort();
    assert!(run.await.unwrap_err().is_cancelled());
    assert!(dropped.load(Ordering::SeqCst));
}

#[tokio::test]
async fn a_dropped_receiver_unwinds_the_agent() {
    struct Talks;
    impl Agent for Talks {
        type State = ();
        async fn run(&self, ctx: &mut RunContext<()>) -> Result<RunOutcome> {
            loop {
                ctx.say("anyone?")?;
                tokio::task::yield_now().await;
            }
        }
    }
    let (driver, mut events) = Runner::new(Talks).into_parts(input());
    let run = tokio::spawn(driver);
    assert_eq!(
        events.next().await.map(|event| event.event_type()),
        Some(EventType::RunStarted)
    );
    drop(events);
    tokio::time::timeout(Duration::from_secs(1), run)
        .await
        .expect("the next emit fails, and `?` ends the agent")
        .expect("the driver does not panic");
}

// ---- a run carried on by a second driver --------------------------------

/// What a first driver sent before it stopped, and the receiver it left.
fn stopped_mid_run<A: Agent>(runner: Runner<A>, input: RunAgentInput) -> Vec<Event> {
    let (driver, mut events) = runner.into_parts(input);
    let mut driver = Box::pin(driver);
    assert!(poll_once(driver.as_mut()).is_pending(), "the agent stalls");
    // The process is gone: nothing unwinds, nothing is closed.
    std::mem::forget(driver);
    events.drain()
}

async fn continued<A: Agent + 'static>(
    runner: Runner<A>,
    input: RunAgentInput,
    sent: &[Event],
) -> Vec<Event> {
    let (driver, events) = runner
        .continue_run(input, sent)
        .expect("there is a run to continue");
    let run = tokio::spawn(driver);
    let events: Vec<Event> = events.collect().await;
    run.await.expect("the driver does not panic");
    events
}

/// Closes the message the first driver left open, by id, and goes on with
/// the typed emitters.
struct Finishes;

impl Agent for Finishes {
    type State = ();

    async fn run(&self, ctx: &mut RunContext<()>) -> Result<RunOutcome> {
        ctx.emit(Event::text_message_content("r-msg-1", " world"))?;
        ctx.emit(Event::text_message_end("r-msg-1"))?;
        ctx.say("And another thing.")?;
        Ok(RunOutcome::Success)
    }
}

#[tokio::test]
async fn a_continuation_sends_no_run_started_and_exactly_one_terminal_event() {
    let sent = stopped_mid_run(Runner::new(Stalls), input());
    assert_eq!(
        sent,
        [
            Event::run_started("t", "r"),
            Event::text_message_start("r-msg-1", TextMessageRole::Assistant),
            Event::text_message_content("r-msg-1", "Hello"),
        ]
    );

    let rest = continued(Runner::new(Finishes), input(), &sent).await;
    assert_eq!(
        rest,
        [
            Event::text_message_content("r-msg-1", " world"),
            Event::text_message_end("r-msg-1"),
            // The context counts on: `r-msg-1` is the message it continued.
            Event::text_message_start("r-msg-2", TextMessageRole::Assistant),
            Event::text_message_content("r-msg-2", "And another thing."),
            Event::text_message_end("r-msg-2"),
            Event::run_finished_success("t", "r"),
        ]
    );

    // The two parts are one run to whoever reads them.
    let run = [sent, rest].concat();
    #[cfg(feature = "client")]
    ag_ui::client::verify_all(&run).expect("one well-formed run");
    #[cfg(feature = "verify")]
    ag_ui::server::EventVerifier::new()
        .replay(&run)
        .expect("one well-formed run");
    let _ = run;
}

#[cfg(feature = "verify")]
#[tokio::test]
async fn what_the_first_driver_left_open_has_to_be_closed() {
    struct Forgets;
    impl Agent for Forgets {
        type State = ();
        async fn run(&self, _ctx: &mut RunContext<()>) -> Result<RunOutcome> {
            Ok(RunOutcome::Success)
        }
    }
    let sent = stopped_mid_run(Runner::new(Stalls), input());
    let rest = continued(Runner::new(Forgets), input(), &sent).await;
    let [Event::RunError(error)] = rest.as_slice() else {
        panic!("{rest:?}");
    };
    assert_eq!(error.code.as_deref(), Some("PROTOCOL_VIOLATION"));
    assert!(error.message.contains("r-msg-1"), "{}", error.message);
}

#[derive(Default, Serialize, Deserialize)]
struct Draft {
    revision: u32,
    notes: String,
}

#[tokio::test]
async fn a_continuation_publishes_state_against_what_the_client_holds() {
    struct Drafts;
    impl Agent for Drafts {
        type State = Draft;
        async fn run(&self, ctx: &mut RunContext<Draft>) -> Result<RunOutcome> {
            ctx.update_state(|draft| {
                draft.revision = 1;
                draft.notes = "a document long enough that a patch is the smaller way".into();
            })?;
            std::future::pending().await
        }
    }
    struct Revises;
    impl Agent for Revises {
        type State = Draft;
        async fn run(&self, ctx: &mut RunContext<Draft>) -> Result<RunOutcome> {
            // The input carries the state as it stands, and the agent reads
            // it as its own.
            assert_eq!(ctx.state().revision, 1);
            ctx.update_state(|draft| draft.revision = 2)?;
            Ok(RunOutcome::Success)
        }
    }

    let sent = stopped_mid_run(Runner::new(Drafts), input());
    let Some(Event::StateSnapshot(published)) = sent.last() else {
        panic!("{sent:?}");
    };
    let mut request = input();
    request.state = published.snapshot.clone();

    let rest = continued(Runner::new(Revises), request, &sent).await;
    assert_eq!(
        rest[0],
        Event::state_delta(vec![ag_ui::PatchOperation::replace("/revision", 2)]),
        "a delta against the snapshot the first driver sent, not a second snapshot"
    );
}

#[tokio::test]
async fn a_subagent_left_active_is_the_continuation_s_to_close() {
    struct Delegates;
    impl Agent for Delegates {
        type State = ();
        async fn run(&self, ctx: &mut RunContext<()>) -> Result<RunOutcome> {
            let mut helper = ctx.subagent("helper")?;
            helper.say("On it.")?;
            std::future::pending().await
        }
    }
    let sent = stopped_mid_run(Runner::new(Delegates), input());
    assert_eq!(sent[1].event_type(), EventType::SubagentStarted);

    // Left active, the run cannot finish: with or without the verifier.
    struct Forgets;
    impl Agent for Forgets {
        type State = ();
        async fn run(&self, _ctx: &mut RunContext<()>) -> Result<RunOutcome> {
            Ok(RunOutcome::Success)
        }
    }
    let rest = continued(Runner::new(Forgets), input(), &sent).await;
    let [Event::RunError(error)] = rest.as_slice() else {
        panic!("{rest:?}");
    };
    assert!(error.message.contains("r-sub-1"), "{}", error.message);

    // Closed by id, it can, and a new subagent gets the next id.
    struct Closes;
    impl Agent for Closes {
        type State = ();
        async fn run(&self, ctx: &mut RunContext<()>) -> Result<RunOutcome> {
            ctx.emit(Event::subagent_finished_success("r-sub-1"))?;
            ctx.subagent("second")?.finish()?;
            Ok(RunOutcome::Success)
        }
    }
    let rest = continued(Runner::new(Closes), input(), &sent).await;
    assert_eq!(
        rest,
        [
            Event::subagent_finished_success("r-sub-1"),
            Event::subagent_started("r-sub-2", "second"),
            Event::subagent_finished_success("r-sub-2"),
            Event::run_finished_success("t", "r"),
        ]
    );

    // A cancelled continuation closes it as a cancelled run does.
    struct Checks;
    impl Agent for Checks {
        type State = ();
        async fn run(&self, ctx: &mut RunContext<()>) -> Result<RunOutcome> {
            ctx.check_cancelled()?;
            Ok(RunOutcome::Success)
        }
    }
    let runner = Runner::new(Checks);
    runner.cancellation_token().cancel();
    let rest = continued(runner, input(), &sent).await;
    assert_eq!(
        types(&rest),
        [EventType::SubagentError, EventType::RunFinished]
    );
}

#[tokio::test]
async fn a_continuation_answers_a_call_the_history_made_and_filters_as_configured() {
    // The request's history: an earlier run asked for a search.
    let mut request = input();
    request.messages = vec![Message::Assistant(AssistantMessage {
        id: "m0".into(),
        tool_calls: Some(vec![ToolCall::new("c0", "search", "{}")]),
        ..Default::default()
    })];
    // The first driver answered it. Replaying that needs the history first.
    let sent = [
        Event::run_started("t", "r"),
        Event::tool_call_result("m1", "c0", "3 results"),
    ];

    struct Calls;
    impl Agent for Calls {
        type State = ();
        async fn run(&self, ctx: &mut RunContext<()>) -> Result<RunOutcome> {
            ctx.tool_call("internal_debug")?.result("hidden")?;
            ctx.tool_call("search")?.result("shown")?;
            Ok(RunOutcome::Success)
        }
    }
    let runner = Runner::new(Calls).transformer(FilterToolCalls::deny(["internal_debug"]));
    let rest = continued(runner, request, &sent).await;
    assert_eq!(
        types(&rest),
        [
            EventType::ToolCallStart,
            EventType::ToolCallEnd,
            EventType::ToolCallResult,
            EventType::RunFinished,
        ]
    );
    let Event::ToolCallStart(shown) = &rest[0] else {
        unreachable!()
    };
    assert_eq!(shown.tool_call_name, "search");
}

/// `continue_run`'s error, without the halves an `Ok` would carry.
fn refusal(input: RunAgentInput, sent: &[Event]) -> Error {
    let refused: Result<(_, EventReceiver)> = Runner::new(Finishes).continue_run(input, sent);
    match refused {
        Ok(_) => panic!("there was no run to continue in {sent:?}"),
        Err(error) => error,
    }
}

#[test]
fn there_has_to_be_a_run_to_continue() {
    let started = Event::run_started("t", "r");

    let nothing = refusal(input(), &[]);
    assert_eq!(
        nothing.to_string(),
        "RUN_STARTED breaks rule `out-of-order` (the event arrived before the event it depends \
         on): there is no run to continue: no RUN_STARTED was sent"
    );

    for terminal in [
        Event::run_finished_success("t", "r"),
        Event::run_error("it failed"),
    ] {
        let ended = refusal(input(), &[started.clone(), terminal.clone()]);
        let Error::Verification(error) = &ended else {
            panic!("{ended}");
        };
        assert_eq!(error.event, terminal.event_type());
        assert_eq!(error.rule, ag_ui::server::Rule::RunEnded);
        assert!(error.detail.ends_with("the run already ended"), "{ended}");
    }

    let other = refusal(
        RunAgentInput::new("t", "another"),
        std::slice::from_ref(&started),
    );
    assert_eq!(
        other.to_string(),
        "protocol violation: the run that was started is \"r\" on thread \"t\", and the input \
         to continue it names run \"another\" on thread \"t\""
    );

    // With the verifier, a stored stream that is not a run's is refused at
    // the event that shows it.
    #[cfg(feature = "verify")]
    {
        let orphan = Event::text_message_content("m1", "no start");
        let Error::Verification(error) = refusal(input(), &[started, orphan]) else {
            panic!("expected the content event's own rejection");
        };
        assert_eq!(error.event, EventType::TextMessageContent);
        assert_eq!(error.rule, ag_ui::server::Rule::NotOpen);
    }
}
