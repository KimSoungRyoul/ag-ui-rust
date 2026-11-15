//! The one extension point: rewriting the event stream on its way out.
//!
//! Event observers, filters and rewriters implement [`StreamTransformer`].
//! Built-in transformers include [`ToolResultToState`] and
//! [`SubagentVisibility`].
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
use std::convert::Infallible;

use crate::server::error::{Result, Rule, VerificationError};
use crate::streams::{ChunkError, Kind, Opener, Stream, Streams};
use crate::{Event, EventType, Message, MessageId, PatchOperation, SubagentRunId, ToolCallId};
use serde_json::Value;

/// Rewrites events on their way from an agent to the transport.
///
/// # Contract
///
/// Returning an empty `Vec` drops the event. Returning several events splices
/// them in, in order. A transformer that drops the start of something must drop
/// its continuation and terminator too. The verifier cannot identify every
/// orphaned ID-less chunk after filtering. Normalize chunks first, or track
/// the producer's open streams before deciding which chunks to drop.
///
/// Dropping is silent: the emit that produced the event returns `Ok`. A
/// transformer that cannot tell what to do with an event, and must not let
/// it through, says so from [`try_transform`](Self::try_transform) instead.
pub trait StreamTransformer: Send {
    /// Rewrites one event into zero or more events.
    fn transform(&mut self, event: Event) -> Vec<Event>;

    /// Rewrites one event, or refuses it. This is the method a hosted run
    /// calls.
    ///
    /// An `Err` fails the emit that produced the event with that error, and
    /// nothing of the event is sent: the agent is told, where an empty `Vec`
    /// drops the event without a word. The default never refuses: it is
    /// [`transform`](Self::transform).
    ///
    /// Implement it for a transformer that can meet an event it is unable to
    /// judge, and write `transform` as the same rewrite with such an event
    /// dropped, for a caller that has nowhere to report a refusal.
    ///
    /// # Errors
    ///
    /// Whatever the transformer refuses the event with.
    /// [`FilterToolCalls`] refuses an event of a call it never saw start.
    fn try_transform(&mut self, event: Event) -> Result<Vec<Event>> {
        Ok(self.transform(event))
    }

    /// Learns the conversation the run continues — the request's `messages` —
    /// once, before the run's first event.
    ///
    /// Called whether or not `RUN_STARTED` echoes the input, which is optional
    /// and off by default. A transformer that judges an event by the id it
    /// names needs it: a resumed run may answer a tool call the interrupted
    /// run opened, and that call is in the history, not in this run's events.
    /// The default ignores it.
    fn seed(&mut self, history: &[Message]) {
        let _ = history;
    }

    /// Learns the events the run has already sent, once, after
    /// [`seed`](Self::seed) and before the first event of a driver that
    /// continues the run: [`Runner::continue_run`].
    ///
    /// `sent` is what a consumer holds: the output of the chain the earlier
    /// driver ran, not what its agent emitted. An event that chain dropped is
    /// not in it, and one it rewrote is there as rewritten. It may begin with
    /// the runs that came before on the same stream.
    ///
    /// A transformer that judges an event by the id it names needs it, as it
    /// needs the history: a continuation finishes and answers calls the
    /// earlier driver opened. The default ignores it.
    ///
    /// [`Runner::continue_run`]: crate::server::Runner::continue_run
    fn replay(&mut self, sent: &[Event]) {
        let _ = sent;
    }
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

    /// Runs `event` through every transformer in order. An event a
    /// transformer would refuse is dropped: see
    /// [`try_transform`](Self::try_transform).
    pub fn transform(&mut self, event: Event) -> Vec<Event> {
        let Ok(events) = self.pass(event, |transformer, event| {
            Ok::<_, Infallible>(transformer.transform(event))
        });
        events
    }

    /// Runs `event` through every transformer in order, or fails with the
    /// refusal of the first that refuses what it is given.
    ///
    /// Nothing comes out of a refused event. The transformers before the one
    /// that refused have seen it all the same, and remember what they
    /// remember of an event.
    ///
    /// # Errors
    ///
    /// The first refusal: see [`StreamTransformer::try_transform`].
    pub fn try_transform(&mut self, event: Event) -> Result<Vec<Event>> {
        self.pass(event, |transformer, event| transformer.try_transform(event))
    }

    /// Feeds each transformer what the one before it produced.
    fn pass<E>(
        &mut self,
        event: Event,
        mut step: impl FnMut(&mut dyn StreamTransformer, Event) -> Result<Vec<Event>, E>,
    ) -> Result<Vec<Event>, E> {
        let mut current = vec![event];
        for transformer in &mut self.transformers {
            let mut next = Vec::with_capacity(current.len());
            for event in current.drain(..) {
                next.extend(step(transformer.as_mut(), event)?);
            }
            current = next;
            if current.is_empty() {
                break;
            }
        }
        Ok(current)
    }
}

impl StreamTransformer for TransformerChain {
    fn transform(&mut self, event: Event) -> Vec<Event> {
        Self::transform(self, event)
    }

    fn try_transform(&mut self, event: Event) -> Result<Vec<Event>> {
        Self::try_transform(self, event)
    }

    /// Every transformer in the chain learns the history as the request
    /// carries it: what one transformer would do to the history is a rewrite
    /// of the stream, and the history is not part of the stream.
    fn seed(&mut self, history: &[Message]) {
        for transformer in &mut self.transformers {
            transformer.seed(history);
        }
    }

    /// Every transformer in the chain is shown the same events: what was
    /// sent is the output of the whole chain, whichever of its transformers
    /// shaped it.
    fn replay(&mut self, sent: &[Event]) {
        for transformer in &mut self.transformers {
            transformer.replay(sent);
        }
    }
}

/// Which side of the list passes.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
enum FilterMode {
    Allow,
    Deny,
}

