//! One run of a conversation: the stream it is read as, and what that stream yields.

use std::collections::{HashSet, VecDeque};
use std::pin::Pin;
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};
use std::task::{Context as TaskContext, Poll};

use crate::client::{
    apply::{
        Changed, MessageChangeKind, ReasoningChange, ReasoningChangeKind, Subagent,
        SubagentChangeKind,
    },
    chunks::ChunkNormalizer,
    enforce::{Checked, CheckedStream},
    error::{Error, Result},
    transport::Transport,
    verify::Verifier,
};
use crate::{
    Event, Interrupt, Message, MessageId, ResumeEntry, RunAgentInput, RunId, RunOutcome, ThreadId,
    ToolCallId,
};
use futures_core::Stream;
use futures_util::{StreamExt, task::AtomicWaker};
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use serde_json::Value;

use super::pending::{ResumeSubmission, SubmissionStatus, validate_responses};
use super::{StateViewError, Thread, unique};

/// Something a view should react to.
///
/// One [`Update`] is one redraw. `S` is the caller's state type; it is
/// [`serde_json::Value`] unless a [`Thread`] is asked for something better.
#[derive(Debug)]
#[non_exhaustive]
pub enum Update<S = Value> {
    /// A message was created, appended to, or completed.
    Message(MessageUpdate),
    /// `MESSAGES_SNAPSHOT` replaced the conversation. Messages may have
    /// disappeared, so redraw all of it.
    Messages(Vec<Message>),
    /// The application state changed, and here it is in the caller's type.
    ///
    /// Carries no association with whatever was open when it arrived — a tool
    /// call, a message — because the wire carries none either. Where it lands
    /// in the stream is the only nesting there is; see the [module
    /// docs](super).
    State(S),
    /// A reasoning message was created, appended to, or completed. It is in
    /// [`Thread::messages`] like any other message, and is announced here and
    /// not as an [`Update::Message`], so a view can draw reasoning apart
    /// from the reply, or leave it out.
    ///
    /// The span around reasoning messages, `REASONING_START` to
    /// `REASONING_END`, opens no message and yields no update. An application
    /// that needs to know one is open reads those two events in
    /// [`Thread::on_event`].
    Reasoning(ReasoningUpdate),
    /// A subagent was announced, resumed, finished, suspended or failed.
    ///
    /// The lifecycle only: what a subagent *says* arrives as ordinary
    /// [`Update::Message`]s and [`Update::Reasoning`]s whose messages carry
    /// its `subagent_run_id`, so a view groups by that and uses this for the
    /// group's header and status. See [`Thread::subagents`].
    Subagent(SubagentUpdate),
    /// The run paused and needs a human. Answer it with
    /// [`Thread::resume`] — one update per pending interrupt.
    Interrupt(Interrupt),
    /// Something went wrong: a malformed stream, a patch that would not apply,
    /// a transport failure, a `RUN_ERROR`.
    ///
    /// Not necessarily fatal — a run survives a patch it could not apply. When
    /// it is fatal, the matching [`Update::Done`] follows.
    Error(Error),
    /// Not an error: the stream was accepted, with a correction or an omission
    /// the application may want to log. An unrecognised field, enum member or
    /// event was dropped, a retired `THINKING_*` event was translated, a
    /// deprecated `null` was read as omitted, or the producer declared a newer
    /// protocol version. The run is unaffected.
    ///
    /// Each text is delivered once per run and repeats at most 64 characters
    /// of input, with control characters escaped. [`RunReport::diagnostics`]
    /// keeps it with kind `warning`.
    Warning(String),
    /// The run ended, and how. Always the last update of a run, on every path
    /// out: the agent finishing, the agent failing, and the transport dying
    /// mid-sentence.
    Done(RunEnd),
}

/// A message that changed, and the message as it now stands.
#[derive(Clone, Debug, PartialEq)]
#[non_exhaustive]
pub struct MessageUpdate {
    /// Index into [`Thread::messages`].
    pub index: usize,
    /// The message's id.
    pub id: MessageId,
    /// What this event did to it — the text delta, the tool call, the close.
    pub change: MessageChangeKind,
    /// The whole message, assembled so far.
    pub message: Message,
}

