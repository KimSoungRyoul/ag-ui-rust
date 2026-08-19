//! The one extension point: rewriting the event stream on its way out.
//!
//! Everything that wants to observe, drop, rewrite or add events implements
//! [`StreamTransformer`]. There is deliberately no second mechanism — an early
//! draft of this crate also carried a builder of `map_content` / `map_result` /
//! `map_interrupt` closures ported from the .NET SDK, which meant two ways to
//! do the same thing and a pile of `Box<dyn Fn>`. Those hooks are built-in
//! transformers now: see [`ToolResultToState`].
//!
//! Transformers run in the order they were added, each seeing what the previous
//! one produced, before the ordering verifier sees anything. That order is what
//! makes [`FilterToolCalls`] safe: the verifier never sees the half of a tool
//! call that was dropped.
//!
//! ```
//! # use ag_ui::serve::{FilterToolCalls, TransformerChain, ToolResultToState};
//! let chain = TransformerChain::new()
//!     .with(FilterToolCalls::deny(["internal_debug"]))
//!     .with(ToolResultToState::snapshot("load_document").replacing());
//! assert_eq!(chain.len(), 2);
//! ```

use std::collections::{HashMap, HashSet};

use crate::{Event, PatchOperation, ToolCallId};
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
/// # use ag_ui::serve::{FilterToolCalls, StreamTransformer};
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
/// # use ag_ui::serve::{StreamTransformer, ToolResultToState};
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