/// What a filter makes of a tool call id.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Verdict {
    /// The call passes: what belongs to it goes out.
    Pass,
    /// The call is filtered: what belongs to it is dropped.
    Drop,
    /// Nothing has named the tool this id calls.
    Unknown,
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
///
/// # A conversation the stream restates
///
/// A `MESSAGES_SNAPSHOT`, and the request echoed on `RUN_STARTED`, carry
/// tool calls too, and are filtered by the same names. A filtered call is
/// taken out of the assistant message that made it, with the encrypted value
/// it carries, and the tool message that answers it is taken out of the
/// list. So is:
///
/// - an assistant message that is left with neither content nor calls. It
///   was there to carry the call, and a client that read the run's events
///   never held it: the events that would have made it were dropped.
///   Whatever else it carried goes with it;
/// - a tool message that answers a call nothing named. It has no tool name
///   of its own to judge it by.
///
/// The calls a restated conversation carries are judged as they are met,
/// like a `TOOL_CALL_START`, so a result that follows one goes where the
/// call went.
///
/// ```
/// # use ag_ui::{AssistantMessage, Event, Message, ToolCall};
/// # use ag_ui::server::{FilterToolCalls, StreamTransformer};
/// let restated = Event::messages_snapshot(vec![
///     Message::Assistant(AssistantMessage {
///         id: "msg-1".into(),
///         content: Some("Looking.".into()),
///         tool_calls: Some(vec![
///             ToolCall::new("c1", "search", "{}"),
///             ToolCall::new("c2", "secret_tool", "{}"),
///         ]),
///         ..Default::default()
///     }),
///     Message::tool("msg-2", "c2", "the result"),
/// ]);
///
/// let mut filter = FilterToolCalls::deny(["secret_tool"]);
/// let Some(Event::MessagesSnapshot(snapshot)) = filter.transform(restated).pop() else {
///     unreachable!("the snapshot is kept");
/// };
/// let [Message::Assistant(assistant)] = snapshot.messages.as_slice() else {
///     unreachable!("the tool message went with its call");
/// };
/// assert_eq!(assistant.tool_calls.as_ref().unwrap().len(), 1);
/// ```
///
/// # A call the filter did not see start
///
/// A call is judged by its name, and only what opens a call names it. The
/// filter learns the names from the run's events, from the request's history
/// ([`seed`](StreamTransformer::seed)), from a restated conversation, and in
/// a run that another driver began from the events already sent
/// ([`replay`](StreamTransformer::replay)).
///
/// An event of a call none of these named cannot be judged: arguments, an
/// end, a result, an encrypted value, a chunk that names no tool. The filter
/// refuses it. [`try_transform`](StreamTransformer::try_transform), which a
/// hosted run calls, fails with [`Rule::UnknownId`], or with
/// [`Rule::MalformedChunk`] for a chunk, so the emit fails and the agent is
/// told; [`transform`](StreamTransformer::transform) drops the event.
///
/// It is not let through, because the filter exists to keep a tool off the
/// wire and that cannot depend on the `verify` feature. One case is a call
/// that was filtered where the filter cannot see it: by the filter of an
/// earlier run, so that the client's history never held it, or by the
/// earlier driver of this run, so that the events sent do not. With the
/// ordering checks on, the arguments, the end and the result of such a call
/// would be rejected after the filter in any case, as those of a call that
/// never started. Without them they would be sent. And it is not dropped
/// with an `Ok`, because the other case is a call the filter has nothing
/// against, whose agent would go on as if the event had been sent.
///
/// In a run that continues another driver's, name the call on every chunk:
/// the filter knows the calls that were sent, not the chunk streams the
/// earlier agent had open, so it refuses a chunk that names no id.
#[derive(Clone, Debug)]
pub struct FilterToolCalls {
    mode: FilterMode,
    names: HashSet<String>,
    /// Every named call's verdict survives its end for a later result.
    dropped: HashSet<ToolCallId>,
    call_names: HashMap<ToolCallId, String>,
    /// The producer's open streams, including calls removed from the output.
    /// A bare chunk must be resolved here, before filtering can make it look
    /// like a continuation of a different call to the consumer.
    streams: Streams,
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
            call_names: HashMap::new(),
            streams: Streams::default(),
        }
    }

    fn passes(&self, name: &str) -> bool {
        match self.mode {
            FilterMode::Allow => self.names.contains(name),
            FilterMode::Deny => !self.names.contains(name),
        }
    }

    /// Records what names a call, and gives the verdict on the call.
    fn judge(&mut self, id: &ToolCallId, name: &str) -> Verdict {
        match self.call_names.get(id) {
            // The consumer keeps the name from the first chunk with this id.
            // A later name cannot reclassify its arguments as another tool.
            Some(original) => {
                if original != name {
                    self.dropped.insert(id.clone());
                }
            }
            None => {
                self.call_names.insert(id.clone(), name.to_owned());
                if !self.passes(name) {
                    self.dropped.insert(id.clone());
                }
            }
        }
        self.verdict(id)
    }

    fn verdict(&self, id: &ToolCallId) -> Verdict {
        if self.dropped.contains(id) {
            Verdict::Drop
        } else if self.call_names.contains_key(id) {
            Verdict::Pass
        } else {
            Verdict::Unknown
        }
    }

    /// Judges every call a restated conversation carries.
    fn learn(&mut self, messages: &[Message]) {
        for message in messages {
            if let Message::Assistant(assistant) = message {
                for call in assistant.tool_calls.iter().flatten() {
                    self.judge(&call.id, &call.function.name);
                }
            }
        }
    }

    /// Takes the filtered calls out of a restated conversation, and what
    /// only made sense with them. The type docs say what goes and why.
    fn prune(&self, messages: &mut Vec<Message>) {
        messages.retain_mut(|message| match message {
            Message::Assistant(assistant) => {
                let Some(calls) = &mut assistant.tool_calls else {
                    return true;
                };
                let carried = calls.len();
                calls.retain(|call| self.verdict(&call.id) == Verdict::Pass);
                // Untouched, or still carrying a call that passes.
                if calls.len() == carried || !calls.is_empty() {
                    return true;
                }
                // Every call it carried was filtered. It stays for what it
                // says, if it says anything.
                assistant.tool_calls = None;
                assistant
                    .content
                    .as_deref()
                    .is_some_and(|content| !content.is_empty())
            }
            Message::Tool(tool) => self.verdict(&tool.tool_call_id) == Verdict::Pass,
            _ => true,
        });
    }

    /// Whether an event is dropped, or why it cannot be judged: it belongs
    /// to a call nothing named.
    fn judge_event(&mut self, event: &Event) -> Result<bool, VerificationError> {
        let at = event.event_type();
        let decided = |verdict: Verdict, id: &ToolCallId| match verdict {
            Verdict::Pass => Ok(false),
            Verdict::Drop => Ok(true),
            Verdict::Unknown => Err(Self::unknown_call(at, id)),
        };
        match event {
            Event::ToolCallStart(e) => {
                Ok(self.judge(&e.tool_call_id, &e.tool_call_name) == Verdict::Drop)
            }
            Event::ToolCallChunk(e) => match (&e.tool_call_id, &e.tool_call_name) {
                (Some(id), Some(name)) => Ok(self.judge(id, name) == Verdict::Drop),
                // It continues a call, or opens one without naming its tool.
                (Some(id), None) => match self.verdict(id) {
                    Verdict::Unknown => {
                        let id = id.as_str().to_owned();
                        Err(Self::unplaced_chunk(&ChunkError::Unnamed { id }))
                    }
                    verdict => decided(verdict, id),
                },
                (None, name) => {
                    let stream = self
                        .streams
                        .continued(Kind::Tool, &e.subagent_run_id)
                        .map_err(|error| Self::unplaced_chunk(&error))?;
                    let id = ToolCallId::new(&stream.id);
                    let renamed = name.as_ref().is_some_and(|name| {
                        self.call_names
                            .get(&id)
                            .is_none_or(|original| original != name)
                    });
                    if renamed {
                        self.dropped.insert(id.clone());
                    }
                    decided(self.verdict(&id), &id)
                }
            },
            Event::ToolCallArgs(e) => decided(self.verdict(&e.tool_call_id), &e.tool_call_id),
            Event::ToolCallEnd(e) => decided(self.verdict(&e.tool_call_id), &e.tool_call_id),
            Event::ToolCallResult(e) => decided(self.verdict(&e.tool_call_id), &e.tool_call_id),
            Event::ReasoningEncryptedValue(e)
                if e.subtype == crate::ReasoningEncryptedValueSubtype::ToolCall =>
            {
                let id = ToolCallId::new(&e.entity_id);
                decided(self.verdict(&id), &id)
            }
            Event::MessagesSnapshot(e) => {
                self.learn(&e.messages);
                Ok(false)
            }
            Event::RunStarted(e) => {
                if let Some(input) = &e.input {
                    self.learn(&input.messages);
                }
                Ok(false)
            }
            _ => Ok(false),
        }
    }

    /// Follows an event that was judged, and sends it on unless its call
    /// is filtered.
    fn forward(&mut self, mut event: Event, dropped: bool) -> Vec<Event> {
        // Observe the unfiltered input. A removed start still changes where
        // the producer's later ID-less chunks belong.
        self.streams.track(&event);

        if dropped {
            return Vec::new();
        }
        match &mut event {
            Event::MessagesSnapshot(snapshot) => self.prune(&mut snapshot.messages),
            Event::RunStarted(started) => {
                if let Some(input) = &mut started.input {
                    self.prune(&mut input.messages);
                }
            }
            _ => {}
        }
        vec![event]
    }

    /// The refusal of an event that names a call nothing named a tool for.
    fn unknown_call(event: EventType, id: &ToolCallId) -> VerificationError {
        let detail = format!(
            "tool call {:?} was never started where `FilterToolCalls` could see it: not in this \
             run, the events already sent or the request's history. Without the tool's name it \
             cannot tell whether the call is filtered, so the event is not sent",
            id.as_str()
        );
        VerificationError::new(event, Rule::UnknownId, detail)
    }

    /// The refusal of a chunk that belongs to no call the filter can name.
    fn unplaced_chunk(error: &ChunkError) -> VerificationError {
        let detail = format!(
            "{error}. `FilterToolCalls` cannot tell which call the chunk belongs to, or whether \
             that call is filtered, so the chunk is not sent"
        );
        VerificationError::new(EventType::ToolCallChunk, Rule::MalformedChunk, detail)
    }
}