/// Reasoning that changed, and the reasoning as it now stands.
#[derive(Clone, Debug, PartialEq)]
#[non_exhaustive]
pub struct ReasoningUpdate {
    /// Index into [`Thread::messages`], where the reasoning message is.
    pub index: usize,
    /// The reasoning message's id.
    pub id: MessageId,
    /// What this event did to it.
    pub change: ReasoningChangeKind,
    /// The accumulated reasoning text.
    pub text: String,
}

/// A subagent that changed, and the subagent as it now stands.
#[derive(Clone, Debug, PartialEq)]
#[non_exhaustive]
pub struct SubagentUpdate {
    /// Index into [`Thread::subagents`].
    pub index: usize,
    /// The invocation's id — what the messages it produced carry.
    pub run_id: crate::SubagentRunId,
    /// What this event did to it.
    pub change: SubagentChangeKind,
    /// The whole entry, status included.
    pub subagent: Subagent,
}

/// How the remote run ended, or how local consumption stopped.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum RunEnd {
    /// The server confirmed successful completion. Local diagnostics remain separate.
    Success {
        /// Optional server result.
        result: Option<Value>,
    },
    /// The server completed, with frontend tool calls awaiting answers.
    SuccessWithPendingToolCalls {
        /// Optional server result.
        result: Option<Value>,
        /// Calls to answer in the next request, in call order.
        pending_tool_call_ids: Vec<ToolCallId>,
    },
    /// The server paused for input.
    Interrupted {
        /// Current pending questions.
        interrupts: Vec<Interrupt>,
    },
    /// The server or transport failed, or its terminal event was invalid.
    Failed {
        /// Human-readable cause.
        message: String,
        /// Optional server error code.
        code: Option<String>,
    },
    /// The server confirmed that it stopped the run before completion.
    Cancelled,
    /// Local consumption stopped. This does not confirm cancellation on the server.
    Aborted,
}

/// One retained diagnostic, independent of the terminal outcome.
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub struct RunDiagnostic {
    /// A stable broad category (`state`, `protocol`, `patch`, `transport`, `run`,
    /// `unsupported`, or `warning` for an [`Update::Warning`]).
    pub kind: &'static str,
    /// Human-readable cause.
    pub message: String,
}

/// Results from the whole run, including updates consumed before `collect_report`.
#[derive(Clone, Debug, PartialEq)]
#[non_exhaustive]
pub struct RunReport {
    /// The observed terminal outcome.
    pub end: RunEnd,
    /// Messages whose IDs were absent when this run was prepared, reasoning
    /// messages among them.
    pub new_messages: Vec<Message>,
    /// Retained diagnostics, in observation order.
    pub diagnostics: Vec<RunDiagnostic>,
    /// Diagnostics beyond the configured retention limit.
    pub diagnostics_omitted: usize,
}

impl<T: Transport, S> Thread<T, S> {
    pub(super) fn start(
        &mut self,
        message: Option<Message>,
        resume: Option<Vec<ResumeEntry>>,
    ) -> Result<RunStream<'_, T, S>> {
        let run_id = match &self.next_run_id {
            Some(id) => id.clone(),
            None => RunId::new(self.fresh_id(true)?),
        };
        let initial_ids = self
            .messages()
            .iter()
            .map(|message| message.id().clone())
            .collect();
        // Activity messages describe local renderer state. Like the reference
        // client, retain them in the thread but omit them from agent requests.
        // Nothing else is left out: a reasoning message goes back as it
        // stands, which is what returns its `encryptedValue` to the agent.
        let messages = self
            .messages()
            .iter()
            .chain(message.iter())
            .filter(|message| !matches!(message, Message::Activity(_)))
            .cloned()
            .collect();
        let input = RunAgentInput {
            thread_id: self.thread_id.clone(),
            run_id,
            protocol_version: self.protocol_version.clone(),
            parent_run_id: None,
            state: self.raw_state().clone(),
            messages,
            tools: self.tools.clone(),
            context: self.context.clone(),
            forwarded_props: self.forwarded_props.clone(),
            resume,
        };
        let verifier = self.verify.then(Verifier::new);
        let request_run_id = input.run_id.clone();
        Ok(RunStream {
            request_run_id,
            thread: self,
            prepared: Some((input, message)),
            events: None,
            normalizer: ChunkNormalizer::new(),
            verifier,
            expanded: Vec::new(),
            ready: VecDeque::new(),
            done: false,
            dispatched: false,
            abort: AbortHandle::default(),
            end: None,
            finished: None,
            diagnostics: Vec::new(),
            diagnostics_omitted: 0,
            initial_ids,
            response_ids: None,
            started_tool_calls: HashSet::new(),
        })
    }
}

