//! The low level: events exactly as the agent sent them.
//!
//! [`Session`](ag_ui::client::Session) assembles; this does not. A proxy, a
//! recorder, a bridge to another protocol and a person debugging a stream all
//! want the events unassembled, and that is what
//! [`RemoteAgent`](ag_ui::client::RemoteAgent) is for.
//!
//! Including the human-in-the-loop round trip: [`interrupts_of`] reads what a
//! `RUN_FINISHED` paused on and [`resume_run`] builds the request that answers
//! it, so pausing and resuming needs no session at all — only the previous
//! request, which the caller already has.

use std::io::{self, Write};

use ag_ui::client::{HttpAgent, InterruptExt as _, RunParams, interrupts_of, resume_run};
use ag_ui::{Event, EventType, ResumeEntry, RunAgentInput, Tool};
use futures_util::StreamExt as _;
use serde_json::{Value, json};

/// Streams one run and prints every event, then answers any pause and streams
/// the resumed run too.
///
/// `approve` decides what a pause is answered with, and `tools` is what the
/// agent is offered — an agent that needs one it was not sent fails the run.
/// Returns how many events were printed across every run it drove.
pub async fn trace(
    agent: &HttpAgent,
    thread: &str,
    said: &str,
    tools: Vec<Tool>,
    approve: bool,
    out: &mut impl Write,
) -> io::Result<usize> {
    let mut input: RunAgentInput = RunParams::new(thread, format!("{thread}-run-1"))
        .user(format!("{thread}-msg-1"), said)
        .tools(tools)
        .into();

    let mut printed = 0;
    let mut round = 1;

    loop {
        writeln!(out, "--- run {round} · {}", input.run_id)?;
        let (count, paused) = stream_once(agent, input.clone(), approve, out).await?;
        printed += count;

        let Some(entries) = paused else {
            return Ok(printed);
        };
        // A resumed run is a run of its own: same conversation, same state, new
        // id. `resume_run` carries the rest over, so a caller cannot forget a
        // field the agent needs.
        round += 1;
        input = resume_run(&input, format!("{thread}-run-{round}"), entries);
    }
}

/// Prints one run's events, reporting the answers its pause needs.
async fn stream_once(
    agent: &HttpAgent,
    input: RunAgentInput,
    approve: bool,
    out: &mut impl Write,
) -> io::Result<(usize, Option<Vec<ResumeEntry>>)> {
    let mut events = agent.run(input);
    let mut printed = 0;
    let mut resume = None;

    while let Some(event) = events.next().await {
        let event = match event {
            Ok(event) => event,
            // A transport that cannot reach the agent yields one error item and
            // ends, so this is the whole of the failure path.
            Err(error) => {
                writeln!(out, "  !! {error}")?;
                break;
            }
        };
        printed += 1;
        writeln!(
            out,
            "  {:<26} {}",
            name(event.event_type()),
            payload(&event)
        )?;

        let interrupts = interrupts_of(&event);
        if !interrupts.is_empty() {
            resume = Some(
                interrupts
                    .iter()
                    .map(|interrupt| {
                        if approve {
                            interrupt.resolve(json!({"confirm": true}))
                        } else {
                            interrupt.cancel()
                        }
                    })
                    .collect(),
            );
        }
    }
    Ok((printed, resume))
}

/// The wire name of an event type.
fn name(kind: EventType) -> String {
    serde_json::to_value(kind)
        .ok()
        .and_then(|value| value.as_str().map(str::to_owned))
        .unwrap_or_else(|| format!("{kind:?}"))
}

/// The event's fields, minus the two every event carries.
fn payload(event: &Event) -> String {
    let Ok(Value::Object(mut fields)) = serde_json::to_value(event) else {
        return String::new();
    };
    fields.remove("type");
    fields.remove("timestamp");
    serde_json::to_string(&fields).unwrap_or_default()
}
