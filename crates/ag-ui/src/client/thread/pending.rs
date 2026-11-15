//! What a conversation owes before another run: an answer to each interrupt
//! it is paused on, a submission the server has not confirmed, and a result
//! for each tool call nothing answered.

use std::collections::HashSet;

use crate::client::{
    error::{Error, Result},
    interrupts::InterruptExt,
    transport::Transport,
};
use crate::{
    Interrupt, Message, ResumeEntry, ResumeStatus, RunId, ToolCallId, ToolContent, ToolMessage,
};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::Thread;
use super::run::{RunEnd, RunStream};

/// Local observation of an interrupt response submission.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct ResumeSubmission {
    /// The request that carried these responses.
    pub run_id: RunId,
    /// Exact answers, retained for reconciliation by the application.
    pub entries: Vec<ResumeEntry>,
    /// Whether a valid terminal response has confirmed the request.
    pub status: SubmissionStatus,
}

/// A submission is never retried automatically.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum SubmissionStatus {
    /// Dispatched and still being observed. Restoring this becomes `Unconfirmed`.
    InFlight,
    /// Consumption stopped without a validated success or interrupt terminal.
    Unconfirmed,
}

impl<T, S> Thread<T, S> {
    pub(super) fn unconfirm(&mut self) {
        if let Some(submission) = &mut self.submission {
            submission.status = SubmissionStatus::Unconfirmed;
        }
    }
    pub(super) fn confirm_interrupt_tool_decisions(
        &mut self,
        started_in_run: &HashSet<ToolCallId>,
    ) {
        let Some(submission) = &self.submission else {
            return;
        };
        for interrupt in &self.interrupts {
            if submission
                .entries
                .iter()
                .any(|entry| entry.interrupt_id == interrupt.id)
            {
                if let Some(id) = &interrupt.tool_call_id {
                    if started_in_run.contains(id) {
                        continue;
                    }
                    self.decided_interrupt_tool_call_ids.insert(id.clone());
                }
            }
        }
    }
}

