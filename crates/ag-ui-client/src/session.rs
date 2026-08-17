//! The high-level API: a conversation you send text to.
//!
//! [`Agent`] gives you events. A UI does not want events — it
//! wants "this message grew by three characters", "the state changed, here it
//! is typed", "the agent is waiting for you to approve something". A [`Session`]
//! is the thread, its accumulated messages and its typed state, and it yields
//! [`Update`]s instead of raw events.
//!
//! Everything the protocol makes fiddly happens inside: chunk events are
//! normalized, the stream is verified, deltas are folded into messages, and the
//! next run automatically carries the conversation so far.
//!
//! ```
//! use ag_ui_client::{Session, Update, transport::ReplayTransport};
//! use ag_ui_core::{Event, TextMessageRole};
//! use futures_util::StreamExt;
//!
//! # let rt = tokio::runtime::Builder::new_current_thread().build().unwrap();
//! # rt.block_on(async {
//! let transport = ReplayTransport::new([
//!     Event::run_started("thread-1", "run-1"),
//!     Event::text_message_start("msg-1", TextMessageRole::Assistant),
//!     Event::text_message_content("msg-1", "Sunny."),
//!     Event::text_message_end("msg-1"),
//!     Event::run_finished_success("thread-1", "run-1"),
//! ]);
//!
//! let mut session = Session::<_>::new(transport, "thread-1");
//! let mut run = session.send("what is the weather?");
//! while let Some(update) = run.next().await {
//!     if let Update::Message(message) = update {
//!         println!("{}: {:?}", message.id, message.change);
//!     }
//! }
//! drop(run);
//!
//! // The user's turn and the agent's reply are both in the thread now, so the
//! // next `send` carries them.
//! assert_eq!(session.messages().len(), 2);
//! # });
//! ```

use std::collections::VecDeque;
use std::pin::Pin;
use std::task::{Context as TaskContext, Poll};

use ag_ui_core::{
    Context, Event, Interrupt, Message, MessageId, ReasoningMessage, ResumeEntry, RunAgentInput,
    RunId, RunOutcome, ThreadId, Tool,
};
use futures_core::Stream;
use serde::de::DeserializeOwned;
use serde_json::Value;

use crate::agent::Agent;
use crate::apply::{Applier, Changed, MessageChangeKind, ReasoningChangeKind};
use crate::chunks::ChunkNormalizer;
use crate::error::Error;
use crate::interrupts::InterruptExt;
use crate::transport::{EventStream, Transport};
use crate::verify::Verifier;

/// Something a view should react to.
///
/// One [`Update`] is one redraw. `S` is the caller's state type; it is
/// [`serde_json::Value`] unless a [`Session`] is asked for something better.
#[derive(Debug)]
#[non_exhaustive]
pub enum Update<S = Value> {
    /// A message was created, appended to, or completed.
    Message(MessageUpdate),
    /// `MESSAGES_SNAPSHOT` replaced the conversation. Messages may have
    /// disappeared, so redraw all of it.
    Messages(Vec<Message>),
    /// The application state changed, and here it is in the caller's type.
    State(S),
    /// Reasoning text arrived. Kept separate from the reply.
    Reasoning(ReasoningUpdate),
    /// The run paused and needs a human. Answer it with
    /// [`Session::resume`] — one update per pending interrupt.
    Interrupt(Interrupt),
    /// Something went wrong: a malformed stream, a patch that would not apply,
    /// a transport failure, a `RUN_ERROR`.
    ///
    /// Not necessarily fatal. A run that ends also yields [`Update::Done`].
    Error(Error),
    /// The run ended. Always the last update of a run.
    Done(RunEnd),
}

