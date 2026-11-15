//! One request of a conversation: the stream that answers it, the runs that
//! stream carries, and what it yields.

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
    enforce::{Checked, CheckedStream, REMEMBERED},
    error::{Error, Result},
    transport::Transport,
    verify::{Verifier, nested_run_started},
};
use crate::{
    Event, Interrupt, Message, MessageId, ReasoningEncryptedValueEvent,
    ReasoningEncryptedValueSubtype, ResumeEntry, RunAgentInput, RunId, RunOutcome, ToolCallId,
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
    ///
    /// Only for interrupts that became pending on the thread: those of the
    /// requested run, or of a run after it, which replace the requested
    /// run's. A replayed run's are history, and are in its [`RunSummary`]
    /// only. [`Thread::interrupts`] holds what is pending once the stream
    /// has ended.
    Interrupt(Interrupt),
    /// Something went wrong: a malformed stream, a patch that would not apply,
    /// a transport failure, a `RUN_ERROR`.
    ///
    /// Not necessarily fatal — a run survives a patch it could not apply, and
    /// a stream goes on after a `RUN_ERROR`, which fails one run and may be
    /// followed by the next. When it ends the stream, the matching
    /// [`Update::Done`] follows.
    Error(Error),
    /// Not an error: the stream was accepted, with a correction or an omission
    /// the application may want to log. An unrecognised field, enum member or
    /// event was dropped, a retired `THINKING_*` event was translated, a
    /// deprecated `null` was read as omitted, or the producer declared a newer
    /// protocol version. The run is unaffected. So is it in two more cases: a
    /// run after the requested one finished, and the pending interrupts were
    /// not replaced because a resume submission is still unconfirmed; or a
    /// `REASONING_ENCRYPTED_VALUE` was dropped because the thread holds no
    /// message or tool call by its `entityId` that can store it.
    ///
    /// Each text is delivered once per run and repeats at most 64 characters
    /// of input, with control characters escaped. [`RunReport::diagnostics`]
    /// keeps it with kind `warning`.
    Warning(String),
    /// One run on the stream ended, and how.
    ///
    /// A stream carries the run the request asked for, and AG-UI 1.0 lets
    /// runs the producer replays precede it. Each run gets one of these, the
    /// only run of a stream included, once nothing can change how it ended:
    /// at its `RUN_ERROR`, or for a run that reported `RUN_FINISHED` at
    /// whatever follows it — the next `RUN_STARTED`, a late `RUN_ERROR`, the
    /// end of the stream. [`Update::Done`] follows the last one.
    RunEnded(RunSummary),
    /// The stream ended, and how the request went: the [`RunEnd`] of the
    /// requested run — of the last run on the stream, should the producer
    /// have gone on after it — or how reading stopped before any such run
    /// ended. Always the last update, on every path out: the agent
    /// finishing, the agent failing, and the transport dying mid-sentence.
    ///
    /// It arrives when the stream ends, not at a terminal event: another run
    /// may follow `RUN_FINISHED` or `RUN_ERROR`.
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

/// One run a stream carried, and how it ended.
///
/// The runs of a stream fall in three places. Those before the requested run
/// are *replayed*: history the producer restates. They are read into the
/// conversation and reported here, and change nothing the thread owes. The
/// *requested* run answers the request: it alone confirms a resume
/// submission. A run *after* it is one AG-UI 1.0 does not provide for — the
/// requested run is the last of its exchange — but a producer that keeps the
/// stream open sends one, and the agent then stands where that run left it.
/// So what the thread owes next, its pending interrupts and the tool calls a
/// success outcome names, is what the last run from the requested one on
/// reported.
#[derive(Clone, Debug, PartialEq)]
#[non_exhaustive]
pub struct RunSummary {
    /// The id its `RUN_STARTED` carried. `None` for a failure that opened
    /// the stream: a `RUN_ERROR` with no run before it.
    pub run_id: Option<RunId>,
    /// How it ended, or how reading it stopped.
    pub end: RunEnd,
    /// Whether this is the run the request asked for: the one whose
    /// `RUN_STARTED` carried the request's run id, or a failure that opened
    /// the stream, which is that run failing before it began.
    pub requested: bool,
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

/// Results from the whole stream, including updates consumed before `collect_report`.
#[derive(Clone, Debug, PartialEq)]
#[non_exhaustive]
pub struct RunReport {
    /// How the request went: what [`Update::Done`] carried.
    pub end: RunEnd,
    /// Every run the stream carried, in order. One entry for a stream that
    /// carried the requested run alone; none when reading stopped before any
    /// run began.
    pub runs: Vec<RunSummary>,
    /// Messages whose IDs were absent when this request was prepared,
    /// reasoning messages among them, whichever run on the stream produced
    /// them.
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
            run: None,
            runs: Vec::new(),
            opened: false,
            live: false,
            outcome: None,
            diagnostics: Vec::new(),
            diagnostics_omitted: 0,
            initial_ids,
        })
    }
}

