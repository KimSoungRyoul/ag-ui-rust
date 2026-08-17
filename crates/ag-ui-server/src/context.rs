//! What an agent is handed for one run.

use std::future::Future;

use ag_ui_core::{
    Context, Event, Message, MessageId, ResumeEntry, RunAgentInput, RunId, StepName,
    TextMessageRole, ThreadId, Tool, ToolCallId,
};
use futures_channel::mpsc;
use futures_util::future::{Either, select};
use serde_json::Value;

use crate::agent::AgentState;
use crate::cancel::{CancellationToken, Cancelled};
use crate::emit::{
    EventReceiver, EventSink, MessageHandle, ReasoningHandle, StepGuard, ToolCallHandle,
};
use crate::error::{Error, Result};
use crate::state::StateManager;
use crate::transform::TransformerChain;

/// The request, the state, the event sink and the cancellation flag — one
/// run's whole world.
///
/// An agent gets `&mut RunContext<S>` and emits through it. Every emitter takes
/// `&mut self`, which is what makes two overlapping messages a borrow-check
/// error rather than a protocol violation discovered by a confused frontend.
///
/// ```
/// # use ag_ui_core::RunAgentInput;
/// # use ag_ui_server::RunContext;
/// # let (mut ctx, _events) = RunContext::<()>::new(RunAgentInput::new("thread-1", "run-1"))?;
/// assert_eq!(ctx.thread_id().as_str(), "thread-1");
/// assert!(ctx.messages().is_empty());
///
/// let mut message = ctx.assistant_message()?;
/// message.delta("Hello")?;
/// message.end()?;
/// # Ok::<(), ag_ui_server::Error>(())
/// ```
#[derive(Debug)]
pub struct RunContext<S> {
    input: RunAgentInput,
    state: S,
    states: StateManager,
    sink: EventSink,
    next_message: u64,
    next_tool_call: u64,
}

impl<S: AgentState> RunContext<S> {
    /// Builds a context and the receiving half of its event stream.
    ///
    /// This is the harness for unit-testing an [`Agent`](crate::Agent) without
    /// the run driver: call the agent's body, then assert on
    /// [`EventReceiver::drain`]. Nothing emits `RUN_STARTED` here — that is the
    /// driver's job, and skipping it lets a test exercise one method in
    /// isolation.
    pub fn new(input: RunAgentInput) -> Result<(Self, EventReceiver)> {
        let (tx, rx) = mpsc::unbounded();
        let sink = EventSink::new(tx, TransformerChain::new(), CancellationToken::new());
        let state = decode_state(&input.state)?;
        Ok((Self::from_parts(input, state, sink), EventReceiver::new(rx)))
    }

    /// Assembles a context from an already-decoded state.
    ///
    /// The run driver decodes first so that a state that does not fit `S` is
    /// reported through the sink it still owns, as a `RUN_ERROR`.
    pub(crate) fn from_parts(input: RunAgentInput, state: S, sink: EventSink) -> Self {
        Self {
            input,
            state,
            states: StateManager::new(),
            sink,
            next_message: 0,
            next_tool_call: 0,
        }
    }

    /// The typed state, as of the last publish.
    pub fn state(&self) -> &S {
        &self.state
    }

    /// The typed state, mutably. Nothing is emitted until you call
    /// [`publish_state`](Self::publish_state).
    pub fn state_mut(&mut self) -> &mut S {
        &mut self.state
    }

    /// Replaces the state and publishes the change.
    ///
    /// The first publish of a run is a `STATE_SNAPSHOT`; later ones are a
    /// `STATE_DELTA` unless the patch would be no smaller than the snapshot.
    /// See [`StateManager`].
    pub fn set_state(&mut self, state: &S) -> Result<()> {
        let value = serde_json::to_value(state)?;
        // Round-tripping keeps `state()` and the published snapshot in step
        // without asking `S` to be `Clone`.
        self.state = serde_json::from_value(value.clone())?;
        self.publish_value(value)
    }