/// A message that changed, and the message as it now stands.
#[derive(Clone, Debug, PartialEq)]
pub struct MessageUpdate {
    /// Index into [`Session::messages`].
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
pub struct ReasoningUpdate {
    /// The reasoning message's id.
    pub id: MessageId,
    /// What this event did to it.
    pub change: ReasoningChangeKind,
    /// The accumulated reasoning text.
    pub text: String,
}

/// How a run ended.
#[derive(Clone, Debug, PartialEq)]
#[non_exhaustive]
pub enum RunEnd {
    /// The agent finished.
    Success {
        /// The agent's return value, if it sent one.
        result: Option<Value>,
    },
    /// The agent paused for human input. The same interrupts arrived
    /// individually as [`Update::Interrupt`], and are on
    /// [`Session::interrupts`] until the next run.
    Interrupted {
        /// What the agent is waiting for.
        interrupts: Vec<Interrupt>,
    },
    /// The run failed. The matching [`Update::Error`] came first.
    Failed {
        /// What went wrong, for a human.
        message: String,
        /// The machine-readable code, when the agent sent one.
        code: Option<String>,
    },
}

/// A conversation with an agent.
///
/// Holds the thread id, the messages both sides have said, and the application
/// state. `S` is the type the state deserializes into; it defaults to
/// [`serde_json::Value`], so `Session::<_>::new(transport, thread)` is the
/// untyped spelling and `Session::<_, MyState>::new(…)` the typed one.
///
/// To stream updates, `S` must be `Deserialize + Clone + Unpin` — an
/// [`Update::State`] carries the state by value, so a view can hold it after
/// the run has moved on. `#[derive(Clone, Deserialize)]` on a plain struct is
/// all that takes.
#[derive(Debug)]
pub struct Session<T, S = Value> {
    agent: Agent<T>,
    thread_id: ThreadId,
    applier: Applier,
    state: Option<S>,
    tools: Vec<Tool>,
    context: Vec<Context>,
    forwarded_props: Value,
    verify: bool,
    interrupts: Vec<Interrupt>,
    runs: u64,
    messages_sent: u64,
    next_run_id: Option<RunId>,
}

impl<T, S> Session<T, S> {
    /// A new conversation over `transport`.
    pub fn new(transport: T, thread_id: impl Into<ThreadId>) -> Self {
        Self::builder(transport, thread_id).build()
    }

    /// A builder, for seeding history, tools, context, or turning verification
    /// off.
    pub fn builder(transport: T, thread_id: impl Into<ThreadId>) -> SessionBuilder<T, S> {
        SessionBuilder::new(transport, thread_id)
    }

    /// The conversation this session is part of.
    pub fn thread_id(&self) -> &ThreadId {
        &self.thread_id
    }

    /// The assembled conversation, oldest first — everything the user sent and
    /// everything the agent has said across every run.
    pub fn messages(&self) -> &[Message] {
        self.applier.messages()
    }

    /// The application state in the caller's type, once the agent has published
    /// one that deserializes.
    pub fn state(&self) -> Option<&S> {
        self.state.as_ref()
    }

    /// The application state as raw JSON. Always current, even when the typed
    /// view is not.
    pub fn raw_state(&self) -> &Value {
        self.applier.state()
    }

    /// The reasoning messages, kept out of the transcript.
    pub fn reasoning(&self) -> &[ReasoningMessage] {
        self.applier.reasoning()
    }

    /// What the agent is waiting for, if the last run paused.
    pub fn interrupts(&self) -> &[Interrupt] {
        &self.interrupts
    }

    /// The applier underneath, for a view that wants the raw materialised
    /// state.
    pub fn applier(&self) -> &Applier {
        &self.applier
    }

    /// The low-level agent underneath.
    pub fn agent(&self) -> &Agent<T> {
        &self.agent
    }

    /// Appends a message without starting a run — a tool result computed on the
    /// client, or history loaded from a store.
    pub fn push_message(&mut self, message: Message) {
        self.applier.push_message(message);
    }

    /// Replaces the state without going through the agent.
    pub fn set_state(&mut self, state: impl Into<Value>) {
        self.applier.set_state(state);
    }

    /// Offers a different set of tools from the next run on.
    pub fn set_tools(&mut self, tools: impl Into<Vec<Tool>>) {
        self.tools = tools.into();
    }

    /// Names the next run explicitly, instead of the generated
    /// `{thread}-run-{n}`.
    ///
    /// Servers that key resumption on a run id need this; most do not.
    pub fn set_next_run_id(&mut self, run_id: impl Into<RunId>) {
        self.next_run_id = Some(run_id.into());
    }

