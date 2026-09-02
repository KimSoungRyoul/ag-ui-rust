//! The one extension point: rewriting the event stream on its way out.
//!
//! Everything that wants to observe, drop, rewrite or add events implements
//! [`StreamTransformer`]. There is deliberately no second mechanism — an early
//! draft of this crate also carried a builder of `map_content` / `map_result` /
//! `map_interrupt` closures ported from the .NET SDK, which meant two ways to
//! do the same thing and a pile of `Box<dyn Fn>`. Those hooks are built-in
//! transformers now: see [`ToolResultToState`]. So is the compatibility knob
//! for consumers that predate subagents: [`SubagentVisibility`].
//!
//! Transformers run in the order they were added, each seeing what the previous
//! one produced, before the ordering verifier sees anything. That order is what
//! makes [`FilterToolCalls`] safe: the verifier never sees the half of a tool
//! call that was dropped.
//!
//! ```
//! # use ag_ui::server::{FilterToolCalls, TransformerChain, ToolResultToState};
//! let chain = TransformerChain::new()
//!     .with(FilterToolCalls::deny(["internal_debug"]))
//!     .with(ToolResultToState::snapshot("load_document").replacing());
//! assert_eq!(chain.len(), 2);
//! ```

use std::collections::{HashMap, HashSet};

use crate::{Event, MessageId, PatchOperation, ToolCallId};
use serde_json::Value;

/// Rewrites events on their way from an agent to the transport.
///
/// # Why `&mut self`
///
/// Any useful transformer is a small state machine: dropping a tool call means
/// remembering which id was dropped so its `TOOL_CALL_ARGS` go too. Taking
/// `&mut self` says that directly instead of pushing every implementation into
/// `RefCell`. The chain is owned by the run, so there is no sharing to lose.
///
/// # Contract
///
/// Returning an empty `Vec` drops the event. Returning several events splices
/// them in, in order. A transformer that drops the start of something must drop
/// its continuation and terminator too, or the ordering verifier will reject
/// what it produces.
pub trait StreamTransformer: Send {
    /// Rewrites one event into zero or more events.
    fn transform(&mut self, event: Event) -> Vec<Event>;
}

/// Transformers applied in sequence.
///
/// An empty chain is free: the run skips it entirely rather than allocating a
/// `Vec` per event.
#[derive(Default)]
pub struct TransformerChain {
    transformers: Vec<Box<dyn StreamTransformer>>,
}

impl std::fmt::Debug for TransformerChain {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TransformerChain")
            .field("len", &self.transformers.len())
            .finish()
    }
}

impl TransformerChain {
    /// An empty chain.
    pub fn new() -> Self {
        Self::default()
    }

    /// Appends a transformer, returning the chain for further building.
    #[must_use]
    pub fn with(mut self, transformer: impl StreamTransformer + 'static) -> Self {
        self.push(transformer);
        self
    }

    /// Appends a transformer.
    pub fn push(&mut self, transformer: impl StreamTransformer + 'static) {
        self.transformers.push(Box::new(transformer));
    }

    /// How many transformers are in the chain.
    pub fn len(&self) -> usize {
        self.transformers.len()
    }

    /// Whether the chain would pass every event through untouched.
    pub fn is_empty(&self) -> bool {
        self.transformers.is_empty()
    }

    /// Runs `event` through every transformer in order.
    pub fn transform(&mut self, event: Event) -> Vec<Event> {
        let mut current = vec![event];
        for transformer in &mut self.transformers {
            let mut next = Vec::with_capacity(current.len());
            for event in current.drain(..) {
                next.extend(transformer.transform(event));
            }
            current = next;
            if current.is_empty() {
                break;
            }
        }
        current
    }
}

impl StreamTransformer for TransformerChain {
    fn transform(&mut self, event: Event) -> Vec<Event> {
        Self::transform(self, event)
    }
}

/// Which side of the list passes.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
enum FilterMode {
    Allow,
    Deny,
}