    /// Mutates the state in place and publishes the change.
    ///
    /// ```
    /// # use ag_ui_core::RunAgentInput;
    /// # use ag_ui_server::RunContext;
    /// # use serde::{Deserialize, Serialize};
    /// #[derive(Default, Serialize, Deserialize)]
    /// struct Draft { revision: u32 }
    ///
    /// # let (mut ctx, _events) = RunContext::<Draft>::new(RunAgentInput::new("t", "r"))?;
    /// ctx.update_state(|draft| draft.revision += 1)?;
    /// assert_eq!(ctx.state().revision, 1);
    /// # Ok::<(), ag_ui_server::Error>(())
    /// ```
    pub fn update_state(&mut self, update: impl FnOnce(&mut S)) -> Result<()> {
        update(&mut self.state);
        self.publish_state()
    }

    /// Publishes whatever [`state_mut`](Self::state_mut) left behind.
    ///
    /// A no-op when nothing changed since the last publish.
    pub fn publish_state(&mut self) -> Result<()> {
        let value = serde_json::to_value(&self.state)?;
        self.publish_value(value)
    }

    fn publish_value(&mut self, value: Value) -> Result<()> {
        match self.states.publish(value)?.into_event() {
            Some(event) => self.emit(event),
            None => Ok(()),
        }
    }
}

impl<S> RunContext<S> {
    /// The whole request, for anything the accessors do not cover.
    pub fn input(&self) -> &RunAgentInput {
        &self.input
    }

    /// The conversation this run belongs to.
    pub fn thread_id(&self) -> &ThreadId {
        &self.input.thread_id
    }

    /// This run's id.
    pub fn run_id(&self) -> &RunId {
        &self.input.run_id
    }

    /// The run that spawned this one, for nested agents.
    pub fn parent_run_id(&self) -> Option<&RunId> {
        self.input.parent_run_id.as_ref()
    }

    /// Conversation history, oldest first.
    pub fn messages(&self) -> &[Message] {
        &self.input.messages
    }

    /// Tools the client is offering for this run.
    pub fn tools(&self) -> &[Tool] {
        &self.input.tools
    }

    /// One offered tool by name.
    pub fn tool(&self, name: &str) -> Option<&Tool> {
        self.input.tools.iter().find(|tool| tool.name == name)
    }

    /// Ambient context entries.
    pub fn context(&self) -> &[Context] {
        &self.input.context
    }

    /// Arbitrary passthrough properties, opaque to the protocol.
    pub fn forwarded_props(&self) -> &Value {
        &self.input.forwarded_props
    }

    /// Answers to the interrupts a previous run paused on.
    ///
    /// Empty unless this request resumes a paused run.
    pub fn resume(&self) -> &[ResumeEntry] {
        self.input.resume.as_deref().unwrap_or_default()
    }

    /// The answer to one interrupt, by its [`Interrupt::id`].
    ///
    /// [`Interrupt::id`]: ag_ui_core::Interrupt::id
    pub fn resume_for(&self, interrupt_id: &str) -> Option<&ResumeEntry> {
        self.resume()
            .iter()
            .find(|entry| entry.interrupt_id == interrupt_id)
    }

    /// Whether this request resumes a paused run.
    pub fn is_resume(&self) -> bool {
        self.input.is_resume()
    }

    /// Whether the run has been cancelled.
    pub fn is_cancelled(&self) -> bool {
        self.sink.cancel_token().is_cancelled()
    }

    /// [`Error::Cancelled`] once the run is cancelled, for use with `?`.
    pub fn check_cancelled(&self) -> Result<()> {
        if self.is_cancelled() {
            return Err(Error::Cancelled);
        }
        Ok(())
    }

    /// A handle a transport can trip on client disconnect.
    pub fn cancel_token(&self) -> CancellationToken {
        self.sink.cancel_token().clone()
    }

    /// Resolves once the run is cancelled.
    pub fn cancelled(&self) -> Cancelled {
        self.sink.cancel_token().cancelled()
    }