impl StreamTransformer for FilterToolCalls {
    /// A call an earlier run made is judged by its name like a new one, so the
    /// result a resumed run reports for it goes the way the call went.
    fn seed(&mut self, history: &[Message]) {
        self.learn(history);
    }

    /// A call the earlier driver of the run opened is judged by its name
    /// like a new one, so a continuation can finish and answer it.
    ///
    /// The chunk streams the events sent left open are not taken as the
    /// producer's: a call this filter's predecessor dropped may have been
    /// streaming in the same lane, and it is not among the events sent. So
    /// a chunk that names no id is refused until a chunk of this driver has
    /// opened a stream for it to continue.
    fn replay(&mut self, sent: &[Event]) {
        for event in sent {
            match event {
                Event::ToolCallStart(e) => {
                    self.judge(&e.tool_call_id, &e.tool_call_name);
                }
                Event::ToolCallChunk(e) => {
                    if let (Some(id), Some(name)) = (&e.tool_call_id, &e.tool_call_name) {
                        self.judge(id, name);
                    }
                }
                Event::MessagesSnapshot(e) => self.learn(&e.messages),
                Event::RunStarted(e) => {
                    if let Some(input) = &e.input {
                        self.learn(&input.messages);
                    }
                }
                _ => {}
            }
        }
    }

    /// Drops what [`try_transform`](Self::try_transform) refuses: an event
    /// of a call the filter never saw start.
    fn transform(&mut self, event: Event) -> Vec<Event> {
        match self.judge_event(&event) {
            Ok(dropped) => self.forward(event, dropped),
            // Nobody is told the event was not sent, so the producer goes on
            // as if it had been, and its streams are followed that way.
            Err(_) => {
                self.streams.track(&event);
                Vec::new()
            }
        }
    }

    /// A refused event changes nothing here. It was not sent and its emit
    /// said so, so the producer's streams are where they were.
    fn try_transform(&mut self, event: Event) -> Result<Vec<Event>> {
        let dropped = self.judge_event(&event)?;
        Ok(self.forward(event, dropped))
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
/// take down the run. For [`delta`](Self::delta) the expected shape is a JSON
/// Patch a consumer can read: an array of RFC 6902 operations whose `path`
/// and `from` are JSON Pointers. A patch that is well formed and does not
/// apply to the state is still promoted; a consumer reports that and keeps
/// the state it has.
///
/// ```
/// # use ag_ui::Event;
/// # use ag_ui::server::{StreamTransformer, ToolResultToState};
/// let mut promote = ToolResultToState::snapshot("load_document").replacing();
/// promote.transform(Event::tool_call_start("c1", "load_document"));
/// let out = promote.transform(Event::tool_call_result("m1", "c1", r#"{"title":"Notes"}"#));
/// assert_eq!(out, vec![Event::state_snapshot(serde_json::json!({"title": "Notes"}))]);
/// ```
///
/// The call need not have started in this run. One the request's history
/// holds unanswered, one a restated conversation carries, and in a run that
/// another driver began one the events already sent opened, are all calls
/// whose result is promoted.
#[derive(Clone, Debug)]
pub struct ToolResultToState {
    tool_name: String,
    form: StateForm,
    keep_result: bool,
    /// The calls to the tool that have no result yet.
    awaiting: HashSet<ToolCallId>,
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
            awaiting: HashSet::new(),
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
            // A patch a consumer could not read is no more the expected
            // shape than content that is not JSON.
            StateForm::Delta => serde_json::from_str::<Vec<PatchOperation>>(content)
                .ok()
                .filter(|patch| crate::server::state::well_formed(patch).is_ok())
                .map(Event::state_delta),
        }
    }

    /// Learns the calls to the tool that a conversation holds unanswered.
    fn learn(&mut self, messages: &[Message]) {
        for message in messages {
            if let Message::Assistant(assistant) = message {
                for call in assistant.tool_calls.iter().flatten() {
                    if call.function.name == self.tool_name {
                        self.awaiting.insert(call.id.clone());
                    }
                }
            }
        }
        // A call the conversation answers is not waiting for a result.
        for message in messages {
            if let Message::Tool(tool) = message {
                self.awaiting.remove(&tool.tool_call_id);
            }
        }
    }

    /// Learns from an event which calls are to the tool.
    fn note(&mut self, event: &Event) {
        match event {
            Event::ToolCallStart(e) if e.tool_call_name == self.tool_name => {
                self.awaiting.insert(e.tool_call_id.clone());
            }
            Event::ToolCallChunk(e) => {
                if let (Some(id), Some(name)) = (&e.tool_call_id, &e.tool_call_name) {
                    if name == &self.tool_name {
                        self.awaiting.insert(id.clone());
                    }
                }
            }
            Event::MessagesSnapshot(e) => self.learn(&e.messages),
            Event::RunStarted(e) => {
                if let Some(input) = &e.input {
                    self.learn(&input.messages);
                }
            }
            _ => {}
        }
    }
}

impl StreamTransformer for ToolResultToState {
    /// A call to the tool that an earlier run left unanswered is one whose
    /// result a resumed run reports, and it is promoted like any other.
    fn seed(&mut self, history: &[Message]) {
        self.learn(history);
    }