/// Drops whole tool calls by tool name.
///
/// Both halves of a call are removed — `TOOL_CALL_START`, its `TOOL_CALL_ARGS`,
/// the `TOOL_CALL_END`, the `TOOL_CALL_RESULT` and any encrypted reasoning blob
/// attached to it — so what reaches the client is a stream that never mentions
/// the tool.
///
/// ```
/// # use ag_ui::Event;
/// # use ag_ui::server::{FilterToolCalls, StreamTransformer};
/// let mut filter = FilterToolCalls::deny(["secret_tool"]);
/// assert!(filter.transform(Event::tool_call_start("c1", "secret_tool")).is_empty());
/// assert!(filter.transform(Event::tool_call_args("c1", "{}")).is_empty());
/// assert_eq!(filter.transform(Event::tool_call_start("c2", "public")).len(), 1);
/// ```
#[derive(Clone, Debug)]
pub struct FilterToolCalls {
    mode: FilterMode,
    names: HashSet<String>,
    dropped: HashSet<ToolCallId>,
}

impl FilterToolCalls {
    /// Passes only calls to the named tools.
    pub fn allow<I, T>(names: I) -> Self
    where
        I: IntoIterator<Item = T>,
        T: Into<String>,
    {
        Self::new(FilterMode::Allow, names)
    }

    /// Passes everything except calls to the named tools.
    pub fn deny<I, T>(names: I) -> Self
    where
        I: IntoIterator<Item = T>,
        T: Into<String>,
    {
        Self::new(FilterMode::Deny, names)
    }

    fn new<I, T>(mode: FilterMode, names: I) -> Self
    where
        I: IntoIterator<Item = T>,
        T: Into<String>,
    {
        Self {
            mode,
            names: names.into_iter().map(Into::into).collect(),
            dropped: HashSet::new(),
        }
    }

    fn passes(&self, name: &str) -> bool {
        match self.mode {
            FilterMode::Allow => self.names.contains(name),
            FilterMode::Deny => !self.names.contains(name),
        }
    }

    /// Records the verdict for a call and reports whether it should be dropped.
    fn judge(&mut self, id: &ToolCallId, name: &str) -> bool {
        if self.passes(name) {
            self.dropped.remove(id);
            false
        } else {
            self.dropped.insert(id.clone());
            true
        }
    }
}

impl StreamTransformer for FilterToolCalls {
    fn transform(&mut self, event: Event) -> Vec<Event> {
        let drop = match &event {
            Event::ToolCallStart(payload) => {
                self.judge(&payload.tool_call_id, &payload.tool_call_name)
            }
            Event::ToolCallChunk(payload) => match (&payload.tool_call_id, &payload.tool_call_name)
            {
                (Some(id), Some(name)) => self.judge(id, name),
                (Some(id), None) => self.dropped.contains(id),
                _ => false,
            },
            Event::ToolCallArgs(payload) => self.dropped.contains(&payload.tool_call_id),
            Event::ToolCallEnd(payload) => self.dropped.contains(&payload.tool_call_id),
            Event::ToolCallResult(payload) => self.dropped.contains(&payload.tool_call_id),
            Event::ReasoningEncryptedValue(payload) => {
                payload.subtype == crate::ReasoningEncryptedValueSubtype::ToolCall
                    && self.dropped.contains(&ToolCallId::new(&payload.entity_id))
            }
            _ => false,
        };

        if drop { Vec::new() } else { vec![event] }
    }
}

/// How a promoted tool result reaches the client's state.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
enum StateForm {
    /// The result is the new state.
    Snapshot,
    /// The result is an RFC 6902 patch against the current state.
    Delta,
}