    /// Races `future` against cancellation, returning `None` if cancellation
    /// won.
    ///
    /// The way to make a long model call interruptible:
    ///
    /// ```
    /// # use ag_ui_core::RunAgentInput;
    /// # use ag_ui_server::{Error, RunContext};
    /// # let rt = tokio::runtime::Builder::new_current_thread().build().unwrap();
    /// # rt.block_on(async {
    /// # let (ctx, _events) = RunContext::<()>::new(RunAgentInput::new("t", "r"))?;
    /// let answer = ctx
    ///     .until_cancelled(async { "the model's reply" })
    ///     .await
    ///     .ok_or(Error::Cancelled)?;
    /// assert_eq!(answer, "the model's reply");
    /// # Ok::<(), Error>(())
    /// # })?;
    /// # Ok::<(), Error>(())
    /// ```
    /// Deliberately not an `async fn`: that would capture `&self` in the
    /// returned future, and a future holding a borrow of the run context is
    /// only `Send` if the context is `Sync` — which it is not, since a
    /// transformer only has to be `Send`.
    pub fn until_cancelled<F: Future>(&self, future: F) -> impl Future<Output = Option<F::Output>> {
        let cancelled = self.cancelled();
        async move {
            futures_util::pin_mut!(future, cancelled);
            match select(future, cancelled).await {
                Either::Left((output, _)) => Some(output),
                Either::Right(((), _)) => None,
            }
        }
    }

    /// Emits an event as-is — the escape hatch under the typed emitters.
    ///
    /// Everything the handles emit goes through here, so a raw event is
    /// transformed and verified like any other.
    pub fn emit(&mut self, event: Event) -> Result<()> {
        self.sink.emit(event)
    }

    /// A fresh message id, unique within the run.
    ///
    /// Derived from the run id and a counter rather than a UUID: the protocol
    /// asks for opaque strings, this crate takes no `uuid` dependency, and a
    /// deterministic id makes a recorded stream diffable. Pass your own id to
    /// [`message_with_id`](Self::message_with_id) when you need one.
    pub fn new_message_id(&mut self) -> MessageId {
        self.next_message += 1;
        MessageId::new(format!("{}-msg-{}", self.id_prefix(), self.next_message))
    }

    /// A fresh tool call id, unique within the run.
    pub fn new_tool_call_id(&mut self) -> ToolCallId {
        self.next_tool_call += 1;
        ToolCallId::new(format!("{}-call-{}", self.id_prefix(), self.next_tool_call))
    }

    fn id_prefix(&self) -> &str {
        if self.input.run_id.is_empty() {
            "run"
        } else {
            self.input.run_id.as_str()
        }
    }

    /// Opens an assistant message under a fresh id — `TEXT_MESSAGE_START`.
    pub fn assistant_message(&mut self) -> Result<MessageHandle<'_>> {
        self.message(TextMessageRole::Assistant)
    }

    /// Opens a message with the given role under a fresh id.
    pub fn message(&mut self, role: TextMessageRole) -> Result<MessageHandle<'_>> {
        let id = self.new_message_id();
        self.message_with_id(id, role)
    }

    /// Opens a message under an id you choose.
    pub fn message_with_id(
        &mut self,
        id: impl Into<MessageId>,
        role: TextMessageRole,
    ) -> Result<MessageHandle<'_>> {
        MessageHandle::start(&mut self.sink, id.into(), role)
    }

    /// Emits a whole assistant message — start, content, end — and returns its
    /// id.
    pub fn say(&mut self, text: impl Into<String>) -> Result<MessageId> {
        let mut message = self.message(TextMessageRole::Assistant)?;
        message.delta(text)?;
        let id = message.id().clone();
        message.end()?;
        Ok(id)
    }

    /// Opens a reasoning block under a fresh id — `REASONING_START`.
    pub fn reasoning(&mut self) -> Result<ReasoningHandle<'_>> {
        let id = self.new_message_id();
        self.reasoning_with_id(id)
    }

    /// Opens a reasoning block under an id you choose.
    pub fn reasoning_with_id(&mut self, id: impl Into<MessageId>) -> Result<ReasoningHandle<'_>> {
        ReasoningHandle::start(&mut self.sink, id.into())
    }

    /// Emits a whole reasoning block in one call and returns its id.
    pub fn think(&mut self, text: impl Into<String>) -> Result<MessageId> {
        let mut reasoning = self.reasoning()?;
        reasoning.delta(text)?;
        let id = reasoning.id().clone();
        reasoning.end()?;
        Ok(id)
    }

    /// Opens a call to `name` under a fresh id — `TOOL_CALL_START`.
    pub fn tool_call(&mut self, name: &str) -> Result<ToolCallHandle<'_>> {
        let id = self.new_tool_call_id();
        self.tool_call_with_id(id, name)
    }

    /// Opens a call to `name` under an id you choose.
    pub fn tool_call_with_id(
        &mut self,
        id: impl Into<ToolCallId>,
        name: &str,
    ) -> Result<ToolCallHandle<'_>> {
        let id = id.into();
        let result_message_id = self.new_message_id();
        ToolCallHandle::start(&mut self.sink, id, name, None, result_message_id)
    }

    /// Opens a named step — `STEP_STARTED`.
    ///
    /// The returned guard dereferences to this context, and emits
    /// `STEP_FINISHED` when it drops.
    pub fn step(&mut self, name: impl Into<StepName>) -> Result<StepGuard<'_, S>> {
        StepGuard::start(self, name.into())
    }

    /// Whether a terminal event has already gone out.
    pub(crate) fn is_terminated(&self) -> bool {
        self.sink.is_terminated()
    }

    /// Recovers the sink so the run driver can emit the terminal event through
    /// the same transformers and the same verifier the agent used.
    pub(crate) fn into_sink(self) -> EventSink {
        self.sink
    }
}