    /// The same for a call the earlier driver of the run opened.
    fn replay(&mut self, sent: &[Event]) {
        for event in sent {
            self.note(event);
            if let Event::ToolCallResult(result) = event {
                self.awaiting.remove(&result.tool_call_id);
            }
        }
    }

    fn transform(&mut self, event: Event) -> Vec<Event> {
        self.note(&event);
        let Event::ToolCallResult(payload) = &event else {
            return vec![event];
        };
        if !self.awaiting.remove(&payload.tool_call_id) {
            return vec![event];
        }
        let promoted = payload
            .content
            .as_text()
            .and_then(|text| self.state_event(text));
        let Some(mut state) = promoted else {
            return vec![event];
        };
        // Provenance travels with the state: a subagent's result promoted
        // is the subagent's publish.
        if let Some(id) = &payload.subagent_run_id {
            state.set_subagent_run_id(id.clone());
        }
        if self.keep_result {
            vec![event, state]
        } else {
            vec![state]
        }
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
/// `ctx.subagent(..)` meant it, and silently flattening that output would
/// surprise the caller. Flip it per endpoint when
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
///
/// The list of modes is `#[non_exhaustive]`: another way to show subagents
/// to a consumer can be added without a breaking release.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum SubagentVisibility {
    /// The full surface: the lifecycle events, and `subagentRunId` on
    /// everything a subagent produced.
    #[default]
    Attributed,
    /// The pre-subagent shape: no lifecycle events and no `subagentRunId`
    /// anywhere — not on events, not on the messages inside
    /// `MESSAGES_SNAPSHOT` or the `RUN_STARTED` input echo, not on the
    /// interrupts a paused run reports. A subagent's own text arrives as the
    /// parent's work. A `*_CHUNK` event that named no id is given the one its
    /// attribution resolved to, since without the attribution the consumer
    /// would resolve it differently.
    ///
    /// A subagent's steps are dropped rather than flattened. A step brackets
    /// its own agent's graph, and as the parent's it would either collide
    /// with an open step of the same name — the common shape: a parent
    /// `tools` step wrapping the delegation and the child's own `tools`
    /// inside it — or misdescribe the parent's graph. The lifecycle events
    /// were the child's structure; so are its steps.
    Inline,
    /// Only the parent's own events. Everything a subagent produced is
    /// dropped — including the result of a call it requested, even when the
    /// parent executed it, since a result for a call the consumer never saw
    /// is a protocol error. The converse holds too: a result answering the
    /// *parent's* call is kept, untagged, whoever executed it — the consumer
    /// saw the call, and a call left unanswered in the history is what the
    /// next request would carry back.
    ///
    /// The one thing kept is the run's shared state. A `STATE_SNAPSHOT` or
    /// `STATE_DELTA` a subagent published describes the *thread's* state,
    /// not the subagent's work: a client that never saw it would mirror a
    /// stale state and send that back on its next request. It goes out with
    /// the tag cleared, as the parent's.
    Hidden,
}

impl SubagentVisibility {
    /// The transformer for this mode. [`Attributed`](Self::Attributed) passes
    /// events through unchanged.
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
/// Both modes keep the model of the stream a consumer expands chunks by — one
/// chunk stream per owner, ended by what AG-UI 1.0 says ends it — because a
/// `*_CHUNK` event that names no id is resolved *through its attribution* on
/// the consuming side, and stripping the attribution would send it to the
/// wrong stream. Such a chunk is given the id it resolves to before its tag
/// goes.
///
/// They keep that model twice: once for the stream the agent emits, and once
/// for the stream the filter sends, which is the one a consumer follows. The
/// two differ, because an event that leaves its subagent's lane ends a chunk
/// stream in the lane it arrives in. Two subagents that stream at once take
/// turns ending each other's stream once [`Inline`](SubagentVisibility::Inline)
/// has put both in the parent's lane, and the state a subagent publishes,
/// which [`Hidden`](SubagentVisibility::Hidden) keeps as the parent's, ends
/// the parent's. The agent's stream is still open then, so its next chunk
/// says only what a continuation says, and on the consuming side that chunk
/// opens the item again. It is given what opens it, from what the agent
/// opened the stream with: `toolCallName` and `parentMessageId` for a tool
/// call, `role` and `name` for a message. A chunk whose stream the consumer
/// still holds is sent as it is.
///
/// [`Hidden`](SubagentVisibility::Hidden) additionally remembers what each
/// subagent owns — messages, tool calls, activities — for the rest of the
/// run, not only while they are open, so an untagged continuation, re-open,
/// patch or result for a subagent's entity is dropped with the rest of it
/// rather than leaking into the parent's stream. That is the owner-aware
/// verifier's reading too: the first writer owns the id, and an absent tag
/// agrees with any owner. An entity the consumer has seen opened keeps that
/// visibility until it closes, whatever a snapshot says about its owner
/// meanwhile — a consumer must never be left with a message it saw opened
/// and never sees closed.
#[derive(Debug)]
pub struct SubagentFilter {
    mode: SubagentVisibility,
    /// The chunk streams the agent has open, per owner: what it emitted.
    streams: Streams,
    /// The chunk streams a consumer has open: what this filter sent.
    output: Streams,
    /// Text message ids a subagent owns, and which.
    hidden_text: HashMap<MessageId, SubagentRunId>,
    /// Reasoning ids — the block and the message inside it — a subagent
    /// owns. A bucket of its own, as the verifiers keep it, so a producer
    /// that reuses an id across the two kinds is not misread.
    hidden_reasoning: HashMap<MessageId, SubagentRunId>,
    /// Tool call ids a subagent owns — or that sit in a message it owns.
    hidden_tool_calls: HashMap<ToolCallId, SubagentRunId>,
    /// Every activity seen, and whether a subagent owns it.
    activities: HashMap<MessageId, bool>,
    /// What is open, and whether the consumer saw it open.
    open_visibility: HashMap<(Kind, String), bool>,
    /// Whether the consumer saw each tool call start, for the result that
    /// may arrive long after the call closed.
    call_visibility: HashMap<ToolCallId, bool>,
}

/// What a chunk naming no id turned out to be.
enum Bare {
    /// Not such a chunk.
    No,
    /// Resolved to a stream, and given its id.
    Resolved {
        owner: Option<SubagentRunId>,
        kind: Kind,
        id: String,
    },
    /// Nothing to resolve it against; the consumer will say so.
    Unresolved,
}

impl SubagentFilter {
    /// A filter for `mode`.
    pub fn new(mode: SubagentVisibility) -> Self {
        Self {
            mode,
            streams: Streams::default(),
            output: Streams::default(),
            hidden_text: HashMap::new(),
            hidden_reasoning: HashMap::new(),
            hidden_tool_calls: HashMap::new(),
            activities: HashMap::new(),
            open_visibility: HashMap::new(),
            call_visibility: HashMap::new(),
        }
    }

    /// The mode this filter applies.
    pub fn mode(&self) -> SubagentVisibility {
        self.mode
    }