/// Promotes a named tool's result into a state event.
///
/// A tool whose job is to produce state — `load_document`, `set_filters` —
/// otherwise forces the agent to emit the result and then publish the state by
/// hand. This transformer does it: when `tool_name` returns, its JSON content
/// becomes a `STATE_SNAPSHOT` or a `STATE_DELTA`.
///
/// The result event is kept by default so the client still sees the tool
/// completed; [`replacing`](Self::replacing) drops it instead. Content that is
/// not JSON of the expected shape is left alone — a malformed result must not
/// take down the run.
///
/// ```
/// # use ag_ui::Event;
/// # use ag_ui::server::{StreamTransformer, ToolResultToState};
/// let mut promote = ToolResultToState::snapshot("load_document").replacing();
/// promote.transform(Event::tool_call_start("c1", "load_document"));
/// let out = promote.transform(Event::tool_call_result("m1", "c1", r#"{"title":"Notes"}"#));
/// assert_eq!(out, vec![Event::state_snapshot(serde_json::json!({"title": "Notes"}))]);
/// ```
#[derive(Clone, Debug)]
pub struct ToolResultToState {
    tool_name: String,
    form: StateForm,
    keep_result: bool,
    names: HashMap<ToolCallId, String>,
}

impl ToolResultToState {
    /// Turns `tool_name`'s result into a `STATE_SNAPSHOT`.
    pub fn snapshot(tool_name: impl Into<String>) -> Self {
        Self::new(tool_name, StateForm::Snapshot)
    }

    /// Turns `tool_name`'s result into a `STATE_DELTA`. The result content must
    /// be a JSON array of RFC 6902 operations.
    pub fn delta(tool_name: impl Into<String>) -> Self {
        Self::new(tool_name, StateForm::Delta)
    }

    fn new(tool_name: impl Into<String>, form: StateForm) -> Self {
        Self {
            tool_name: tool_name.into(),
            form,
            keep_result: true,
            names: HashMap::new(),
        }
    }

    /// Drops the `TOOL_CALL_RESULT` instead of emitting it alongside the state
    /// event.
    #[must_use]
    pub fn replacing(mut self) -> Self {
        self.keep_result = false;
        self
    }

    fn state_event(&self, content: &str) -> Option<Event> {
        match self.form {
            StateForm::Snapshot => serde_json::from_str::<Value>(content)
                .ok()
                .map(Event::state_snapshot),
            StateForm::Delta => serde_json::from_str::<Vec<PatchOperation>>(content)
                .ok()
                .map(Event::state_delta),
        }
    }
}

impl StreamTransformer for ToolResultToState {
    fn transform(&mut self, event: Event) -> Vec<Event> {
        match &event {
            Event::ToolCallStart(payload) => {
                if payload.tool_call_name == self.tool_name {
                    self.names
                        .insert(payload.tool_call_id.clone(), payload.tool_call_name.clone());
                }
            }
            Event::ToolCallChunk(payload) => {
                match (&payload.tool_call_id, &payload.tool_call_name) {
                    (Some(id), Some(name)) if name == &self.tool_name => {
                        self.names.insert(id.clone(), name.clone());
                    }
                    _ => {}
                }
            }
            Event::ToolCallResult(payload) => {
                let promoted = self
                    .names
                    .remove(&payload.tool_call_id)
                    .and_then(|_| self.state_event(&payload.content));
                if let Some(state) = promoted {
                    return if self.keep_result {
                        vec![event, state]
                    } else {
                        vec![state]
                    };
                }
            }
            _ => {}
        }
        vec![event]
    }
}