    /// Builds the next request: the conversation so far, the state so far, and
    /// a freshly minted run id.
    fn input(&mut self, resume: Option<Vec<ResumeEntry>>) -> RunAgentInput {
        RunAgentInput {
            thread_id: self.thread_id.clone(),
            run_id: self.next_run_id(),
            parent_run_id: None,
            state: self.applier.state().clone(),
            messages: self.applier.messages().to_vec(),
            tools: self.tools.clone(),
            context: self.context.clone(),
            forwarded_props: self.forwarded_props.clone(),
            resume,
        }
    }

    fn next_run_id(&mut self) -> RunId {
        if let Some(run_id) = self.next_run_id.take() {
            return run_id;
        }
        self.runs += 1;
        RunId::new(format!("{}-run-{}", self.thread_id, self.runs))
    }

    fn next_message_id(&mut self) -> MessageId {
        self.messages_sent += 1;
        MessageId::new(format!("{}-msg-{}", self.thread_id, self.messages_sent))
    }
}

impl<T: Transport, S> Session<T, S> {
    /// Sends the user's turn and streams what the agent does about it.
    ///
    /// The message is appended to the conversation before the request goes out,
    /// so it is in [`Session::messages`] whatever happens to the run.
    pub fn send(&mut self, text: impl Into<String>) -> RunStream<'_, T, S> {
        let id = self.next_message_id();
        self.push_message(Message::user(id, text.into()));
        self.start(None)
    }

    /// Sends a message of any role and streams the run.
    pub fn send_message(&mut self, message: Message) -> RunStream<'_, T, S> {
        self.push_message(message);
        self.start(None)
    }

    /// Starts a run without adding anything — after pushing a tool result, or
    /// to let an agent continue on its own.
    pub fn run(&mut self) -> RunStream<'_, T, S> {
        self.start(None)
    }

    /// Answers one interrupt and resumes the paused run.
    ///
    /// The answer's shape is up to the agent; when the interrupt carried a
    /// `responseSchema`, `payload` should satisfy it.
    pub fn resume(
        &mut self,
        interrupt: &Interrupt,
        payload: impl Into<Value>,
    ) -> RunStream<'_, T, S> {
        self.resume_many([interrupt.resolve(payload)])
    }

    /// Declines one interrupt and resumes the paused run.
    pub fn cancel(&mut self, interrupt: &Interrupt) -> RunStream<'_, T, S> {
        self.resume_many([interrupt.cancel()])
    }

    /// Answers several interrupts at once — a run can pause on more than one.
    ///
    /// Any interrupt left unanswered is dropped: the resumed run supersedes the
    /// paused one, and the agent only sees what is in this request. Use
    /// [`ResumeBuilder`](crate::interrupts::ResumeBuilder) to answer them all.
    pub fn resume_many(
        &mut self,
        entries: impl IntoIterator<Item = ResumeEntry>,
    ) -> RunStream<'_, T, S> {
        self.start(Some(entries.into_iter().collect()))
    }

    fn start(&mut self, resume: Option<Vec<ResumeEntry>>) -> RunStream<'_, T, S> {
        self.interrupts.clear();
        let input = self.input(resume);
        // The stream is `'static`, so this borrow of the agent ends here and
        // the session is free to be borrowed mutably for the run.
        let events = self.agent.run(input);
        let verifier = self.verify.then(Verifier::new);
        RunStream {
            session: self,
            events,
            normalizer: ChunkNormalizer::new(),
            verifier,
            expanded: Vec::new(),
            ready: VecDeque::new(),
            done: false,
        }
    }
}

/// Builds a [`Session`].
#[derive(Debug)]
pub struct SessionBuilder<T, S = Value> {
    transport: T,
    thread_id: ThreadId,
    messages: Vec<Message>,
    state: Value,
    tools: Vec<Tool>,
    context: Vec<Context>,
    forwarded_props: Value,
    verify: bool,
    marker: std::marker::PhantomData<fn() -> S>,
}