    /// The stream a stream-bearing event belongs to, if it names one.
    fn entity(event: &Event) -> Option<(Kind, &str)> {
        Some(match event {
            Event::TextMessageStart(e) => (Kind::Text, e.message_id.as_str()),
            Event::TextMessageContent(e) => (Kind::Text, e.message_id.as_str()),
            Event::TextMessageEnd(e) => (Kind::Text, e.message_id.as_str()),
            Event::TextMessageChunk(e) => (Kind::Text, e.message_id.as_ref()?.as_str()),
            Event::ReasoningStart(e) => (Kind::Reasoning, e.message_id.as_str()),
            Event::ReasoningMessageStart(e) => (Kind::Reasoning, e.message_id.as_str()),
            Event::ReasoningMessageContent(e) => (Kind::Reasoning, e.message_id.as_str()),
            Event::ReasoningMessageEnd(e) => (Kind::Reasoning, e.message_id.as_str()),
            Event::ReasoningEnd(e) => (Kind::Reasoning, e.message_id.as_str()),
            Event::ReasoningMessageChunk(e) => (Kind::Reasoning, e.message_id.as_ref()?.as_str()),
            Event::ToolCallStart(e) => (Kind::Tool, e.tool_call_id.as_str()),
            Event::ToolCallArgs(e) => (Kind::Tool, e.tool_call_id.as_str()),
            Event::ToolCallEnd(e) => (Kind::Tool, e.tool_call_id.as_str()),
            Event::ToolCallResult(e) => (Kind::Tool, e.tool_call_id.as_str()),
            Event::ToolCallChunk(e) => (Kind::Tool, e.tool_call_id.as_ref()?.as_str()),
            _ => return None,
        })
    }

    /// The message a tool call sits in, when the event says.
    fn carrier(event: &Event) -> Option<&MessageId> {
        match event {
            Event::ToolCallStart(e) => e.parent_message_id.as_ref(),
            Event::ToolCallChunk(e) => e.parent_message_id.as_ref(),
            _ => None,
        }
    }

    /// Whose an event is: its tag; else the recorded owner of its entity —
    /// for a tool call, the owner of the message that carries it; else
    /// whoever has that entity open; else the parent.
    fn owner_for(&self, event: &Event, tag: &Option<SubagentRunId>) -> Option<SubagentRunId> {
        if tag.is_some() {
            return tag.clone();
        }
        let (kind, id) = Self::entity(event)?;
        let recorded = match kind {
            Kind::Text => self.hidden_text.get(&MessageId::new(id)).cloned(),
            Kind::Reasoning => self.hidden_reasoning.get(&MessageId::new(id)).cloned(),
            Kind::Tool => Self::carrier(event)
                .and_then(|parent| self.hidden_text.get(parent).cloned())
                .or_else(|| self.hidden_tool_calls.get(&ToolCallId::new(id)).cloned()),
        };
        // Untagged, it continues whatever chunk stream is assembling its id.
        recorded.or_else(|| {
            let holder = self.streams.holder(kind, id)?;
            holder.owner.clone()
        })
    }

    /// Gives a chunk naming no id the id its attribution resolves to on the
    /// consuming side, so that stripping the attribution afterwards cannot
    /// send it to another stream.
    fn fill_bare(&self, event: &mut Event, tag: &Option<SubagentRunId>) -> Bare {
        let kind = match event {
            Event::TextMessageChunk(e) if e.message_id.is_none() => Kind::Text,
            Event::ReasoningMessageChunk(e) if e.message_id.is_none() => Kind::Reasoning,
            Event::ToolCallChunk(e) if e.tool_call_id.is_none() => Kind::Tool,
            _ => return Bare::No,
        };
        let Ok(stream) = self.streams.continued(kind, tag) else {
            return Bare::Unresolved;
        };
        let (owner, id) = (stream.owner.clone(), stream.id.clone());
        match event {
            Event::TextMessageChunk(e) => e.message_id = Some(MessageId::new(&id)),
            Event::ReasoningMessageChunk(e) => e.message_id = Some(MessageId::new(&id)),
            Event::ToolCallChunk(e) => e.tool_call_id = Some(ToolCallId::new(&id)),
            _ => unreachable!("matched above"),
        }
        Bare::Resolved { owner, kind, id }
    }

    /// What the agent opened the stream a chunk continues with.
    ///
    /// Read once the chunk has its id and before it is tracked. `None` for
    /// a chunk that continues nothing the agent has open: it opens its own
    /// stream, and carries what opens it or is malformed as emitted.
    fn opener(&self, event: &Event, tag: &Option<SubagentRunId>) -> Option<Opener> {
        let (kind, id) = chunk_item(event)?;
        assembling(&self.streams, kind, id, tag).map(|stream| stream.opener.clone())
    }

    /// Sends an event on, as the last thing either mode does with it.
    ///
    /// `opener` is what [`opener`](Self::opener) read for it. A chunk that
    /// continues a stream the consumer no longer holds is given the fields
    /// that open its item there, and the consumer's streams are followed
    /// with what is sent.
    fn send(&mut self, mut event: Event, opener: Option<Opener>) -> Vec<Event> {
        if let Some(opener) = opener {
            self.reopen(&mut event, opener);
        }
        self.output.track(&event);
        vec![event]
    }

    /// Fills in what opens a chunk's item, unless the consumer has the
    /// chunk's stream open and so reads the chunk as a continuation.
    fn reopen(&self, event: &mut Event, opener: Opener) {
        let tag = event.subagent_run_id().cloned();
        let continues = chunk_item(event)
            .is_some_and(|(kind, id)| assembling(&self.output, kind, id, &tag).is_some());
        if continues {
            return;
        }
        match (event, opener) {
            (Event::TextMessageChunk(chunk), Opener::Text { role, name }) => {
                chunk.role.get_or_insert(role);
                if chunk.name.is_none() {
                    chunk.name = name;
                }
            }
            (
                Event::ToolCallChunk(chunk),
                Opener::Tool {
                    name,
                    parent_message_id,
                },
            ) => {
                chunk.tool_call_name.get_or_insert(name);
                if chunk.parent_message_id.is_none() {
                    chunk.parent_message_id = parent_message_id;
                }
            }
            // A reasoning message is opened by its id alone.
            _ => {}
        }
    }