/// What a consumer sees of an agent's subagents.
///
/// [`Attributed`](Self::Attributed) — the default, and no transformer at all —
/// sends the stream as the agent emitted it. The other two exist because a
/// client older than `@ag-ui/client` 0.0.59 rejects the `SUBAGENT_*` event
/// *types* while decoding: an unknown field is tolerated, an unknown event
/// type is not, and there is nothing a client can do about it after the
/// fact. A producer with such consumers must not send them, and this is how
/// it does not.
///
/// Upstream's integrations default to inline and make the full surface
/// opt-in. This crate defaults the other way, because a transformer that
/// rewrites the stream is opt-in here like every other: an agent that wrote
/// `ctx.subagent(..)` meant it, and silently flattening what it said is the
/// kind of surprise the design notes argue against. Flip it per endpoint when
/// your consumers are older:
///
/// ```
/// # use ag_ui::RunOutcome;
/// # use ag_ui::server::{Agent, Result, RunContext, Runner, SubagentVisibility};
/// # struct MyAgent;
/// # impl Agent for MyAgent {
/// #     type State = ();
/// #     async fn run(&self, _ctx: &mut RunContext<()>) -> Result<RunOutcome> { Ok(RunOutcome::Success) }
/// # }
/// let runner = Runner::new(MyAgent).transformer(SubagentVisibility::inline());
/// # let _ = runner;
/// ```
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum SubagentVisibility {
    /// The full surface: the lifecycle events, and `subagentRunId` on
    /// everything a subagent produced.
    #[default]
    Attributed,
    /// The pre-subagent shape: no lifecycle events and no `subagentRunId`
    /// anywhere — not on events, not on the messages inside
    /// `MESSAGES_SNAPSHOT` or the `RUN_STARTED` input echo, not on the
    /// interrupts a paused run reports. A subagent's own text arrives as the
    /// parent's work.
    Inline,
    /// Only the parent's own events. Everything a subagent produced is
    /// dropped — including the result of a call it requested, even when the
    /// parent executed it, since a result for a call the consumer never saw
    /// is a protocol error.
    ///
    /// The one thing kept is the run's shared state. A `STATE_SNAPSHOT` or
    /// `STATE_DELTA` a subagent published describes the *thread's* state,
    /// not the subagent's work: a client that never saw it would mirror a
    /// stale state and send that back on its next request. It goes out with
    /// the tag cleared, as the parent's.
    Hidden,
}

impl SubagentVisibility {
    /// The transformer for this mode. Pointless but harmless for
    /// [`Attributed`](Self::Attributed), which passes everything through.
    pub fn filter(self) -> SubagentFilter {
        SubagentFilter::new(self)
    }

    /// The transformer for [`Inline`](Self::Inline).
    pub fn inline() -> SubagentFilter {
        Self::Inline.filter()
    }

    /// The transformer for [`Hidden`](Self::Hidden).
    pub fn hidden() -> SubagentFilter {
        Self::Hidden.filter()
    }
}

/// The transformer behind [`SubagentVisibility`].
///
/// [`Inline`](SubagentVisibility::Inline) is stateless.
/// [`Hidden`](SubagentVisibility::Hidden) remembers which ids a subagent
/// owns — for the rest of the run, not only while they are open — so an
/// untagged continuation, re-open or result for a subagent's entity is
/// dropped with the rest of it rather than leaking into the parent's stream.
/// That is the owner-aware verifier's reading too: the first writer owns the
/// id, and an absent tag agrees with any owner.
#[derive(Debug)]
pub struct SubagentFilter {
    mode: SubagentVisibility,
    /// Text and reasoning message ids a subagent owns.
    hidden_messages: HashSet<MessageId>,
    /// Tool call ids a subagent owns — or that sit in a message it owns.
    hidden_tool_calls: HashSet<ToolCallId>,
    /// The id the last chunk of each family carried, so a chunk that carries
    /// none is judged by the stream it continues.
    last_text_chunk: Option<MessageId>,
    last_reasoning_chunk: Option<MessageId>,
    last_tool_chunk: Option<ToolCallId>,
}

impl SubagentFilter {
    /// A filter for `mode`.
    pub fn new(mode: SubagentVisibility) -> Self {
        Self {
            mode,
            hidden_messages: HashSet::new(),
            hidden_tool_calls: HashSet::new(),
            last_text_chunk: None,
            last_reasoning_chunk: None,
            last_tool_chunk: None,
        }
    }