#[derive(Default)]
struct AbortState {
    aborted: AtomicBool,
    waker: AtomicWaker,
}
/// Cloneable cancellation for one local run. Wakes a pending poll and drops its
/// connection/response stream. It does not confirm remote cancellation.
#[derive(Clone, Default)]
pub struct AbortHandle {
    state: Arc<AbortState>,
}
impl std::fmt::Debug for AbortHandle {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AbortHandle")
            .field("aborted", &self.is_aborted())
            .finish()
    }
}
impl AbortHandle {
    /// Request local cancellation and wake its consumer.
    pub fn abort(&self) {
        self.state.aborted.store(true, Ordering::Release);
        self.state.waker.wake();
    }
    /// Whether cancellation was requested.
    pub fn is_aborted(&self) -> bool {
        self.state.aborted.load(Ordering::Acquire)
    }
}

/// A lazily dispatched run. It borrows the thread until dropped. Every consumed
/// run yields one `Done`; dropping an unpolled run leaves the thread unchanged.
pub struct RunStream<'a, T, S = Value> {
    thread: &'a mut Thread<T, S>,
    prepared: Option<(RunAgentInput, Option<Message>)>,
    events: Option<CheckedStream>,
    normalizer: ChunkNormalizer,
    verifier: Option<Verifier>,
    expanded: Vec<Event>,
    ready: VecDeque<Update<S>>,
    done: bool,
    dispatched: bool,
    abort: AbortHandle,
    end: Option<RunEnd>,
    /// What `RUN_FINISHED` reported, held until the stream ends: a
    /// `RUN_ERROR` may still follow it and fail the run.
    finished: Option<(RunOutcome, Option<Value>)>,
    diagnostics: Vec<RunDiagnostic>,
    diagnostics_omitted: usize,
    initial_ids: HashSet<MessageId>,
    response_ids: Option<(ThreadId, RunId)>,
    request_run_id: RunId,
    started_tool_calls: HashSet<ToolCallId>,
}
impl<T, S> std::fmt::Debug for RunStream<'_, T, S> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RunStream")
            .field("pending", &self.ready.len())
            .field("dispatched", &self.dispatched)
            .field("done", &self.done)
            .finish_non_exhaustive()
    }
}
impl<T, S> RunStream<'_, T, S> {
    /// Read-only materialized conversation while the run is borrowed.
    pub fn thread(&self) -> &Thread<T, S> {
        self.thread
    }
    /// A handle that cancels this run only.
    pub fn abort_handle(&self) -> AbortHandle {
        self.abort.clone()
    }
    /// Interrupt consumption locally.
    pub fn abort(&self) {
        self.abort.abort();
    }
    fn diagnostic(&mut self, error: Error) {
        if self.diagnostics.len() < self.thread.diagnostic_limit {
            let kind = match &error {
                Error::State(_) => "state",
                Error::Protocol(_) | Error::InvalidPatchDocument { .. } => "protocol",
                Error::Unsupported(_) => "unsupported",
                Error::Patch { .. } => "patch",
                Error::Run { .. } => "run",
                Error::Json(_) | Error::Decode(_) => "decode",
                _ => "transport",
            };
            self.diagnostics.push(RunDiagnostic {
                kind,
                message: error.to_string(),
            });
        } else {
            self.diagnostics_omitted += 1;
        }
        self.ready.push_back(Update::Error(error));
    }
    /// Kept like a diagnostic, within the same limit, but delivered as a
    /// warning: the run is unaffected.
    fn warning(&mut self, warning: String) {
        if self.diagnostics.len() < self.thread.diagnostic_limit {
            self.diagnostics.push(RunDiagnostic {
                kind: "warning",
                message: warning.clone(),
            });
        } else {
            self.diagnostics_omitted += 1;
        }
        self.ready.push_back(Update::Warning(warning));
    }
    fn terminate(&mut self, end: RunEnd) {
        if self.done {
            return;
        }
        self.done = true;
        self.events = None;
        self.prepared = None;
        self.finished = None;
        self.thread.applier.reset_streams();
        if self.dispatched {
            self.thread.last_run_end = Some(end.clone());
            self.thread.active_run_id = None;
        }
        self.end = Some(end.clone());
        self.ready.push_back(Update::Done(end));
    }
    /// Queues the update for a change to a reasoning message, with the text
    /// the message now holds.
    fn reasoning_changed(&mut self, change: ReasoningChange) {
        let text = self
            .thread
            .applier
            .text_of(change.id.clone())
            .unwrap_or_default()
            .to_owned();
        self.ready.push_back(Update::Reasoning(ReasoningUpdate {
            index: change.index,
            id: change.id,
            change: change.kind,
            text,
        }));
    }
    fn stop_subagents(&mut self) {
        let (messages, reasoning) = self.thread.applier.abort_messages();
        for change in messages {
            if let Some(message) = self.thread.applier.messages().get(change.index) {
                self.ready.push_back(Update::Message(MessageUpdate {
                    index: change.index,
                    id: change.id,
                    change: change.kind,
                    message: message.clone(),
                }));
            }
        }
        for change in reasoning {
            self.reasoning_changed(change);
        }
        for change in self.thread.applier.abort_subagents() {
            let subagent = self.thread.applier.subagents()[change.index].clone();
            self.ready.push_back(Update::Subagent(SubagentUpdate {
                index: change.index,
                run_id: change.run_id,
                change: change.kind,
                subagent,
            }));
        }
    }
    fn fail(&mut self, error: Error) {
        let message = error.to_string();
        self.diagnostic(error);
        self.thread.unconfirm();
        self.stop_subagents();
        self.terminate(RunEnd::Failed {
            message,
            code: None,
        });
    }
    /// Ends the run as its `RUN_FINISHED` reported, if one arrived and nothing
    /// has ended the run since.
    fn finish_reported(&mut self) -> bool {
        let Some((outcome, result)) = self.finished.take() else {
            return false;
        };
        if !matches!(outcome, RunOutcome::Cancelled) {
            self.thread
                .confirm_interrupt_tool_decisions(&self.started_tool_calls);
        }
        match outcome {
            RunOutcome::Success => {
                self.thread.submission = None;
                self.thread.interrupts.clear();
                self.terminate(RunEnd::Success { result });
            }
            RunOutcome::SuccessWithPendingToolCalls {
                pending_tool_call_ids,
            } => {
                self.thread.submission = None;
                self.thread.interrupts.clear();
                self.terminate(RunEnd::SuccessWithPendingToolCalls {
                    result,
                    pending_tool_call_ids,
                });
            }
            RunOutcome::Interrupt { interrupts } => {
                self.thread.submission = None;
                self.thread.interrupts.clone_from(&interrupts);
                for interrupt in &interrupts {
                    self.ready.push_back(Update::Interrupt(interrupt.clone()));
                }
                self.terminate(RunEnd::Interrupted { interrupts });
            }
            RunOutcome::Cancelled => {
                self.thread.unconfirm();
                self.stop_subagents();
                self.terminate(RunEnd::Cancelled);
            }
        }
        true
    }
}
impl<T, S> Drop for RunStream<'_, T, S> {
    fn drop(&mut self) {
        // A run that reported how it ended keeps that ending.
        self.finish_reported();
        if self.dispatched && !self.done {
            self.thread.unconfirm();
            self.thread.applier.abort_subagents();
            self.thread.last_run_end = Some(RunEnd::Aborted);
            self.thread.active_run_id = None;
        }
    }
}