impl<T, S> SessionBuilder<T, S> {
    /// A builder for a conversation over `transport`.
    pub fn new(transport: T, thread_id: impl Into<ThreadId>) -> Self {
        Self {
            transport,
            thread_id: thread_id.into(),
            messages: Vec::new(),
            state: Value::Object(serde_json::Map::new()),
            tools: Vec::new(),
            context: Vec::new(),
            forwarded_props: Value::Null,
            verify: true,
            marker: std::marker::PhantomData,
        }
    }

    /// Seeds the conversation with existing history.
    #[must_use]
    pub fn messages(mut self, messages: impl Into<Vec<Message>>) -> Self {
        self.messages = messages.into();
        self
    }

    /// Seeds the application state.
    #[must_use]
    pub fn state(mut self, state: impl Into<Value>) -> Self {
        self.state = state.into();
        self
    }

    /// Offers tools on every run.
    #[must_use]
    pub fn tools(mut self, tools: impl Into<Vec<Tool>>) -> Self {
        self.tools = tools.into();
        self
    }

    /// Sets the ambient context entries sent on every run.
    #[must_use]
    pub fn context(mut self, context: impl Into<Vec<Context>>) -> Self {
        self.context = context.into();
        self
    }

    /// Sets the passthrough properties sent on every run.
    #[must_use]
    pub fn forwarded_props(mut self, props: impl Into<Value>) -> Self {
        self.forwarded_props = props.into();
        self
    }

    /// Turns [protocol verification](crate::verify) on or off. On by default.
    ///
    /// Off is for producers whose quirks you have decided to live with; the
    /// applier stays tolerant either way, so what you lose is the diagnosis,
    /// not the conversation.
    #[must_use]
    pub fn verify(mut self, verify: bool) -> Self {
        self.verify = verify;
        self
    }

    /// Builds the session.
    pub fn build(self) -> Session<T, S> {
        Session {
            agent: Agent::new(self.transport),
            thread_id: self.thread_id,
            applier: Applier::new()
                .with_messages(self.messages)
                .with_state(self.state),
            state: None,
            tools: self.tools,
            context: self.context,
            forwarded_props: self.forwarded_props,
            verify: self.verify,
            interrupts: Vec::new(),
            runs: 0,
            messages_sent: 0,
            next_run_id: None,
        }
    }
}

/// One run, as a stream of [`Update`]s.
///
/// Borrows the session mutably: the conversation and the state are being
/// updated as the stream is polled, which is what makes
/// [`Session::messages`] correct the moment the run ends.
///
/// The stream ends after [`Update::Done`]. It also ends if the transport stops
/// early, in which case verification (when on) reports the truncation as an
/// [`Update::Error`] first.
pub struct RunStream<'a, T, S = Value> {
    session: &'a mut Session<T, S>,
    events: EventStream,
    normalizer: ChunkNormalizer,
    verifier: Option<Verifier>,
    expanded: Vec<Event>,
    ready: VecDeque<Update<S>>,
    done: bool,
}

impl<T, S> std::fmt::Debug for RunStream<'_, T, S> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RunStream")
            .field("pending", &self.ready.len())
            .field("done", &self.done)
            .finish_non_exhaustive()
    }
}