    /// The mode this filter applies.
    pub fn mode(&self) -> SubagentVisibility {
        self.mode
    }

    /// Strips the subagent surface from an event, or drops it entirely when
    /// it *is* the subagent surface.
    fn inline(mut event: Event) -> Vec<Event> {
        match &mut event {
            Event::SubagentStarted(_) | Event::SubagentFinished(_) | Event::SubagentError(_) => {
                return Vec::new();
            }
            Event::MessagesSnapshot(snapshot) => {
                for message in &mut snapshot.messages {
                    message.set_subagent_run_id(None);
                }
            }
            Event::RunStarted(started) => {
                if let Some(input) = &mut started.input {
                    for message in &mut input.messages {
                        message.set_subagent_run_id(None);
                    }
                }
            }
            Event::RunFinished(finished) => Self::strip_interrupt_tags(finished),
            _ => {
                event.clear_subagent_run_id();
            }
        }
        vec![event]
    }

    /// The interrupts a paused run reports name the subagent that raised
    /// them, and a consumer that never saw that subagent has no group to
    /// file them under. The question still stands, so the interrupt stays;
    /// only the tag goes.
    fn strip_interrupt_tags(finished: &mut crate::RunFinishedEvent) {
        if let Some(crate::RunOutcome::Interrupt { interrupts }) = &mut finished.outcome {
            for interrupt in interrupts {
                interrupt.subagent_run_id = None;
            }
        }
    }

    /// Keeps the parent's events and drops a subagent's, remembering the ids
    /// a subagent owns so that an untagged event is judged by its opener.
    fn hidden(&mut self, mut event: Event) -> Vec<Event> {
        let owned = event.subagent_run_id().is_some();
        let keep = match &event {
            Event::SubagentStarted(_) | Event::SubagentFinished(_) | Event::SubagentError(_) => {
                false
            }

            Event::TextMessageStart(e) => self.claim_message(&e.message_id, owned),
            Event::TextMessageContent(e) => self.message_kept(&e.message_id, owned),
            Event::TextMessageEnd(e) => self.message_kept(&e.message_id, owned),
            Event::TextMessageChunk(e) => match &e.message_id {
                Some(id) => {
                    self.last_text_chunk = Some(id.clone());
                    self.claim_message(id, owned)
                }
                None => !owned && !self.last_hidden(self.last_text_chunk.as_ref()),
            },

            Event::ReasoningStart(e) => self.claim_message(&e.message_id, owned),
            Event::ReasoningMessageStart(e) => self.claim_message(&e.message_id, owned),
            Event::ReasoningMessageContent(e) => self.message_kept(&e.message_id, owned),
            Event::ReasoningMessageEnd(e) => self.message_kept(&e.message_id, owned),
            Event::ReasoningEnd(e) => self.message_kept(&e.message_id, owned),
            Event::ReasoningMessageChunk(e) => match &e.message_id {
                Some(id) => {
                    self.last_reasoning_chunk = Some(id.clone());
                    self.claim_message(id, owned)
                }
                None => !owned && !self.last_hidden(self.last_reasoning_chunk.as_ref()),
            },

            // A call belongs to the message that carries it, so an untagged
            // call inside a subagent's message is the subagent's.
            Event::ToolCallStart(e) => {
                let hidden = owned || self.in_hidden_message(e.parent_message_id.as_ref());
                self.claim_tool_call(&e.tool_call_id, hidden)
            }
            Event::ToolCallChunk(e) => match &e.tool_call_id {
                Some(id) => {
                    self.last_tool_chunk = Some(id.clone());
                    let hidden = owned || self.in_hidden_message(e.parent_message_id.as_ref());
                    self.claim_tool_call(id, hidden)
                }
                None => {
                    !owned
                        && !self
                            .last_tool_chunk
                            .as_ref()
                            .is_some_and(|id| self.hidden_tool_calls.contains(id))
                }
            },
            Event::ToolCallArgs(e) => self.tool_call_kept(&e.tool_call_id, owned),
            Event::ToolCallEnd(e) => self.tool_call_kept(&e.tool_call_id, owned),
            // A result for a call the consumer never saw is a protocol error,
            // whoever executed it.
            Event::ToolCallResult(e) => self.tool_call_kept(&e.tool_call_id, owned),

            // The thread's state, whoever published it.
            Event::StateSnapshot(_) | Event::StateDelta(_) => true,

            _ => !owned,
        };
        if !keep {
            return Vec::new();
        }
        match &mut event {
            // Authoritative: the snapshot restates the conversation, so what
            // is hidden is restated with it.
            Event::MessagesSnapshot(snapshot) => {
                self.hidden_messages.clear();
                self.hidden_tool_calls.clear();
                self.seed_hidden(&snapshot.messages);
                let hidden_calls = &self.hidden_tool_calls;
                snapshot
                    .messages
                    .retain(|message| Self::message_shown(message, hidden_calls));
            }
            // History, not a rewrite: what it carries is remembered alongside
            // what the run has already shown.
            Event::RunStarted(started) => {
                if let Some(input) = &mut started.input {
                    self.seed_hidden(&input.messages);
                    let hidden_calls = &self.hidden_tool_calls;
                    input
                        .messages
                        .retain(|message| Self::message_shown(message, hidden_calls));
                }
            }
            Event::StateSnapshot(_) | Event::StateDelta(_) => {
                event.clear_subagent_run_id();
            }
            Event::RunFinished(finished) => Self::strip_interrupt_tags(finished),
            _ => {}
        }
        vec![event]
    }

