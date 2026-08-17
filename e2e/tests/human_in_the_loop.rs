//! The interrupt round trip, closed over real HTTP.
//!
//! `RunFinished.outcome` exists for exactly this: the agent stops and says what
//! it needs, the client answers, and a *second* request carries the answer back
//! into `ctx.resume()`. Both halves of the answer are covered — resolved and
//! cancelled — and the agent fails the run outright if the payload it was
//! promised is not there, so a lost answer cannot pass as a success.

mod common;

use ag_ui_client::{InterruptExt as _, RunEnd, Session, Update};
use ag_ui_core::{Interrupt, JsonObject, Message, ResumeStatus, RunOutcome};
use ag_ui_server::{Agent, Error, Result, RunContext};
use common::{serve, transport};
use futures_util::StreamExt as _;
use serde_json::{Value, json};

const INTERRUPT_ID: &str = "approve-deploy";

/// Refuses to deploy until a human says so, and reports what they said.
struct Deployer;

impl Agent for Deployer {
    type State = ();

    async fn run(&self, ctx: &mut RunContext<()>) -> Result<RunOutcome> {
        let Some(answer) = ctx.resume_for(INTERRUPT_ID) else {
            ctx.say("Deploying to production needs a human.")?;
            return Ok(RunOutcome::interrupt(vec![request()]));
        };

        match answer.status {
            ResumeStatus::Resolved => {
                // Reading the payload is the proof that it survived the trip:
                // an answer that arrived empty fails the run instead of
                // quietly succeeding.
                let build = answer
                    .payload
                    .as_ref()
                    .and_then(|payload| payload.get("build"))
                    .and_then(Value::as_u64)
                    .ok_or_else(|| Error::agent("the approval carried no build number"))?;
                ctx.say(format!("Deployed build {build}."))?;
            }
            ResumeStatus::Cancelled => {
                ctx.say("Left production alone.")?;
            }
        }

        Ok(RunOutcome::Success)
    }
}

/// The interrupt the agent pauses on, with every optional field set so the
/// whole payload has to survive serialization.
fn request() -> Interrupt {
    let mut schema = JsonObject::new();
    schema.insert("type".to_owned(), json!("object"));
    schema.insert("required".to_owned(), json!(["build"]));

    Interrupt {
        id: INTERRUPT_ID.to_owned(),
        reason: "tool_approval".to_owned(),
        message: Some("Deploy build 42 to production?".to_owned()),
        tool_call_id: Some("call-deploy".into()),
        response_schema: Some(schema),
        ..Default::default()
    }
}

/// Drains a run, returning every update it produced.
macro_rules! drain {
    ($run:expr) => {{
        let mut updates = Vec::new();
        let mut run = $run;
        while let Some(update) = run.next().await {
            updates.push(update);
        }
        updates
    }};
}

/// The last thing the assistant said.
fn last_reply<T>(session: &Session<T>) -> &str {
    session
        .messages()
        .iter()
        .rev()
        .find_map(|message| match message {
            Message::Assistant(assistant) => assistant.content.as_deref(),
            _ => None,
        })
        .expect("the agent should have said something")
}

/// Runs the first turn and returns the session, paused, with its interrupt.
async fn pause() -> (Session<ag_ui_client::transport::HttpTransport>, Interrupt) {
    let url = serve(Deployer).await;
    let mut session = Session::<_>::new(transport(&url), "deploy");

    let updates = drain!(session.send("ship build 42"));
    let interrupts: Vec<Interrupt> = updates
        .into_iter()
        .filter_map(|update| match update {
            Update::Interrupt(interrupt) => Some(interrupt),
            _ => None,
        })
        .collect();

    assert_eq!(interrupts.len(), 1, "{interrupts:?}");
    let interrupt = interrupts.into_iter().next().expect("one interrupt");
    (session, interrupt)
}

#[tokio::test(flavor = "multi_thread")]
async fn an_interrupt_outcome_reaches_the_client_with_every_field_intact() {
    let (session, interrupt) = pause().await;

    assert_eq!(interrupt, request(), "the interrupt must survive the wire");
    assert!(interrupt.is_tool_approval());
    assert_eq!(session.interrupts(), [request()]);
    assert_eq!(
        last_reply(&session),
        "Deploying to production needs a human."
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn the_run_ends_as_interrupted_rather_than_successful() {
    let url = serve(Deployer).await;
    let mut session = Session::<_>::new(transport(&url), "deploy");
    let updates = drain!(session.send("ship build 42"));

    match updates.last() {
        Some(Update::Done(RunEnd::Interrupted { interrupts })) => {
            assert_eq!(interrupts.as_slice(), [request()]);
        }
        other => panic!("a paused run must end as Interrupted, not {other:?}"),
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn a_resolved_answer_resumes_the_run_and_reaches_ctx_resume() {
    let (mut session, interrupt) = pause().await;

    let updates = drain!(session.resume(&interrupt, json!({"build": 42})));

    assert_eq!(
        updates
            .last()
            .map(|update| matches!(update, Update::Done(RunEnd::Success { .. }))),
        Some(true),
        "{updates:?}"
    );
    // Only reachable by reading `answer.payload` server-side.
    assert_eq!(last_reply(&session), "Deployed build 42.");
    assert!(
        session.interrupts().is_empty(),
        "an answered interrupt is no longer pending"
    );

    // The resumed run is a run of its own, in the same thread.
    assert_eq!(
        session.applier().run_id().map(|id| id.as_str()),
        Some("deploy-run-2")
    );
    assert_eq!(session.messages().len(), 3, "{:?}", session.messages());
}

#[tokio::test(flavor = "multi_thread")]
async fn a_cancelled_answer_resumes_the_run_down_the_other_branch() {
    let (mut session, interrupt) = pause().await;

    let updates = drain!(session.cancel(&interrupt));

    assert_eq!(
        updates
            .last()
            .map(|update| matches!(update, Update::Done(RunEnd::Success { .. }))),
        Some(true),
        "{updates:?}"
    );
    // The agent took the Cancelled branch: it neither deployed nor paused
    // again, which is what it would have done had the entry gone missing.
    assert_eq!(last_reply(&session), "Left production alone.");
    assert!(session.interrupts().is_empty());
}

/// The negative case that keeps the two above honest: an answer for a different
/// interrupt id leaves the agent in its "nobody has answered" branch.
#[tokio::test(flavor = "multi_thread")]
async fn an_answer_to_a_different_interrupt_does_not_resume_this_one() {
    let (mut session, _interrupt) = pause().await;
    let stray = Interrupt::new("some-other-question", "tool_approval");

    let updates = drain!(session.resume_many([stray.resolve(json!({"build": 42}))]));

    match updates.last() {
        Some(Update::Done(RunEnd::Interrupted { interrupts })) => {
            assert_eq!(interrupts.as_slice(), [request()], "it should pause again");
        }
        other => panic!("an unmatched answer must not resume the run: {other:?}"),
    }
}
