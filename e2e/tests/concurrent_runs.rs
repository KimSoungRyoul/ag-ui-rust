//! Many clients, one mounted agent, all in flight at once.
//!
//! Nothing behind the endpoint is shared mutable state, and the proof is that
//! every session ends up with exactly its own events and nobody else's. The
//! overlap is not left to luck: each run stops at a shared barrier that only
//! releases once every run has reached it, so a server that answered one
//! request at a time would deadlock here rather than pass slowly.

mod common;

use std::sync::Arc;
use std::time::Duration;

use ag_ui_client::{RunEnd, Session, Update};
use ag_ui_core::{Message, RunOutcome, UserContent};
use ag_ui_server::{Agent, Result, RunContext};
use common::{serve, transport};
use futures_util::StreamExt as _;
use serde::{Deserialize, Serialize};
use tokio::sync::Barrier;
use tokio::time::timeout;

/// Enough to interleave properly, few enough to stay quick.
const CLIENTS: usize = 8;

/// Replies per run.
const REPLIES: usize = 4;

/// A deadline, because the failure mode being ruled out is a server that
/// serializes runs — which presents as a hang, not as a wrong answer.
const DEADLINE: Duration = Duration::from_secs(20);

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
struct Tally {
    thread: String,
    replies: u32,
}

/// Echoes the caller's own question back, stamped with the ids of *this* run,
/// and waits for every other run to get here first.
struct Echo {
    barrier: Arc<Barrier>,
}

impl Agent for Echo {
    type State = Tally;

    async fn run(&self, ctx: &mut RunContext<Tally>) -> Result<RunOutcome> {
        let question = last_question(ctx);
        let thread = ctx.thread_id().to_string();
        let run = ctx.run_id().to_string();

        let mut step = ctx.step("answer")?;
        for index in 0..REPLIES {
            let mut message = step.assistant_message()?;
            message.delta(format!("{thread}/{run}#{index}: "))?;
            message.delta(question.clone())?;
            message.end()?;

            // Halfway through, wait for everyone. Every run has to be open at
            // the same time for this to return at all.
            if index == REPLIES / 2 {
                self.barrier.wait().await;
            }
        }

        step.update_state(|tally| {
            tally.thread.clone_from(&thread);
            tally.replies = REPLIES as u32;
        })?;

        drop(step);
        Ok(RunOutcome::Success)
    }
}

/// The most recent thing the user said.
fn last_question(ctx: &RunContext<Tally>) -> String {
    ctx.messages()
        .iter()
        .rev()
        .find_map(|message| match message {
            Message::User(user) => match &user.content {
                UserContent::Text(text) => Some(text.clone()),
                UserContent::Parts(_) => None,
            },
            _ => None,
        })
        .unwrap_or_default()
}

/// What one client got back.
struct Transcript {
    thread: String,
    replies: Vec<String>,
    state: Option<Tally>,
    ended: RunEnd,
}

/// Runs every client at once against one served agent.
async fn run_all() -> Vec<Transcript> {
    let barrier = Arc::new(Barrier::new(CLIENTS));
    let url = serve(Echo {
        barrier: Arc::clone(&barrier),
    })
    .await;

    let clients: Vec<_> = (0..CLIENTS)
        .map(|index| {
            let url = url.clone();
            tokio::spawn(async move {
                let thread = format!("thread-{index}");
                let mut session = Session::<_, Tally>::new(transport(&url), thread.clone());

                let mut ended = None;
                {
                    let mut run = session.send(format!("question from {thread}"));
                    while let Some(update) = run.next().await {
                        match update {
                            Update::Done(end) => ended = Some(end),
                            Update::Error(error) => panic!("{thread}: {error}"),
                            _ => {}
                        }
                    }
                }

                let replies = session
                    .messages()
                    .iter()
                    .filter_map(|message| match message {
                        Message::Assistant(assistant) => assistant.content.clone(),
                        _ => None,
                    })
                    .collect();

                Transcript {
                    thread,
                    replies,
                    state: session.state().cloned(),
                    ended: ended.expect("every run ends"),
                }
            })
        })
        .collect();

    let mut transcripts = Vec::with_capacity(CLIENTS);
    for client in clients {
        transcripts.push(
            timeout(DEADLINE, client)
                .await
                .expect("concurrent runs must not serialize")
                .expect("no client task should panic"),
        );
    }
    transcripts
}

#[tokio::test(flavor = "multi_thread")]
async fn concurrent_clients_each_get_only_their_own_events() {
    let transcripts = run_all().await;
    assert_eq!(transcripts.len(), CLIENTS);

    for transcript in &transcripts {
        let thread = &transcript.thread;
        assert_eq!(transcript.ended, RunEnd::Success { result: None });
        assert_eq!(
            transcript.replies.len(),
            REPLIES,
            "{thread}: {:?}",
            transcript.replies
        );

        for (index, reply) in transcript.replies.iter().enumerate() {
            assert_eq!(
                reply,
                &format!("{thread}/{thread}-run-1#{index}: question from {thread}"),
                "a reply from another run leaked into {thread}"
            );
        }

        // …and no other client's thread id appears anywhere in this transcript.
        for other in &transcripts {
            if other.thread != *thread {
                assert!(
                    !transcript
                        .replies
                        .iter()
                        .any(|reply| reply.contains(&other.thread)),
                    "{thread} saw {}'s events",
                    other.thread
                );
            }
        }
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn concurrent_runs_keep_their_state_to_themselves() {
    let transcripts = run_all().await;

    for transcript in &transcripts {
        assert_eq!(
            transcript.state,
            Some(Tally {
                thread: transcript.thread.clone(),
                replies: REPLIES as u32,
            }),
            "{} ended up with someone else's state",
            transcript.thread
        );
    }
}