    /// Strips the subagent surface from an event, or drops it entirely when
    /// it *is* the subagent surface — the lifecycle, and a subagent's steps.
    fn inline(&mut self, mut event: Event) -> Vec<Event> {
        let tag = event.subagent_run_id().cloned();
        // Filled before it is tracked: with its id a chunk continues the
        // stream it resolved to, which is what the consumer will see.
        self.fill_bare(&mut event, &tag);
        let opener = self.opener(&event, &tag);
        self.streams.track(&event);
        let subagents_step =
            matches!(event, Event::StepStarted(_) | Event::StepFinished(_)) && tag.is_some();
        if subagents_step {
            return Vec::new();
        }
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
        self.send(event, opener)
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

    /// Keeps the parent's events and drops a subagent's, remembering what
    /// each subagent owns so that an untagged event is judged by its opener.
    fn hidden(&mut self, mut event: Event) -> Vec<Event> {
        let tag = event.subagent_run_id().cloned();
        let owned = tag.is_some();
        let bare = self.fill_bare(&mut event, &tag);
        let opener = self.opener(&event, &tag);
        let keep = match bare {
            // The chunk continues a stream: it goes where the stream went.
            Bare::Resolved { owner, kind, id } => self
                .open_visibility
                .get(&(kind, id))
                .copied()
                .unwrap_or(owner.is_none()),
            Bare::Unresolved => !owned,
            Bare::No => {
                // Before the event is tracked: it may end the stream that
                // says whose it is.
                let owner = self.owner_for(&event, &tag);
                self.streams.track(&event);
                self.judge(&event, &tag, owner.as_ref())
            }
        };
        if !keep {
            return Vec::new();
        }
        match &mut event {
            // Authoritative: the snapshot restates the conversation, so what
            // it carries is re-read — and what it does not carry is left as
            // the run established it, as the verifiers leave it.
            Event::MessagesSnapshot(snapshot) => {
                self.seed_hidden(&snapshot.messages, true);
                let hidden_calls = &self.hidden_tool_calls;
                snapshot
                    .messages
                    .retain_mut(|message| Self::show_message(message, hidden_calls));
            }
            // History, not a rewrite: what it carries is remembered alongside
            // what the run has already shown.
            Event::RunStarted(started) => {
                if let Some(input) = &mut started.input {
                    self.seed_hidden(&input.messages, false);
                    let hidden_calls = &self.hidden_tool_calls;
                    input
                        .messages
                        .retain_mut(|message| Self::show_message(message, hidden_calls));
                }
            }
            Event::StateSnapshot(_)
            | Event::StateDelta(_)
            | Event::ToolCallResult(_)
            | Event::ActivitySnapshot(_)
            | Event::ActivityDelta(_) => {
                event.clear_subagent_run_id();
            }
            Event::RunFinished(finished) => Self::strip_interrupt_tags(finished),
            _ => {}
        }
        self.send(event, opener)
    }

    /// Whether the consumer sees an event with an id, by who owns it.
    fn judge(
        &mut self,
        event: &Event,
        tag: &Option<SubagentRunId>,
        owner: Option<&SubagentRunId>,
    ) -> bool {
        let owned = tag.is_some();
        match event {
            Event::SubagentStarted(_) | Event::SubagentFinished(_) | Event::SubagentError(_) => {
                false
            }

            Event::TextMessageStart(e) => self.opened(Kind::Text, &e.message_id, owner),
            Event::TextMessageChunk(e) => match &e.message_id {
                Some(id) => self.opened(Kind::Text, id, owner),
                None => unreachable!("a bare chunk was resolved or passed through"),
            },
            Event::TextMessageContent(e) => self.continued(Kind::Text, &e.message_id, owned),
            Event::TextMessageEnd(e) => self.closed(Kind::Text, &e.message_id, owned),

            Event::ReasoningStart(e) => self.opened(Kind::Reasoning, &e.message_id, owner),
            Event::ReasoningMessageStart(e) => self.opened(Kind::Reasoning, &e.message_id, owner),
            Event::ReasoningMessageChunk(e) => match &e.message_id {
                Some(id) => self.opened(Kind::Reasoning, id, owner),
                None => unreachable!("a bare chunk was resolved or passed through"),
            },
            Event::ReasoningMessageContent(e) => {
                self.continued(Kind::Reasoning, &e.message_id, owned)
            }
            Event::ReasoningMessageEnd(e) => self.closed(Kind::Reasoning, &e.message_id, owned),
            Event::ReasoningEnd(e) => self.closed(Kind::Reasoning, &e.message_id, owned),

            Event::ToolCallStart(e) => self.call_opened(&e.tool_call_id, owner),
            Event::ToolCallChunk(e) => match &e.tool_call_id {
                Some(id) => self.call_opened(id, owner),
                None => unreachable!("a bare chunk was resolved or passed through"),
            },
            Event::ToolCallArgs(e) => self.call_continued(&e.tool_call_id, owned),
            Event::ToolCallEnd(e) => {
                let keep = self.call_continued(&e.tool_call_id, owned);
                self.open_visibility
                    .remove(&(Kind::Tool, e.tool_call_id.as_str().to_owned()));
                keep
            }
            // A result goes where its call went, whoever executed it: one for
            // a call the consumer never saw is a protocol error, and one for
            // a call it did see is owed.
            Event::ToolCallResult(e) => self
                .call_visibility
                .get(&e.tool_call_id)
                .copied()
                .unwrap_or_else(|| !self.hidden_tool_calls.contains_key(&e.tool_call_id)),

            // An activity is owned by the snapshot that minted it, and only a
            // replacing snapshot re-mints it — the verifiers' rule. A merge
            // into a visible activity is kept whoever wrote it, as a result
            // for a visible call is: the entity is the consumer's to keep
            // whole.
            Event::ActivitySnapshot(e) => {
                let existing = self.activities.get(&e.message_id).copied();
                let hidden = match existing {
                    Some(hidden) if !e.replace => hidden,
                    _ => owned,
                };
                self.activities.insert(e.message_id.clone(), hidden);
                !hidden
            }
            Event::ActivityDelta(e) => {
                !self.activities.get(&e.message_id).copied().unwrap_or(false)
            }

            // An opaque blob for an entity the consumer never saw goes with
            // the entity, as `FilterToolCalls` drops one for a dropped call.
            Event::ReasoningEncryptedValue(e) => {
                !owned
                    && match e.subtype {
                        crate::ReasoningEncryptedValueSubtype::ToolCall => !self
                            .hidden_tool_calls
                            .contains_key(&ToolCallId::new(e.entity_id.clone())),
                        crate::ReasoningEncryptedValueSubtype::Message => {
                            let id = MessageId::new(e.entity_id.clone());
                            !self.hidden_text.contains_key(&id)
                                && !self.hidden_reasoning.contains_key(&id)
                        }
                    }
            }

            // The thread's state, whoever published it.
            Event::StateSnapshot(_) | Event::StateDelta(_) => true,

            Event::RunFinished(_) | Event::RunError(_) => {
                self.open_visibility.clear();
                true
            }

            _ => !owned,
        }
    }

    fn owners_mut(&mut self, kind: Kind) -> &mut HashMap<MessageId, SubagentRunId> {
        match kind {
            Kind::Text => &mut self.hidden_text,
            Kind::Reasoning => &mut self.hidden_reasoning,
            Kind::Tool => unreachable!("tool calls have their own map"),
        }
    }

    fn is_hidden(&self, kind: Kind, id: &str) -> bool {
        match kind {
            Kind::Text => self.hidden_text.contains_key(&MessageId::new(id)),
            Kind::Reasoning => self.hidden_reasoning.contains_key(&MessageId::new(id)),
            Kind::Tool => self.hidden_tool_calls.contains_key(&ToolCallId::new(id)),
        }
    }

    /// A message opens under `owner` — the first writer keeps it — and the
    /// consumer sees it iff the owner is the parent, unless it is already
    /// open, in which case it keeps the visibility it was opened with.
    fn opened(&mut self, kind: Kind, id: &MessageId, owner: Option<&SubagentRunId>) -> bool {
        if let Some(owner) = owner {
            self.owners_mut(kind)
                .entry(id.clone())
                .or_insert_with(|| owner.clone());
        }
        *self
            .open_visibility
            .entry((kind, id.as_str().to_owned()))
            .or_insert(owner.is_none())
    }

    fn continued(&self, kind: Kind, id: &MessageId, owned: bool) -> bool {
        !owned
            && self
                .open_visibility
                .get(&(kind, id.as_str().to_owned()))
                .copied()
                .unwrap_or_else(|| !self.is_hidden(kind, id.as_str()))
    }

    fn closed(&mut self, kind: Kind, id: &MessageId, owned: bool) -> bool {
        let keep = self.continued(kind, id, owned);
        self.open_visibility.remove(&(kind, id.as_str().to_owned()));
        keep
    }

    fn call_opened(&mut self, id: &ToolCallId, owner: Option<&SubagentRunId>) -> bool {
        if let Some(owner) = owner {
            self.hidden_tool_calls
                .entry(id.clone())
                .or_insert_with(|| owner.clone());
        }
        let visible = *self
            .open_visibility
            .entry((Kind::Tool, id.as_str().to_owned()))
            .or_insert(owner.is_none());
        self.call_visibility.insert(id.clone(), visible);
        visible
    }

    fn call_continued(&self, id: &ToolCallId, owned: bool) -> bool {
        !owned
            && self
                .open_visibility
                .get(&(Kind::Tool, id.as_str().to_owned()))
                .copied()
                .unwrap_or_else(|| !self.hidden_tool_calls.contains_key(id))
    }

    /// Re-reads what a replay says about ownership. Authoritatively — a
    /// `MESSAGES_SNAPSHOT` — an untagged message takes its id back for the
    /// parent; as history — the `RUN_STARTED` echo — only the subagents'
    /// messages are added. A tool message goes where its call went, so one
    /// answering a visible call is not hidden however it is tagged.
    fn seed_hidden(&mut self, messages: &[crate::Message], authoritative: bool) {
        for message in messages {
            let id = message.id().clone();
            if let crate::Message::Activity(_) = message {
                if authoritative || !self.activities.contains_key(&id) {
                    self.activities
                        .insert(id, message.subagent_run_id().is_some());
                }
                continue;
            }
            let owner = match message {
                crate::Message::Tool(tool) => {
                    self.hidden_tool_calls.get(&tool.tool_call_id).cloned()
                }
                _ => message.subagent_run_id().cloned(),
            };
            let calls: Vec<ToolCallId> = match message {
                crate::Message::Assistant(assistant) => assistant
                    .tool_calls
                    .iter()
                    .flatten()
                    .map(|call| call.id.clone())
                    .collect(),
                _ => Vec::new(),
            };
            let kind = match message {
                crate::Message::Reasoning(_) => Kind::Reasoning,
                _ => Kind::Text,
            };
            match owner {
                Some(owner) => {
                    for call in calls {
                        self.hidden_tool_calls.insert(call, owner.clone());
                    }
                    self.owners_mut(kind).insert(id, owner);
                }
                None if authoritative => {
                    self.owners_mut(kind).remove(&id);
                    for call in &calls {
                        self.hidden_tool_calls.remove(call);
                    }
                }
                None => {}
            }
        }
    }

    /// Whether a replayed message reaches the consumer, stripping the tag
    /// from the one kind that may carry one there: a tool message answering
    /// a call the consumer saw is the parent's, whoever executed it.
    fn show_message(
        message: &mut crate::Message,
        hidden_calls: &HashMap<ToolCallId, SubagentRunId>,
    ) -> bool {
        match message {
            crate::Message::Tool(tool) => {
                if hidden_calls.contains_key(&tool.tool_call_id) {
                    return false;
                }
                tool.subagent_run_id = None;
                true
            }
            _ => message.subagent_run_id().is_none(),
        }
    }
}

/// The chunk stream a chunk belongs to: its kind and the id it names.
fn chunk_item(event: &Event) -> Option<(Kind, &str)> {
    Some(match event {
        Event::TextMessageChunk(e) => (Kind::Text, e.message_id.as_ref()?.as_str()),
        Event::ReasoningMessageChunk(e) => (Kind::Reasoning, e.message_id.as_ref()?.as_str()),
        Event::ToolCallChunk(e) => (Kind::Tool, e.tool_call_id.as_ref()?.as_str()),
        _ => return None,
    })
}

/// The stream a chunk that names `id` continues, as the open-stream model
/// places it: the one assembling that id, which for an attributed chunk has
/// to be in the lane it is attributed to.
fn assembling<'a>(
    streams: &'a Streams,
    kind: Kind,
    id: &str,
    tag: &Option<SubagentRunId>,
) -> Option<&'a Stream> {
    streams
        .holder(kind, id)
        .filter(|stream| tag.is_none() || &stream.owner == tag)
}

impl StreamTransformer for SubagentFilter {
    /// History, as the `RUN_STARTED` echo would carry it: what a subagent
    /// owned in an earlier run stays hidden in this one, echo or not.
    fn seed(&mut self, history: &[Message]) {
        if self.mode == SubagentVisibility::Hidden {
            self.seed_hidden(history, false);
        }
    }