impl<T, S> RunStream<'_, T, S>
where
    S: DeserializeOwned + Clone,
{
    /// Runs one event through normalize → verify → apply.
    fn ingest(&mut self, event: Event) {
        // Reuse the buffer across events; it is the same shape every time.
        let mut expanded = std::mem::take(&mut self.expanded);
        expanded.clear();
        let outcome = self.normalizer.normalize(event, &mut expanded);
        for event in expanded.drain(..) {
            self.handle(event);
        }
        self.expanded = expanded;
        if let Err(error) = outcome {
            self.ready.push_back(Update::Error(error));
        }
    }

    fn handle(&mut self, event: Event) {
        if let Some(verifier) = &mut self.verifier {
            if let Err(error) = verifier.verify(&event) {
                self.ready.push_back(Update::Error(error));
                // Do not apply an event the producer should not have sent: a
                // clear error beats state assembled from a broken stream. The
                // exception is an event that ends the run — the run is over
                // either way, and a caller that never hears so waits forever.
                if !matches!(event, Event::RunFinished(_) | Event::RunError(_)) {
                    return;
                }
            }
        }
        match self.session.applier.apply(&event) {
            Ok(changed) => self.emit(changed),
            Err(error) => self.ready.push_back(Update::Error(error)),
        }
    }

    fn emit(&mut self, changed: Changed) {
        match changed {
            Changed::Nothing | Changed::RunStarted { .. } => {}

            Changed::Message(change) => {
                if let Some(message) = self.session.applier.messages().get(change.index) {
                    let update = MessageUpdate {
                        index: change.index,
                        id: change.id,
                        change: change.kind,
                        message: message.clone(),
                    };
                    self.ready.push_back(Update::Message(update));
                }
            }

            Changed::MessagesReplaced => {
                let messages = self.session.applier.messages().to_vec();
                self.ready.push_back(Update::Messages(messages));
            }

            Changed::State => match self.session.applier.state_as::<S>() {
                Ok(state) => {
                    self.session.state = Some(state.clone());
                    self.ready.push_back(Update::State(state));
                }
                // The raw state is still updated and correct; only the typed
                // view is out of date, and that is worth saying out loud.
                Err(error) => self.ready.push_back(Update::Error(error)),
            },

            Changed::Reasoning(change) => {
                let text = self
                    .session
                    .applier
                    .reasoning_text(&change.id)
                    .unwrap_or_default()
                    .to_owned();
                let update = ReasoningUpdate {
                    id: change.id,
                    change: change.kind,
                    text,
                };
                self.ready.push_back(Update::Reasoning(update));
            }

            Changed::RunFinished { outcome, result } => {
                self.done = true;
                match outcome {
                    RunOutcome::Success => {
                        self.ready
                            .push_back(Update::Done(RunEnd::Success { result }));
                    }
                    RunOutcome::Interrupt { interrupts } => {
                        self.session.interrupts.clone_from(&interrupts);
                        for interrupt in &interrupts {
                            self.ready.push_back(Update::Interrupt(interrupt.clone()));
                        }
                        self.ready
                            .push_back(Update::Done(RunEnd::Interrupted { interrupts }));
                    }
                }
            }

            Changed::RunError { message, code } => {
                self.done = true;
                self.ready.push_back(Update::Error(Error::Run {
                    message: message.clone(),
                    code: code.clone(),
                }));
                self.ready
                    .push_back(Update::Done(RunEnd::Failed { message, code }));
            }
        }
    }

    /// The transport stopped sending. Close what the producer left open, then
    /// report a truncated stream.
    fn end_of_stream(&mut self) {
        self.done = true;
        let mut expanded = std::mem::take(&mut self.expanded);
        expanded.clear();
        self.normalizer.finish(&mut expanded);
        for event in expanded.drain(..) {
            self.handle(event);
        }
        self.expanded = expanded;

        if let Some(verifier) = &self.verifier {
            if let Err(error) = verifier.finish() {
                self.ready.push_back(Update::Error(error));
            }
        }
    }
}

// `S: Unpin` because the queued updates hold an `S` and this stream is polled
// through a `&mut`. Every type that deserializes from JSON is `Unpin` in
// practice — self-referential state would not survive `serde` anyway.
impl<T, S> Stream for RunStream<'_, T, S>
where
    S: DeserializeOwned + Clone + Unpin,
{
    type Item = Update<S>;

    fn poll_next(self: Pin<&mut Self>, cx: &mut TaskContext<'_>) -> Poll<Option<Self::Item>> {
        // Every field is `Unpin`: the boxed event stream is a `Pin<Box<…>>` and
        // everything else is plain data.
        let this = self.get_mut();
        loop {
            if let Some(update) = this.ready.pop_front() {
                return Poll::Ready(Some(update));
            }
            if this.done {
                return Poll::Ready(None);
            }
            match this.events.as_mut().poll_next(cx) {
                Poll::Pending => return Poll::Pending,
                Poll::Ready(Some(Ok(event))) => this.ingest(event),
                Poll::Ready(Some(Err(error))) => {
                    // A broken transport cannot recover, so this ends the run.
                    this.ready.push_back(Update::Error(error));
                    this.done = true;
                }
                Poll::Ready(None) => this.end_of_stream(),
            }
        }
    }
}