/// Where a run stands relative to the one the request asked for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Place {
    /// Before it: history the producer restates.
    Replayed,
    /// The one asked for.
    Requested,
    /// After it.
    Later,
}

/// The run being read: from its `RUN_STARTED` until its end is settled.
struct OpenRun {
    run_id: RunId,
    place: Place,
    /// What `RUN_FINISHED` reported, held until the stream says what follows
    /// it: a `RUN_ERROR` may still fail the run.
    finished: Option<(RunOutcome, Option<Value>)>,
    /// The tool calls this run started.
    started_tool_calls: HashSet<ToolCallId>,
    /// The warnings this run gave about encrypted values it dropped, up to
    /// [`REMEMBERED`] of them.
    dropped: HashSet<String>,
}

#[derive(Default)]
struct AbortState {
    aborted: AtomicBool,
    waker: AtomicWaker,
}
/// Cloneable cancellation for one local request. Wakes a pending poll and drops its
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

/// A lazily dispatched request, read to the end of the stream that answers
/// it. It borrows the thread until dropped. Every consumed stream yields one
/// [`Update::RunEnded`] per run it carried and then one [`Update::Done`];
/// dropping an unpolled one leaves the thread unchanged.
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
    /// The run being read.
    run: Option<OpenRun>,
    /// Every run whose end is settled, in stream order.
    runs: Vec<RunSummary>,
    /// Whether a `RUN_STARTED` has arrived. A `RUN_ERROR` before any is the
    /// requested run failing before it began.
    opened: bool,
    /// Whether the requested run has started. A run from then on is live:
    /// its end says where the conversation stands.
    live: bool,
    /// How the last live run ended.
    outcome: Option<RunEnd>,
    diagnostics: Vec<RunDiagnostic>,
    diagnostics_omitted: usize,
    initial_ids: HashSet<MessageId>,
    request_run_id: RunId,
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
    /// A handle that cancels this request only.
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
    /// Ends the stream. The end of the last live run is how the request
    /// went; `stopped` is how reading stopped, for a stream on which no live
    /// run ended.
    fn terminate(&mut self, stopped: RunEnd) {
        if self.done {
            return;
        }
        debug_assert!(self.run.is_none(), "the run being read is settled first");
        self.done = true;
        self.events = None;
        self.prepared = None;
        self.run = None;
        self.thread.applier.reset_streams();
        // The requested run settles a resume submission one way or the
        // other. On a stream without it, nothing did.
        if self.dispatched && self.outcome.is_none() {
            self.thread.unconfirm();
        }
        let end = self.outcome.take().unwrap_or(stopped);
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
    /// Reports what a run that stopped short left open as aborted: its
    /// messages, its reasoning, its running subagents.
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
    /// Stops reading because of `error`. The run being read fails with it,
    /// and so does the request, unless a live run already said how it went.
    fn fail(&mut self, error: Error) {
        let end = RunEnd::Failed {
            message: error.to_string(),
            code: None,
        };
        self.diagnostic(error);
        if !self.settle_stopped(end.clone()) {
            self.stop_subagents();
        }
        self.terminate(end);
    }
    /// Stops reading at the application's request: an abort, or a drop.
    fn stop(&mut self) {
        // A run that reported how it ended keeps that ending.
        if !self.settle_reported() && !self.settle_stopped(RunEnd::Aborted) && self.dispatched {
            self.stop_subagents();
        }
        self.terminate(RunEnd::Aborted);
    }
    /// The stream ended.
    fn end_of_input(&mut self) {
        // A run that reported how it ended is over. One still active was cut
        // short, and so was a stream no run arrived on.
        if !self.settle_reported() && (self.run.is_some() || self.runs.is_empty()) {
            let error = self
                .verifier
                .as_ref()
                .and_then(|verifier| verifier.finish().err())
                .unwrap_or_else(|| {
                    Error::protocol("the stream ended before RUN_FINISHED or RUN_ERROR")
                });
            return self.fail(error);
        }
        match self.outcome.clone() {
            Some(end) => self.terminate(end),
            // Every run on the stream was a replayed one.
            None => self.fail(Error::protocol(format!(
                "the stream ended without the requested run: no RUN_STARTED carried run ID {:?}",
                self.request_run_id.as_str()
            ))),
        }
    }
    /// Whether the run being read reported `RUN_FINISHED`, and waits for the
    /// stream to say what follows.
    fn reported(&self) -> bool {
        self.run.as_ref().is_some_and(|run| run.finished.is_some())
    }
    /// Ends the run being read as its `RUN_FINISHED` reported, if one
    /// arrived and nothing has ended the run since.
    fn settle_reported(&mut self) -> bool {
        if !self.reported() {
            return false;
        }
        let run = self.run.take().expect("a run is being read");
        let (outcome, result) = run.finished.expect("the run reported RUN_FINISHED");
        let end = match outcome {
            RunOutcome::Success => RunEnd::Success { result },
            RunOutcome::SuccessWithPendingToolCalls {
                pending_tool_call_ids,
            } => RunEnd::SuccessWithPendingToolCalls {
                result,
                pending_tool_call_ids,
            },
            RunOutcome::Interrupt { interrupts } => RunEnd::Interrupted { interrupts },
            RunOutcome::Cancelled => RunEnd::Cancelled,
        };
        self.settle(Some(run.run_id), run.place, &run.started_tool_calls, end);
        true
    }
    /// Ends the run being read, if there is one, short of an ending of its
    /// own: reading stopped, or the stream was rejected. A `RUN_FINISHED` it
    /// had reported is withdrawn.
    fn settle_stopped(&mut self, end: RunEnd) -> bool {
        let Some(run) = self.run.take() else {
            return false;
        };
        self.settle(Some(run.run_id), run.place, &run.started_tool_calls, end);
        true
    }
    /// Records how one run ended, and applies what that means for the thread.
    fn settle(
        &mut self,
        run_id: Option<RunId>,
        place: Place,
        started_tool_calls: &HashSet<ToolCallId>,
        end: RunEnd,
    ) {
        let confirms = matches!(
            end,
            RunEnd::Success { .. }
                | RunEnd::SuccessWithPendingToolCalls { .. }
                | RunEnd::Interrupted { .. }
        );
        // Only the requested run answers the request, so only it settles a
        // resume submission.
        if place == Place::Requested {
            if confirms {
                self.thread
                    .confirm_interrupt_tool_decisions(started_tool_calls);
                self.thread.submission = None;
            } else {
                self.thread.unconfirm();
            }
        }
        if confirms {
            // RUN_FINISHED found everything closed, unless nothing checked.
            self.thread.applier.reset_streams();
        } else {
            self.stop_subagents();
        }
        if place != Place::Replayed {
            self.replace_pending(run_id.as_ref(), place, &end);
            self.outcome = Some(end.clone());
        }
        let summary = RunSummary {
            run_id,
            end,
            requested: place == Place::Requested,
        };
        self.runs.push(summary.clone());
        self.ready.push_back(Update::RunEnded(summary));
    }
    /// What a live run's end leaves pending on the thread: an interrupt
    /// outcome sets the pending interrupts, a success or cancelled one
    /// clears them, and a run that failed or was cut short leaves them.
    ///
    /// Not while a resume submission is held. Its entries answer exactly the
    /// pending interrupts, and it stays until the application reconciles it,
    /// so they stay with it.
    fn replace_pending(&mut self, run_id: Option<&RunId>, place: Place, end: &RunEnd) {
        let (pending, outcome): (&[Interrupt], _) = match end {
            RunEnd::Interrupted { interrupts } => (interrupts, "interrupt"),
            RunEnd::Success { .. } | RunEnd::SuccessWithPendingToolCalls { .. } => (&[], "success"),
            RunEnd::Cancelled => (&[], "cancelled"),
            RunEnd::Failed { .. } | RunEnd::Aborted => return,
        };
        if self.thread.submission.is_none() {
            self.thread.interrupts = pending.to_vec();
            for interrupt in pending {
                self.ready.push_back(Update::Interrupt(interrupt.clone()));
            }
        } else if place == Place::Later && self.thread.interrupts != pending {
            // That the requested run left the submission unconfirmed is its
            // own ending. This run came after it, and would have moved the
            // thread on.
            let run = run_id.map_or_else(String::new, |id| {
                format!(" \"{}\"", crate::protocol::excerpt(id.as_str(), false))
            });
            self.warning(format!(
                "run{run} finished with outcome `{outcome}` while a resume submission is \
                 unconfirmed: the pending interrupts stay the ones that submission answers \
                 until the application reconciles it"
            ));
        }
    }
}
impl<T, S> Drop for RunStream<'_, T, S> {
    fn drop(&mut self) {
        if self.dispatched && !self.done {
            self.stop();
        }
    }
}