    /// What was sent is what a consumer holds, so the chunk streams it left
    /// open are the ones a consumer has open now.
    fn replay(&mut self, sent: &[Event]) {
        for event in sent {
            self.output.track(event);
        }
    }

    fn transform(&mut self, event: Event) -> Vec<Event> {
        match self.mode {
            SubagentVisibility::Attributed => vec![event],
            SubagentVisibility::Inline => self.inline(event),
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
    fn an_event_of_a_call_nothing_named_is_refused_and_transform_drops_it() {
        use crate::server::error::Error;

        let blob = Event::reasoning_encrypted_value(
            ReasoningEncryptedValueSubtype::ToolCall,
            "c1",
            "opaque",
        );
        let chunk = |id: Option<&str>| {
            Event::tool_call_chunk(id.map(ToolCallId::new), None, Some("{}".to_owned()))
        };
        let cases = [
            (Event::tool_call_args("c1", "{}"), Rule::UnknownId),
            (Event::tool_call_end("c1"), Rule::UnknownId),
            (Event::tool_call_result("m1", "c1", "done"), Rule::UnknownId),
            (blob, Rule::UnknownId),
            (chunk(Some("c1")), Rule::MalformedChunk),
            (chunk(None), Rule::MalformedChunk),
        ];
        for mut filter in [
            FilterToolCalls::deny(["nope"]),
            FilterToolCalls::allow(["search"]),
        ] {
            for (event, rule) in &cases {
                let refused = filter
                    .try_transform(event.clone())
                    .expect_err("the call was never named");
                let Error::Verification(error) = refused else {
                    panic!("{refused}");
                };
                assert_eq!((error.event, error.rule), (event.event_type(), *rule));
                assert!(error.detail.contains("FilterToolCalls"), "{error}");
                assert!(filter.transform(event.clone()).is_empty());
            }
            // Named, the same call is judged, and what follows goes with it.
            let start = Event::tool_call_start("c1", "search");
            assert_eq!(filter.try_transform(start.clone()).unwrap(), [start]);
            let args = Event::tool_call_args("c1", "{}");
            assert_eq!(filter.try_transform(args.clone()).unwrap(), [args]);
        }
    }

    #[test]
    fn a_refused_event_leaves_the_filter_as_it_was() {
        let open = Event::tool_call_chunk(Some("c1".into()), Some("search".into()), None);
        let bare = || Event::tool_call_chunk(None, None, Some("{}".into()));
        let unknown = Event::tool_call_args("c9", "{}");

        // The emit failed and said so. The event is not on the wire, and the
        // chunk stream of `c1` is where it was.
        let mut filter = FilterToolCalls::deny(["nope"]);
        filter
            .try_transform(open.clone())
            .expect("a call that passes");
        assert!(filter.try_transform(unknown.clone()).is_err());
        assert_eq!(filter.try_transform(bare()).unwrap(), [bare()]);

        // Dropped without a word, it is an event its producer takes as
        // sent: one that ended the chunk stream in its lane.
        let mut filter = FilterToolCalls::deny(["nope"]);
        filter.transform(open);
        assert!(filter.transform(unknown).is_empty());
        assert!(filter.transform(bare()).is_empty());
    }

    #[test]
    fn a_restated_conversation_loses_only_what_the_filter_is_about() {
        use crate::{AssistantMessage, ToolCall};

        let untouched = [
            // Neither content nor calls, and no filtered call was taken out
            // of it: it stays as it is.
            Message::Assistant(AssistantMessage {
                id: "empty".into(),
                ..Default::default()
            }),
            Message::Assistant(AssistantMessage {
                id: "no-calls".into(),
                tool_calls: Some(Vec::new()),
                ..Default::default()
            }),
            Message::Assistant(AssistantMessage {
                id: "public".into(),
                tool_calls: Some(vec![ToolCall::new("c-public", "search", "{}")]),
                ..Default::default()
            }),
            Message::tool("answer", "c-public", "1 hit"),
        ];
        let mut restated = untouched.to_vec();
        restated.extend([
            // Its content is all that is left of it.
            Message::Assistant(AssistantMessage {
                id: "says-and-calls".into(),
                content: Some("One moment.".into()),
                tool_calls: Some(vec![ToolCall::new("c-private", "nope", "{}")]),
                ..Default::default()
            }),
            // Nothing is left of it: an empty content is no content.
            Message::Assistant(AssistantMessage {
                id: "only-calls".into(),
                content: Some(String::new()),
                encrypted_value: Some("opaque".into()),
                tool_calls: Some(vec![ToolCall::new("c-private-2", "nope", "{}")]),
                ..Default::default()
            }),
            Message::tool("hidden-answer", "c-private", "everything"),
            // No call in the conversation, or before it, has this id.
            Message::tool("orphan", "c-unheard-of", "whose result is this?"),
        ]);

        let mut filter = FilterToolCalls::deny(["nope"]);
        let out = filter.transform(Event::messages_snapshot(restated));
        let [Event::MessagesSnapshot(snapshot)] = out.as_slice() else {
            panic!("the snapshot is kept: {out:?}");
        };
        let mut expected = untouched.to_vec();
        expected.push(Message::assistant("says-and-calls", "One moment."));
        assert_eq!(snapshot.messages, expected);

        // The calls the snapshot carried were judged as it passed.
        let answered = Event::tool_call_result("m1", "c-public", "again");
        assert_eq!(filter.try_transform(answered.clone()).unwrap(), [answered]);
        let hidden = Event::tool_call_result("m2", "c-private", "again");
        assert_eq!(filter.try_transform(hidden).unwrap(), []);
    }

    #[test]
    fn a_filter_learns_the_calls_that_were_sent_and_not_their_chunk_streams() {
        let sent = [
            Event::run_started("t", "r"),
            Event::tool_call_start("c1", "search"),
            Event::tool_call_chunk(Some("c2".into()), Some("search".into()), None),
        ];
        let mut filter = FilterToolCalls::allow(["search"]);
        filter.replay(&sent);

        let args = Event::tool_call_args("c1", "{}");
        assert_eq!(filter.try_transform(args.clone()).unwrap(), [args]);
        let named = Event::tool_call_chunk(Some("c2".into()), None, Some("{}".into()));
        assert_eq!(filter.try_transform(named.clone()).unwrap(), [named]);
        // A chunk that names no id would be placed by a stream the filter
        // did not watch being opened.
        let bare = Event::tool_call_chunk(None, None, Some("{}".into()));
        assert!(filter.try_transform(bare).is_err());
    }

    #[test]
    fn a_chain_stops_at_the_first_refusal() {
        let mut chain = TransformerChain::new()
            .with(ToolResultToState::snapshot("load"))
            .with(FilterToolCalls::deny(["nope"]));
        let unknown = Event::tool_call_args("c1", "{}");
        assert!(chain.try_transform(unknown.clone()).is_err());
        assert!(chain.transform(unknown).is_empty());
        let text = Event::text_message_start("m1", TextMessageRole::Assistant);
        assert_eq!(chain.try_transform(text.clone()).unwrap(), [text]);
    }

    #[test]
    fn a_call_the_history_answered_is_not_waiting_for_a_result() {
        use crate::{AssistantMessage, ToolCall};

        let history = [
            Message::Assistant(AssistantMessage {
                id: "a0".into(),
                tool_calls: Some(vec![
                    ToolCall::new("answered", "load", "{}"),
                    ToolCall::new("waiting", "load", "{}"),
                    ToolCall::new("another-tool", "search", "{}"),
                ]),
                ..Default::default()
            }),
            Message::tool("t0", "answered", r#"{"a":0}"#),
        ];
        let mut promote = ToolResultToState::snapshot("load");
        promote.seed(&history);
        for (call, promoted) in [
            ("answered", false),
            ("another-tool", false),
            ("waiting", true),
        ] {
            let result = Event::tool_call_result("m1", call, r#"{"a":1}"#);
            let out = promote.transform(result);
            assert_eq!(out.len(), if promoted { 2 } else { 1 }, "{call}");
        }
        // One result answers a call: a second is not promoted again.
        let again = Event::tool_call_result("m2", "waiting", r#"{"a":2}"#);
        assert_eq!(promote.transform(again).len(), 1);
    }

    #[test]
    fn a_patch_with_a_malformed_pointer_is_not_promoted() {
        let mut promote = ToolResultToState::delta("patch_state");
        for (call, content) in [
            ("c1", r#"[{"op":"add","path":"oops","value":1}]"#),
            ("c2", r#"[{"op":"move","from":"oops","path":"/a"}]"#),
            ("c3", r#"[{"op":"add","path":"/a/~2","value":1}]"#),
        ] {
            promote.transform(Event::tool_call_start(call, "patch_state"));
            let result = Event::tool_call_result("m1", call, content);
            assert_eq!(promote.transform(result.clone()), vec![result], "{content}");
        }
        // The root is a pointer, and so is an escaped segment.
        promote.transform(Event::tool_call_start("c4", "patch_state"));
        let content =
            r#"[{"op":"add","path":"/a~1b","value":1},{"op":"test","path":"","value":{}}]"#;
        let out = promote.transform(Event::tool_call_result("m1", "c4", content));
        assert_eq!(out.len(), 2, "{out:?}");
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
    fn multimodal_result_is_not_promoted_to_state() {
        let mut promote = ToolResultToState::snapshot("load").replacing();
        promote.transform(Event::tool_call_start("c1", "load"));
        let result =
            Event::tool_call_result("m1", "c1", vec![crate::InputContent::text(r#"{"a":1}"#)]);
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
