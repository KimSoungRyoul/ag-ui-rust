//! A usable client workflow and its visible transcript.

use std::error::Error;
use std::io::Write;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use ag_ui::client::{HttpAgent, InterruptExt, RunEnd, RunReport, SubagentStatus, Thread};
use ag_ui::{Event, Message};
use futures_util::StreamExt;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::ReviewState;

/// Application errors include SDK validation and ordinary storage/I/O failures.
pub type AppResult<T> = Result<T, Box<dyn Error + Send + Sync>>;

/// A presentation of the SDK's assembled message, without parsing wire chunks.
#[derive(Debug, Serialize, Deserialize)]
pub struct DisplayMessage {
    pub thread: String,
    pub speaker: String,
    pub invocation: Option<String>,
    pub text: String,
}

/// A presentation of an SDK subagent registry entry.
#[derive(Debug, Serialize, Deserialize)]
pub struct DisplayChild {
    pub thread: String,
    pub name: String,
    pub invocation: String,
    pub parent: Option<String>,
    pub status: String,
    pub result: Option<Value>,
    pub error: Option<String>,
}

/// Results from the actual HTTP workflow, suitable for displaying or checking.
#[derive(Debug, Serialize, Deserialize)]
pub struct Evidence {
    /// The state belongs to the first conversation only.
    pub first: ReviewState,
    /// A second conversation stays independent on the same HttpAgent.
    pub second: ReviewState,
    /// Final state restored from serialized SDK data.
    pub restored: ReviewState,
    /// Raw progress events survive high-level automatic state updates.
    pub progress: Vec<u64>,
    /// Which event scopes ended and how.
    pub subagents: Vec<DisplayChild>,
    /// Final readable messages assembled by Thread, with resolved attribution.
    pub messages: Vec<DisplayMessage>,
    /// Count of A2UI operation batches received over HTTP.
    pub surface_batches: usize,
    /// The reconstructed final A2UI data, using the SDK history helper.
    pub surface_data: Value,
    /// Number of calls the SDK's correction loop made to the async provider.
    pub generation_attempts: u64,
    /// Original and newly generated message IDs remain unique after restore.
    pub unique_message_ids: bool,
    /// A partial approval failed before it could change pending decisions.
    pub partial_approval_rejected: bool,
    /// An outstanding run ended through its explicit abort handle.
    pub aborted: bool,
}

/// Exercises the same public calls a UI would use, over the supplied endpoint.
///
/// The snapshot is encoded as bytes and decoded before restoration. Persistence
/// itself belongs to the application, so the CLI optionally writes those bytes.
pub async fn workflow(url: &str, output: &mut impl Write) -> AppResult<(Evidence, Vec<u8>)> {
    workflow_observed(url, output, None).await
}

/// Optional read-only raw event observer used by the browser's live event log.
pub type Observer = Arc<dyn Fn(&Event) + Send + Sync>;