impl<T: Transport, S: DeserializeOwned + Clone + Unpin> RunStream<'_, T, S> {
    /// Consume the remainder and return the entire run's outcome and diagnostics.
    pub async fn collect_report(mut self) -> RunReport {
        while self.next().await.is_some() {}
        RunReport {
            end: self
                .end
                .clone()
                .expect("a consumed stream has a terminal outcome"),
            new_messages: self
                .thread
                .messages()
                .iter()
                .filter(|message| !self.initial_ids.contains(message.id()))
                .cloned()
                .collect(),
            diagnostics: std::mem::take(&mut self.diagnostics),
            diagnostics_omitted: self.diagnostics_omitted,
        }
    }
}

impl<T, S: DeserializeOwned + Clone> RunStream<'_, T, S> {
    fn ingest(&mut self, event: Event) {
        if let Some(observer) = &mut self.thread.observer {
            observer(&event);
        }
        if matches!(event, Event::RunError(_)) {
            self.handle(event);
            return;
        }
        let mut expanded = std::mem::take(&mut self.expanded);
        expanded.clear();
        let result = self.normalizer.normalize(event, &mut expanded);
        for event in expanded.drain(..) {
            if !self.done {
                self.handle(event);
            }
        }
        self.expanded = expanded;
        if let Err(error) = result {
            self.fail(error);
        }
    }
    fn handle(&mut self, event: Event) {
        // Pair the terminal with the stream's announced run even when optional
        // ordering verification is disabled. Run errors carry no IDs.
        match &event {
            // 1.0 lets a later run follow a closed one on the same stream. This
            // stream reads one run: it ends as reported, and the rest is unread.
            Event::RunStarted(_) if self.finished.is_some() => {
                self.diagnostic(Error::Unsupported(
                    "RUN_STARTED after RUN_FINISHED: a later run on the same stream was not read"
                        .into(),
                ));
                self.finish_reported();
                return;
            }
            // A second RUN_STARTED while the run is active is a nested run
            // whatever ids it carries, and the verifier says so.
            Event::RunStarted(_) if self.response_ids.is_some() && self.verifier.is_some() => {}
            Event::RunStarted(started) => {
                if started.thread_id != self.thread.thread_id
                    || started.run_id != self.request_run_id
                {
                    self.fail(Error::protocol(
                        "RUN_STARTED does not match the requested thread and run IDs",
                    ));
                    return;
                }
                self.response_ids = Some((started.thread_id.clone(), started.run_id.clone()));
            }
            Event::RunFinished(finished) => {
                if self.response_ids.as_ref()
                    != Some(&(finished.thread_id.clone(), finished.run_id.clone()))
                {
                    self.fail(Error::protocol("RUN_FINISHED does not match RUN_STARTED"));
                    return;
                }
                if let Some(RunOutcome::Interrupt { interrupts }) = &finished.outcome {
                    if let Err(error) =
                        unique(interrupts.iter().map(|i| i.id.as_str()), "interrupt")
                    {
                        self.fail(Error::protocol(error.to_string()));
                        return;
                    }
                }
            }
            _ => {}
        }
        if let Some(verifier) = &mut self.verifier {
            if let Err(error) = verifier.verify(&event) {
                self.fail(error);
                return;
            }
        }
        let started_tool_call = match &event {
            Event::ToolCallStart(started) => Some(started.tool_call_id.clone()),
            _ => None,
        };
        match self.thread.applier.apply(&event) {
            Ok(changed) => {
                if let Some(id) = started_tool_call {
                    self.started_tool_calls.insert(id.clone());
                    self.thread.decided_interrupt_tool_call_ids.remove(&id);
                }
                self.emit(changed);
            }
            Err(error @ Error::Patch { .. }) => self.diagnostic(error),
            Err(error) => self.fail(error),
        }
    }
    fn emit(&mut self, changed: Changed) {
        match changed {
            Changed::Nothing | Changed::RunStarted { .. } => {}
            Changed::Message(change) => {
                if let Some(message) = self.thread.applier.messages().get(change.index) {
                    self.ready.push_back(Update::Message(MessageUpdate {
                        index: change.index,
                        id: change.id,
                        change: change.kind,
                        message: message.clone(),
                    }));
                }
            }
            Changed::MessagesReplaced => self
                .ready
                .push_back(Update::Messages(self.thread.messages().to_vec())),
            Changed::State => match self.thread.applier.state_as::<S>() {
                Ok(state) => {
                    self.thread.state = Ok(state.clone());
                    self.ready.push_back(Update::State(state));
                }
                Err(error) => {
                    self.thread.state = Err(StateViewError {
                        message: match &error {
                            Error::State(error) => error.to_string(),
                            _ => error.to_string(),
                        },
                    });
                    self.diagnostic(error);
                }
            },
            Changed::Reasoning(change) => self.reasoning_changed(change),
            Changed::Subagent(change) => {
                if let Some(subagent) = self.thread.applier.subagents().get(change.index) {
                    self.ready.push_back(Update::Subagent(SubagentUpdate {
                        index: change.index,
                        run_id: change.run_id,
                        change: change.kind,
                        subagent: subagent.clone(),
                    }));
                }
            }
            // The run is closed, but a RUN_ERROR may still follow and fail
            // it, so the outcome takes effect when the stream ends.
            Changed::RunFinished { outcome, result } => {
                self.finished = Some((outcome, result));
            }
            Changed::RunError { message, code } => {
                self.diagnostic(Error::Run {
                    message: message.clone(),
                    code: code.clone(),
                });
                self.thread.unconfirm();
                self.stop_subagents();
                self.terminate(RunEnd::Failed { message, code });
            }
        }
    }
}