impl<T: Transport, S> Thread<T, S> {
    /// Answer the single outstanding interrupt. All pending IDs must be answered.
    pub fn resume(
        &mut self,
        interrupt: &Interrupt,
        payload: impl Into<Value>,
    ) -> Result<RunStream<'_, T, S>> {
        self.resume_many([interrupt.resolve(payload)])
    }
    /// Decline the single outstanding interrupt. This is not execution cancellation.
    ///
    /// An expired interrupt can still be declined: that is how a thread moves
    /// past a question nobody answered in time.
    pub fn decline(&mut self, interrupt: &Interrupt) -> Result<RunStream<'_, T, S>> {
        self.resume_many([interrupt.cancel()])
    }
    /// Answer every pending question exactly once. Expiry is checked against the
    /// stored interrupt: an expired one can only be cancelled. Response schemas
    /// are retained for the application/server to validate; this
    /// transport-neutral API does not execute arbitrary schemas.
    pub fn resume_many(
        &mut self,
        entries: impl IntoIterator<Item = ResumeEntry>,
    ) -> Result<RunStream<'_, T, S>> {
        let entries: Vec<_> = entries.into_iter().collect();
        self.preflight(Some(&entries), None)?;
        self.start(None, Some(entries))
    }
    pub(super) fn preflight(
        &self,
        resume: Option<&[ResumeEntry]>,
        message: Option<&Message>,
    ) -> Result<()> {
        if self.thread_id.as_str().is_empty() {
            return Err(Error::Request("thread ID must not be empty".into()));
        }
        if self.submission.is_some() {
            return Err(Error::Request("an unconfirmed submission requires reconciliation with server state before another run".into()));
        }
        match resume {
            None if !self.interrupts.is_empty() => {
                return Err(Error::PendingInterrupts(
                    self.interrupts
                        .iter()
                        .map(|interrupt| interrupt.id.clone())
                        .collect(),
                ));
            }
            Some(entries) => validate_responses(&self.interrupts, entries, true)?,
            None => {}
        }
        let mut pending_tools = self.unanswered_tool_calls();
        if let Some(Message::Tool(answer)) = message {
            // The new answer enters the request on first poll. Check that
            // prospective history without inserting it during preflight.
            pending_tools.retain(|id| id != &answer.tool_call_id);
        }
        if resume.is_some() {
            // A matching resume entry answers the interrupt, including an
            // approval concerning this tool call. It needs no ToolMessage.
            let approval_ids: HashSet<_> = self
                .interrupts
                .iter()
                .filter_map(|interrupt| interrupt.tool_call_id.as_ref())
                .collect();
            pending_tools.retain(|id| !approval_ids.contains(id));
        }
        if !pending_tools.is_empty() {
            return Err(Error::UnansweredToolCalls(pending_tools));
        }
        if let Some(id) = &self.next_run_id {
            if id.as_str().is_empty() || self.run_ids.contains(id) {
                return Err(Error::Request(format!(
                    "run ID {id} is empty or already used"
                )));
            }
        }
        Ok(())
    }
    /// Tool calls the next run must answer first, in call order.
    ///
    /// Every call in the transcript that no later tool message answers: the
    /// frontend calls a run left for the application, and calls a failed or
    /// aborted run never answered. `send`, `run` and `resume_many` refuse to
    /// start a run while one is listed — AG-UI 1.0 requires a thread that
    /// continues to answer each one first, and many models reject a history
    /// with a dangling call. Answer one with a tool message
    /// ([`push_message`](Thread::push_message) or [`send_message`](Self::send_message)),
    /// or give it up with [`abandon_tool_call`](Self::abandon_tool_call). A call
    /// a pending interrupt concerns is answered by resuming that interrupt.
    ///
    /// ```
    /// use ag_ui::client::{Thread, transport::ReplayTransport};
    /// use ag_ui::{AssistantMessage, Message, ToolCall};
    ///
    /// let call = Message::Assistant(AssistantMessage {
    ///     id: "m-1".into(),
    ///     tool_calls: Some(vec![ToolCall::new("call-1", "pick_file", "{}")]),
    ///     ..Default::default()
    /// });
    /// let mut thread = Thread::<_>::builder(ReplayTransport::new([]), "thread-1")
    ///     .messages(vec![call])
    ///     .build()?;
    /// assert_eq!(thread.unanswered_tool_calls(), ["call-1"]);
    /// assert!(thread.send("Hello").is_err());
    ///
    /// thread.push_message(Message::tool("m-2", "call-1", "report.pdf"))?;
    /// assert!(thread.unanswered_tool_calls().is_empty());
    /// # Ok::<(), Box<dyn std::error::Error>>(())
    /// ```
    pub fn unanswered_tool_calls(&self) -> Vec<ToolCallId> {
        // Read the ordered transcript, because an answer preceding a call does
        // not answer that call. A server-side TOOL_CALL_RESULT is already a tool
        // message here, so it clears the same debt as a frontend response.
        let mut pending = Vec::new();
        let mut seen = HashSet::new();
        let mut answered = HashSet::new();
        for message in self.messages() {
            match message {
                Message::Assistant(assistant) => {
                    for call in assistant.tool_calls.iter().flatten() {
                        answered.remove(&call.id);
                        if seen.insert(call.id.clone()) {
                            pending.push(call.id.clone());
                        }
                    }
                }
                Message::Tool(tool) => {
                    answered.insert(tool.tool_call_id.clone());
                }
                _ => {}
            }
        }
        if let Some(RunEnd::SuccessWithPendingToolCalls {
            pending_tool_call_ids,
            ..
        }) = &self.last_run_end
        {
            for id in pending_tool_call_ids {
                if seen.insert(id.clone()) {
                    pending.push(id.clone());
                }
            }
        }
        let explicit_pending = match &self.last_run_end {
            Some(RunEnd::SuccessWithPendingToolCalls {
                pending_tool_call_ids,
                ..
            }) => pending_tool_call_ids.iter().collect::<HashSet<_>>(),
            _ => HashSet::new(),
        };
        pending.retain(|id| {
            !answered.contains(id)
                && (!self.decided_interrupt_tool_call_ids.contains(id)
                    || explicit_pending.contains(id))
        });
        pending
    }
    /// Give up an unanswered tool call, answering it with a tool message whose
    /// `content` and `error` both carry `reason`.
    ///
    /// For a call nobody will execute, such as one a failed or aborted run left
    /// behind. AG-UI 1.0 answers such a call rather than dropping it: a failure
    /// is still an answer, as a tool message with `error` set. The call and its
    /// answer both stay in [`messages`](Thread::messages) and in a snapshot,
    /// and the next run sends them. Nothing is sent now.
    ///
    /// Rejects a call that is not in [`unanswered_tool_calls`](Self::unanswered_tool_calls),
    /// and one a pending interrupt concerns: resume or decline that interrupt
    /// instead.
    ///
    /// ```
    /// # use ag_ui::client::{RunEnd, Thread, transport::ReplayTransport};
    /// # use ag_ui::{Event, Message};
    /// # let transport = ReplayTransport::with_runs([
    /// #     vec![
    /// #         Event::run_started("thread-1", "run-1"),
    /// #         Event::tool_call_start("call-1", "lookup"),
    /// #         Event::run_error("the model provider went away"),
    /// #     ],
    /// #     vec![
    /// #         Event::run_started("thread-1", "run-2"),
    /// #         Event::run_finished_success("thread-1", "run-2"),
    /// #     ],
    /// # ]).matching_requests();
    /// # let rt = tokio::runtime::Builder::new_current_thread().build().unwrap();
    /// # rt.block_on(async {
    /// let mut thread = Thread::<_>::new(transport, "thread-1");
    /// let report = thread.send("look it up").unwrap().collect_report().await;
    /// assert!(matches!(report.end, RunEnd::Failed { .. }));
    ///
    /// // The failed run left `call-1` without a result, so nothing can follow it ...
    /// assert!(thread.send("try again").is_err());
    /// for call in thread.unanswered_tool_calls() {
    ///     thread.abandon_tool_call(&call, "not run: the run that made this call failed").unwrap();
    /// }
    /// // ... until it is answered, and the answer stays in the transcript.
    /// assert!(matches!(thread.messages().last(), Some(Message::Tool(answer)) if answer.error.is_some()));
    /// let report = thread.send("try again").unwrap().collect_report().await;
    /// assert_eq!(report.end, RunEnd::Success { result: None });
    /// # });
    /// ```
    pub fn abandon_tool_call(&mut self, id: &ToolCallId, reason: impl Into<String>) -> Result<()> {
        if !self.unanswered_tool_calls().contains(id) {
            return Err(Error::Request(format!(
                "tool call {id} is not awaiting an answer"
            )));
        }
        if let Some(interrupt) = self
            .interrupts
            .iter()
            .find(|interrupt| interrupt.tool_call_id.as_ref() == Some(id))
        {
            return Err(Error::Request(format!(
                "tool call {id} awaits the decision of interrupt {}; resume or decline it instead",
                interrupt.id
            )));
        }
        let reason = reason.into();
        let message_id = self.fresh_id(false)?;
        self.applier.push_message(Message::Tool(ToolMessage {
            id: message_id.into(),
            content: ToolContent::Text(reason.clone()),
            tool_call_id: id.clone(),
            error: Some(reason),
            ..Default::default()
        }));
        Ok(())
    }
}