/// Same workflow as the terminal, with a queue-backed event observer for the UI.
pub async fn workflow_observed(
    url: &str,
    output: &mut impl Write,
    observer: Option<Observer>,
) -> AppResult<(Evidence, Vec<u8>)> {
    let agent = HttpAgent::new(url)?;
    let mut first = agent.thread_with_state("review-primary", ReviewState::default())?;
    let mut second = agent.thread_with_state("review-secondary", ReviewState::default())?;
    let observed = Arc::new(Mutex::new(Vec::<Event>::new()));
    let sink = Arc::clone(&observed);
    let live = observer.clone();
    first.on_event(move |event| {
        sink.lock().expect("observer lock").push(event.clone());
        if let Some(observer) = &live {
            observer(event);
        }
    });
    if let Some(observer) = observer.clone() {
        second.on_event(move |event| observer(event));
    }

    writeln!(output, "review-desk · {url}")?;
    let report = first.send("review release notes")?.collect_report().await;
    ensure_clean(&report)?;
    print_messages(output, &report.new_messages)?;
    writeln!(output, "state · {}", serde_json::to_string(first.state()?)?)?;

    let report = second
        .send("review broken dependency")?
        .collect_report()
        .await;
    ensure_clean(&report)?;
    writeln!(output, "other thread · {}", second.state()?.title)?;
    for child in second.subagents() {
        writeln!(output, "child · {} · {:?}", child.name, child.status)?;
    }

    let report = first.send("approve")?.collect_report().await;
    if !matches!(report.end, RunEnd::Interrupted { .. }) {
        return Err("review did not request decisions".into());
    }
    let pending = first.interrupts().to_vec();
    let partial_approval_rejected = first
        .resume(&pending[0], json!({"approved": true}))
        .is_err()
        && first.interrupts() == pending;
    writeln!(
        output,
        "approval · partial answer rejected: {partial_approval_rejected}"
    )?;

    let report = first
        .resume_many([
            pending[0].resolve(json!({"approved": true})),
            pending[1].cancel(),
        ])?
        .collect_report()
        .await;
    ensure_clean(&report)?;
    print_messages(output, &report.new_messages)?;

    let snapshot = serde_json::to_vec_pretty(&first.snapshot())?;
    let mut restored =
        agent.restore_thread_with_state::<ReviewState>(serde_json::from_slice(&snapshot)?)?;
    let generation_attempts = Arc::new(AtomicU64::new(0));
    let attempts = generation_attempts.clone();
    restored.on_event(move |event| {
        if let Event::Custom(custom) = event {
            if custom.name == "a2ui-generation" {
                attempts.store(
                    custom.value["attempts"].as_u64().unwrap_or(0),
                    Ordering::Relaxed,
                );
            }
        }
        if let Some(observer) = &observer {
            observer(event);
        }
    });
    let report = restored.send("status")?.collect_report().await;
    ensure_clean(&report)?;
    let mut ids = std::collections::HashSet::new();
    let unique_message_ids = restored
        .messages()
        .iter()
        .all(|message| ids.insert(message.id().clone()));
    writeln!(
        output,
        "restore · {} messages · unique IDs: {unique_message_ids}",
        ids.len()
    )?;

    let report = restored.send("surface")?.collect_report().await;
    ensure_clean(&report)?;
    let surface_batches = report
        .new_messages
        .iter()
        .filter(|message| {
            let Message::Tool(tool) = message else {
                return false;
            };
            serde_json::from_str(&tool.content)
                .is_ok_and(|value| ag_ui_a2ui::toolkit::envelope::is_operations_envelope(&value))
        })
        .count();
    let surface_data = ag_ui_a2ui::find_prior_surface_in(restored.messages())
        .ok_or("A2UI surface absent from conversation history")?
        .data_model
        .to_json()?;
    writeln!(
        output,
        "A2UI · {surface_batches} validated create/edit batches received"
    )?;

    let mut run = restored.send("slow")?;
    let abort = run.abort_handle();
    // Wait until the server's real message arrives. No timer decides whether
    // the test saw a running request; the event itself is the synchronization.
    while let Some(update) = run.next().await {
        if matches!(update, ag_ui::client::Update::Message(_)) {
            abort.abort();
            break;
        }
    }
    let report = run.collect_report().await;
    let aborted = matches!(report.end, RunEnd::Aborted);
    writeln!(output, "run · {:?}", report.end)?;
    ensure_clean(&restored.send("status")?.collect_report().await)?;

    let events = observed.lock().expect("observer lock");
    let progress = events
        .iter()
        .filter_map(|event| match event {
            Event::Custom(custom) if custom.name == "progress" => custom.value["percent"].as_u64(),
            _ => None,
        })
        .collect();
    let subagents: Vec<_> = display_children(&first)
        .chain(display_children(&second))
        .collect();
    writeln!(output, "progress · {progress:?}")?;
    for child in &subagents {
        writeln!(
            output,
            "child · {} · {} · {} · result {:?} · error {:?}",
            child.name, child.invocation, child.status, child.result, child.error
        )?;
    }

    Ok((
        Evidence {
            first: first.state()?.clone(),
            second: second.state()?.clone(),
            restored: restored.state()?.clone(),
            progress,
            subagents,
            messages: display_messages(&restored)
                .chain(display_messages(&second))
                .collect(),
            surface_batches,
            surface_data,
            generation_attempts: generation_attempts.load(Ordering::Relaxed),
            unique_message_ids,
            partial_approval_rejected,
            aborted,
        },
        snapshot,
    ))
}

fn display_messages<T, S>(thread: &Thread<T, S>) -> impl Iterator<Item = DisplayMessage> + '_ {
    thread.messages().iter().filter_map(|message| {
        let Message::Assistant(assistant) = message else {
            return None;
        };
        let text = assistant.content.as_ref()?.clone();
        if text.is_empty() {
            return None;
        }
        let invocation = message.subagent_run_id();
        let speaker = invocation
            .and_then(|id| thread.subagent(id))
            .map_or("supervisor", |child| child.name.as_str());
        Some(DisplayMessage {
            thread: thread.thread_id().to_string(),
            speaker: speaker.into(),
            invocation: invocation.map(ToString::to_string),
            text,
        })
    })
}

fn display_children<T, S>(thread: &Thread<T, S>) -> impl Iterator<Item = DisplayChild> + '_ {
    thread.subagents().iter().map(|child| {
        let (status, result, error) = match &child.status {
            SubagentStatus::Running => ("running", None, None),
            SubagentStatus::Finished { result } => ("finished", result.clone(), None),
            SubagentStatus::Suspended { result, .. } => ("suspended", result.clone(), None),
            SubagentStatus::Failed { message, .. } => ("failed", None, Some(message.clone())),
            SubagentStatus::Aborted => ("aborted", None, None),
            _ => ("stopped", None, None),
        };
        DisplayChild {
            thread: thread.thread_id().to_string(),
            name: child.name.clone(),
            invocation: child.run_id.to_string(),
            parent: child
                .parent_subagent_run_id
                .as_ref()
                .map(ToString::to_string),
            status: status.into(),
            result,
            error,
        }
    })
}

fn ensure_clean(report: &RunReport) -> AppResult<()> {
    if !report.diagnostics.is_empty() || report.diagnostics_omitted > 0 {
        return Err(format!(
            "run diagnostics: {:?}; omitted: {}",
            report.diagnostics, report.diagnostics_omitted
        )
        .into());
    }
    let end = &report.end;
    if matches!(end, RunEnd::Success { .. }) {
        Ok(())
    } else {
        Err(format!("unexpected run outcome: {end:?}").into())
    }
}

fn print_messages(output: &mut impl Write, messages: &[Message]) -> std::io::Result<()> {
    for message in messages {
        if let Message::Assistant(assistant) = message {
            if let Some(text) = &assistant.content {
                writeln!(output, "assistant · {text}")?;
            }
        }
    }
    Ok(())
}