    /// Records a subagent's message id, or reports whether an untagged
    /// opener is the parent's: the first writer owns the id, so an untagged
    /// re-open of a subagent's message is still the subagent's.
    fn claim_message(&mut self, id: &MessageId, owned: bool) -> bool {
        if owned {
            self.hidden_messages.insert(id.clone());
            return false;
        }
        !self.hidden_messages.contains(id)
    }

    fn message_kept(&self, id: &MessageId, owned: bool) -> bool {
        !owned && !self.hidden_messages.contains(id)
    }

    fn claim_tool_call(&mut self, id: &ToolCallId, hidden: bool) -> bool {
        if hidden {
            self.hidden_tool_calls.insert(id.clone());
            return false;
        }
        !self.hidden_tool_calls.contains(id)
    }

    fn tool_call_kept(&self, id: &ToolCallId, owned: bool) -> bool {
        !owned && !self.hidden_tool_calls.contains(id)
    }

    fn in_hidden_message(&self, parent: Option<&MessageId>) -> bool {
        parent.is_some_and(|id| self.hidden_messages.contains(id))
    }

    fn last_hidden(&self, last: Option<&MessageId>) -> bool {
        last.is_some_and(|id| self.hidden_messages.contains(id))
    }

    /// Remembers the subagent-owned messages a replay carries, and the tool
    /// calls inside them.
    fn seed_hidden(&mut self, messages: &[crate::Message]) {
        for message in messages {
            if message.subagent_run_id().is_none() {
                continue;
            }
            self.hidden_messages.insert(message.id().clone());
            if let crate::Message::Assistant(assistant) = message {
                for call in assistant.tool_calls.iter().flatten() {
                    self.hidden_tool_calls.insert(call.id.clone());
                }
            }
        }
    }

    /// Whether a replayed message reaches the consumer: the parent's own,
    /// unless it answers a call the consumer never saw.
    fn message_shown(message: &crate::Message, hidden_calls: &HashSet<ToolCallId>) -> bool {
        if message.subagent_run_id().is_some() {
            return false;
        }
        match message {
            crate::Message::Tool(tool) => !hidden_calls.contains(&tool.tool_call_id),
            _ => true,
        }
    }
}