pub(super) fn validate_responses(
    pending: &[Interrupt],
    entries: &[ResumeEntry],
    expiry: bool,
) -> Result<()> {
    if pending.is_empty() {
        return Err(Error::Request("there are no pending interrupts".into()));
    }
    let mut seen = HashSet::new();
    for entry in entries {
        if !seen.insert(&entry.interrupt_id) {
            return Err(Error::Request(format!(
                "duplicate interrupt ID {}",
                entry.interrupt_id
            )));
        }
        let interrupt = pending
            .iter()
            .find(|interrupt| interrupt.id == entry.interrupt_id)
            .ok_or_else(|| {
                Error::Request(format!("unknown interrupt ID {}", entry.interrupt_id))
            })?;
        // Expiry ends answering, not abandoning: an interrupt nobody answered
        // in time is cancelled, which is how the thread moves past it.
        if expiry && entry.status == ResumeStatus::Resolved {
            if let Some(timestamp) = &interrupt.expires_at {
                let expires = chrono::DateTime::parse_from_rfc3339(timestamp).map_err(|_| {
                    Error::Request(format!(
                        "interrupt {} has an invalid expiry timestamp; it can only be declined",
                        interrupt.id
                    ))
                })?;
                // RFC 3339 leap seconds can only end a UTC month.
                if expires.timestamp_subsec_nanos() >= 1_000_000_000 {
                    let utc_date = expires.naive_utc().date();
                    let ends_month = utc_date
                        .succ_opt()
                        .is_some_and(|next| chrono::Datelike::day(&next) == 1);
                    if expires.timestamp().rem_euclid(86_400) != 86_399 || !ends_month {
                        return Err(Error::Request(format!(
                            "interrupt {} has an invalid expiry timestamp; it can only be declined",
                            interrupt.id
                        )));
                    }
                }
                // Preserve the previous parser's leap-second clamp and full year range.
                let expires_nanos = i128::from(expires.timestamp()) * 1_000_000_000
                    + i128::from(expires.timestamp_subsec_nanos().min(999_999_999));
                if expires_nanos <= now_unix_nanos() {
                    return Err(Error::Request(format!(
                        "interrupt {} has expired; it can only be declined",
                        interrupt.id
                    )));
                }
            }
        }
    }
    if seen.len() != pending.len() {
        return Err(Error::Request(
            "every pending interrupt must be answered exactly once".into(),
        ));
    }
    Ok(())
}

#[cfg(not(target_family = "wasm"))]
fn now_unix_nanos() -> i128 {
    match std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH) {
        Ok(elapsed) => elapsed.as_nanos() as i128,
        Err(error) => -(error.duration().as_nanos() as i128),
    }
}
#[cfg(target_family = "wasm")]
fn now_unix_nanos() -> i128 {
    (js_sys::Date::now() * 1_000_000.0) as i128
}