/// Reads `RunAgentInput::state` as `S`.
///
/// An absent state — JSON `null`, or the empty object clients send for "no
/// state yet" — becomes `S::default()` rather than a deserialization error, so
/// a stateless agent (`State = ()`) works against every client.
pub(crate) fn decode_state<S: AgentState>(value: &Value) -> Result<S> {
    let empty = value.is_null() || value.as_object().is_some_and(serde_json::Map::is_empty);
    if empty {
        return Ok(S::default());
    }
    Ok(serde_json::from_value(value.clone())?)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde::{Deserialize, Serialize};
    use serde_json::json;

    #[derive(Debug, Default, PartialEq, Serialize, Deserialize)]
    struct Counter {
        clicks: u32,
    }

    fn context<S: AgentState>(input: RunAgentInput) -> (RunContext<S>, EventReceiver) {
        RunContext::new(input).expect("state should decode")
    }

    #[test]
    fn ids_are_derived_from_the_run_id() {
        let (mut ctx, _events) = context::<()>(RunAgentInput::new("t", "run-7"));
        assert_eq!(ctx.new_message_id().as_str(), "run-7-msg-1");
        assert_eq!(ctx.new_message_id().as_str(), "run-7-msg-2");
        assert_eq!(ctx.new_tool_call_id().as_str(), "run-7-call-1");
    }

    #[test]
    fn an_empty_state_object_decodes_to_the_default() {
        let mut input = RunAgentInput::new("t", "r");
        input.state = json!({});
        let (ctx, _events) = context::<Counter>(input);
        assert_eq!(ctx.state(), &Counter::default());
    }

    #[test]
    fn typed_state_comes_from_the_input() {
        let mut input = RunAgentInput::new("t", "r");
        input.state = json!({"clicks": 3});
        let (ctx, _events) = context::<Counter>(input);
        assert_eq!(ctx.state().clicks, 3);
    }

    #[test]
    fn a_state_that_does_not_fit_is_an_error() {
        let mut input = RunAgentInput::new("t", "r");
        input.state = json!({"clicks": "three"});
        let error = RunContext::<Counter>::new(input).expect_err("should not decode");
        assert!(matches!(error, Error::Json(_)), "{error}");
    }

    #[test]
    fn resume_entries_are_addressable_by_interrupt_id() {
        let mut input = RunAgentInput::new("t", "r");
        input.resume = Some(vec![ResumeEntry::resolved("i-1", json!(true))]);
        let (ctx, _events) = context::<()>(input);
        assert!(ctx.is_resume());
        assert_eq!(ctx.resume().len(), 1);
        assert!(ctx.resume_for("i-1").is_some());
        assert!(ctx.resume_for("i-2").is_none());
    }

    #[test]
    fn a_cancelled_run_fails_every_emit() {
        let (mut ctx, _events) = context::<()>(RunAgentInput::new("t", "r"));
        ctx.cancel_token().cancel();
        assert!(ctx.is_cancelled());
        let error = ctx.say("too late").expect_err("emit should fail");
        assert!(error.is_cancelled(), "{error}");
    }

    #[test]
    fn a_dropped_receiver_disconnects_the_run() {
        let (mut ctx, events) = context::<()>(RunAgentInput::new("t", "r"));
        drop(events);
        let error = ctx.say("nobody home").expect_err("emit should fail");
        assert!(error.is_disconnected(), "{error}");
    }
}