impl StreamTransformer for SubagentFilter {
    fn transform(&mut self, event: Event) -> Vec<Event> {
        match self.mode {
            SubagentVisibility::Attributed => vec![event],
            SubagentVisibility::Inline => Self::inline(event),
            SubagentVisibility::Hidden => self.hidden(event),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{ReasoningEncryptedValueSubtype, TextMessageRole};
    use serde_json::json;

    #[test]
    fn allow_list_drops_everything_unlisted() {
        let mut filter = FilterToolCalls::allow(["search"]);
        assert_eq!(
            filter
                .transform(Event::tool_call_start("c1", "search"))
                .len(),
            1
        );
        assert!(
            filter
                .transform(Event::tool_call_start("c2", "delete_everything"))
                .is_empty()
        );
        assert!(
            filter
                .transform(Event::tool_call_args("c2", "{}"))
                .is_empty()
        );
        assert!(
            filter
                .transform(Event::tool_call_result("m1", "c2", "gone"))
                .is_empty()
        );
        assert_eq!(filter.transform(Event::tool_call_args("c1", "{}")).len(), 1);
    }

    #[test]
    fn filter_leaves_unrelated_events_alone() {
        let mut filter = FilterToolCalls::deny(["nope"]);
        let event = Event::text_message_start("m1", TextMessageRole::Assistant);
        assert_eq!(filter.transform(event.clone()), vec![event]);
    }

    #[test]
    fn filter_drops_encrypted_reasoning_for_a_dropped_call() {
        let mut filter = FilterToolCalls::deny(["nope"]);
        filter.transform(Event::tool_call_start("c1", "nope"));
        let blob = Event::reasoning_encrypted_value(
            ReasoningEncryptedValueSubtype::ToolCall,
            "c1",
            "opaque",
        );
        assert!(filter.transform(blob).is_empty());
    }

    #[test]
    fn promoting_keeps_the_result_by_default() {
        let mut promote = ToolResultToState::snapshot("load");
        promote.transform(Event::tool_call_start("c1", "load"));
        let result = Event::tool_call_result("m1", "c1", r#"{"a":1}"#);
        assert_eq!(
            promote.transform(result.clone()),
            vec![result, Event::state_snapshot(json!({"a": 1}))]
        );
    }

    #[test]
    fn promoting_a_patch_emits_a_delta() {
        let mut promote = ToolResultToState::delta("patch_state").replacing();
        promote.transform(Event::tool_call_start("c1", "patch_state"));
        let content = r#"[{"op":"replace","path":"/step","value":2}]"#;
        assert_eq!(
            promote.transform(Event::tool_call_result("m1", "c1", content)),
            vec![Event::state_delta(vec![PatchOperation::replace(
                "/step", 2
            )])]
        );
    }

    #[test]
    fn unparseable_content_passes_through_untouched() {
        let mut promote = ToolResultToState::snapshot("load").replacing();
        promote.transform(Event::tool_call_start("c1", "load"));
        let result = Event::tool_call_result("m1", "c1", "not json");
        assert_eq!(promote.transform(result.clone()), vec![result]);
    }

    #[test]
    fn other_tools_are_not_promoted() {
        let mut promote = ToolResultToState::snapshot("load");
        promote.transform(Event::tool_call_start("c1", "something_else"));
        let result = Event::tool_call_result("m1", "c1", r#"{"a":1}"#);
        assert_eq!(promote.transform(result.clone()), vec![result]);
    }

    #[test]
    fn chain_runs_transformers_in_order() {
        let mut chain = TransformerChain::new()
            .with(FilterToolCalls::deny(["load"]))
            .with(ToolResultToState::snapshot("load"));
        // The filter removes the call first, so the promoter never sees it.
        assert!(
            chain
                .transform(Event::tool_call_start("c1", "load"))
                .is_empty()
        );
        assert!(
            chain
                .transform(Event::tool_call_result("m1", "c1", r#"{"a":1}"#))
                .is_empty()
        );
    }
}