impl<T: Transport, S: DeserializeOwned + Clone + Unpin> RunStream<'_, T, S> {
    /// Consume the remainder and return the outcome, the runs and the
    /// diagnostics of the whole stream.
    pub async fn collect_report(mut self) -> RunReport {
        while self.next().await.is_some() {}
        RunReport {
            end: self
                .end
                .clone()
                .expect("a consumed stream has a terminal outcome"),
            runs: std::mem::take(&mut self.runs),
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
            // A failed run ends what it had open along with it. Nothing is
            // closed on its behalf, and nothing of it is left for the next
            // run to close.
            self.normalizer = ChunkNormalizer::new();
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
        // Pair each run's boundary events with each other and with the
        // request, even when optional ordering verification is disabled. Run
        // errors carry no IDs.
        let mut opening = None;
        match &event {
            Event::RunStarted(started) => {
                match self.run.as_ref().filter(|run| run.finished.is_none()) {
                    // Inside a run that has not ended it is a nested run
                    // whatever ids it carries, and the verifier says so.
                    // Without one, the same run announced again is let
                    // through, as it always was.
                    Some(active) => {
                        let again = started.thread_id == self.thread.thread_id
                            && started.run_id == active.run_id;
                        if self.verifier.is_none() && !again {
                            self.fail(nested_run_started());
                            return;
                        }
                    }
                    None => {
                        // The run before it, if it reported how it ended, is
                        // over: nothing can fail it now.
                        self.settle_reported();
                        if started.thread_id != self.thread.thread_id {
                            self.fail(Error::protocol(
                                "RUN_STARTED does not match the requested thread ID",
                            ));
                            return;
                        }
                        opening = Some(started.run_id.clone());
                    }
                }
            }
            Event::RunFinished(finished) => {
                let closes = finished.thread_id == self.thread.thread_id
                    && self
                        .run
                        .as_ref()
                        .is_some_and(|run| run.run_id == finished.run_id);
                if !closes {
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
        if let Some(run_id) = opening {
            self.open(run_id);
        }
        let started_tool_call = match &event {
            Event::ToolCallStart(started) => Some(started.tool_call_id.clone()),
            _ => None,
        };
        match self.thread.applier.apply(&event) {
            Ok(changed) => {
                if let (Event::ReasoningEncryptedValue(value), Changed::Nothing) =
                    (&event, &changed)
                {
                    self.dropped(value);
                }
                if let Some(id) = started_tool_call {
                    self.thread.decided_interrupt_tool_call_ids.remove(&id);
                    if let Some(run) = &mut self.run {
                        run.started_tool_calls.insert(id);
                    }
                }
                self.emit(changed);
            }
            Err(error @ Error::Patch { .. }) => self.diagnostic(error),
            Err(error) => self.fail(error),
        }
    }
    /// Begins reading the run a `RUN_STARTED` opened.
    ///
    /// The run that echoes the request's id is the requested one. One before
    /// it is replayed, one after it is not, and neither is held to the
    /// request's id: AG-UI 1.0, `basic/run-input`, "the requested run echoes
    /// this input's `runId`; a replayed run carries its own".
    fn open(&mut self, run_id: RunId) {
        let place = if run_id == self.request_run_id {
            Place::Requested
        } else if self.live {
            Place::Later
        } else {
            Place::Replayed
        };
        self.opened = true;
        self.live |= place == Place::Requested;
        // A new run starts with nothing open.
        self.thread.applier.reset_streams();
        self.run = Some(OpenRun {
            run_id,
            place,
            finished: None,
            started_tool_calls: HashSet::new(),
            dropped: HashSet::new(),
        });
    }
    /// Warns that an encrypted value was dropped, once per text per run:
    /// AG-UI 1.0 (`events/reasoning`, "Error Handling") says a consumer
    /// SHOULD warn about one whose entity it does not hold, and MAY drop it.
    /// The applier stores it nowhere when no message or tool call it holds
    /// can take it, an activity message included.
    fn dropped(&mut self, event: &ReasoningEncryptedValueEvent) {
        let noun = match event.subtype {
            ReasoningEncryptedValueSubtype::Message => "message",
            ReasoningEncryptedValueSubtype::ToolCall => "tool call",
        };
        let warning = format!(
            "REASONING_ENCRYPTED_VALUE for {noun} \"{}\" was dropped: the thread holds no \
             {noun} with that id that can store it",
            crate::protocol::excerpt(&event.entity_id, false)
        );
        if let Some(run) = &mut self.run {
            if run.dropped.contains(&warning) {
                return;
            }
            if run.dropped.len() < REMEMBERED {
                run.dropped.insert(warning.clone());
            }
        }
        self.warning(warning);
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
            // it, so the outcome takes effect when the stream says what
            // comes next.
            Changed::RunFinished { outcome, result } => {
                if let Some(run) = &mut self.run {
                    run.finished = Some((outcome, result));
                }
            }
            // The run is over, and the stream is not: the next run may
            // follow.
            Changed::RunError { message, code } => {
                self.diagnostic(Error::Run {
                    message: message.clone(),
                    code: code.clone(),
                });
                let end = RunEnd::Failed { message, code };
                if !self.settle_stopped(end.clone()) {
                    // No run to fail. Before any RUN_STARTED, it is the
                    // requested run that failed before it began.
                    let place = match (self.opened, self.live) {
                        (false, _) => Place::Requested,
                        (true, true) => Place::Later,
                        (true, false) => Place::Replayed,
                    };
                    self.live |= place == Place::Requested;
                    self.settle(None, place, &HashSet::new(), end);
                }
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
            if !this.done && this.abort.is_aborted() {
                this.stop();
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
                // the run already ended, so the break is only reported. It
                // still fails a request whose own run was yet to come.
                Poll::Ready(Some(Err(error @ Error::Transport(_)))) if this.reported() => {
                    let stopped = RunEnd::Failed {
                        message: error.to_string(),
                        code: None,
                    };
                    this.diagnostic(error);
                    this.settle_reported();
                    this.terminate(stopped);
                }
                Poll::Ready(Some(Err(error))) => this.fail(error),
                Poll::Ready(None) => this.end_of_input(),
            }
        }
    }
}