impl<T: Transport, S: DeserializeOwned + Clone + Unpin> Stream for RunStream<'_, T, S> {
    type Item = Update<S>;
    fn poll_next(self: Pin<&mut Self>, cx: &mut TaskContext<'_>) -> Poll<Option<Self::Item>> {
        let this = self.get_mut();
        this.abort.state.waker.register(cx.waker());
        loop {
            // A terminal already applied wins over a later abort.
            if !this.done && this.abort.is_aborted() && !this.finish_reported() {
                if this.dispatched {
                    this.thread.unconfirm();
                    this.stop_subagents();
                }
                this.terminate(RunEnd::Aborted);
            }
            if let Some(update) = this.ready.pop_front() {
                return Poll::Ready(Some(update));
            }
            if this.done {
                return Poll::Ready(None);
            }
            if let Some((input, message)) = this.prepared.take() {
                // Expiry may have passed while the prepared stream sat unpolled.
                if let Some(entries) = &input.resume {
                    if let Err(error) = validate_responses(&this.thread.interrupts, entries, true) {
                        this.fail(error);
                        continue;
                    }
                }
                this.dispatched = true;
                this.thread.applier.reset_streams();
                this.thread.run_ids.insert(input.run_id.clone());
                this.thread.active_run_id = Some(input.run_id.clone());
                this.thread.next_run_id = None;
                if let Some(message) = message {
                    this.thread.applier.push_message(message);
                }
                if let Some(entries) = &input.resume {
                    this.thread.submission = Some(ResumeSubmission {
                        run_id: input.run_id.clone(),
                        entries: entries.clone(),
                        status: SubmissionStatus::InFlight,
                    });
                }
                this.events = Some(this.thread.agent.run_checked(input));
            }
            match this
                .events
                .as_mut()
                .expect("dispatched run has a stream")
                .as_mut()
                .poll_next(cx)
            {
                Poll::Pending => return Poll::Pending,
                Poll::Ready(Some(Ok(Checked::Event(event)))) => this.ingest(event),
                Poll::Ready(Some(Ok(Checked::Warning(warning)))) => this.warning(warning),
                // After RUN_FINISHED a broken connection truncates nothing:
                // the run already ended, so the break is only reported.
                Poll::Ready(Some(Err(error @ Error::Transport(_)))) if this.finished.is_some() => {
                    this.diagnostic(error);
                    this.finish_reported();
                }
                Poll::Ready(Some(Err(error))) => this.fail(error),
                // Ended cleanly after RUN_FINISHED: the run is over.
                Poll::Ready(None) if this.finish_reported() => {}
                Poll::Ready(None) => {
                    let error = this
                        .verifier
                        .as_ref()
                        .and_then(|verifier| verifier.finish().err())
                        .unwrap_or_else(|| {
                            Error::protocol("the stream ended before RUN_FINISHED or RUN_ERROR")
                        });
                    this.fail(error);
                }
            }
        }
    }
}
