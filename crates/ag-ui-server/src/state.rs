//! Publishing shared state as snapshots or patches.
//!
//! The client keeps a copy of the agent's state. Sending the whole thing on
//! every change is wasteful for a large document that gained one field, and
//! sending a patch is wasteful for a small document that changed completely.
//! [`StateManager`] keeps the last published snapshot and picks per publish:
//!
//! 1. the first publish is always a `STATE_SNAPSHOT` — the client's copy may
//!    have drifted, and a patch against an unknown base is inapplicable;
//! 2. afterwards it diffs against the last snapshot with [RFC 6902] and emits
//!    `STATE_DELTA`;
//! 3. unless the serialized patch is no smaller than the serialized snapshot,
//!    in which case it snapshots instead.
//!
//! [RFC 6902]: https://datatracker.ietf.org/doc/html/rfc6902

use ag_ui_core::{Event, JsonPatch, PatchOperation};
use serde_json::Value;

use crate::error::Result;

/// What a publish decided to send.
#[derive(Clone, Debug, PartialEq)]
pub enum StatePublish {
    /// The whole state — the first publish, or a change too large to patch.
    Snapshot(Value),
    /// A patch against the previously published snapshot.
    Delta(JsonPatch),
    /// The state is byte-identical to the last publish; nothing to send.
    Unchanged,
}

impl StatePublish {
    /// The event to emit, or `None` for [`StatePublish::Unchanged`].
    pub fn into_event(self) -> Option<Event> {
        match self {
            Self::Snapshot(value) => Some(Event::state_snapshot(value)),
            Self::Delta(patch) => Some(Event::state_delta(patch)),
            Self::Unchanged => None,
        }
    }
}

/// Tracks the last published state so changes can go out as patches.
///
/// ```
/// # use ag_ui_core::PatchOperation;
/// # use ag_ui_server::{StateManager, StatePublish};
/// # use serde_json::json;
/// let mut states = StateManager::new();
/// let notes = "the document the user is editing, at some length";
///
/// // First publish: a snapshot, whatever the size.
/// let first = states.publish(json!({"step": 1, "notes": notes}))?;
/// assert!(matches!(first, StatePublish::Snapshot(_)));
///
/// // One field of a large document: a patch, because it is smaller.
/// let second = states.publish(json!({"step": 2, "notes": notes}))?;
/// assert_eq!(
///     second,
///     StatePublish::Delta(vec![PatchOperation::replace("/step", 2)])
/// );
///
/// // A small document changing wholesale: back to a snapshot, because the
/// // patch would be bigger than the state it describes.
/// let mut small = StateManager::new();
/// small.publish(json!({"a": 1}))?;
/// assert_eq!(
///     small.publish(json!({"b": 2}))?,
///     StatePublish::Snapshot(json!({"b": 2}))
/// );
/// # Ok::<(), ag_ui_server::Error>(())
/// ```
#[derive(Clone, Debug, Default)]
pub struct StateManager {
    published: Option<Value>,
}

impl StateManager {
    /// A manager that has published nothing yet.
    pub fn new() -> Self {
        Self::default()
    }

    /// The last published state, or `None` before the first publish.
    pub fn published(&self) -> Option<&Value> {
        self.published.as_ref()
    }

    /// Forgets the last publish, so the next one is a snapshot again.
    ///
    /// Call this after emitting a `STATE_SNAPSHOT` by hand, or after a
    /// reconnect where the client's copy is no longer known.
    pub fn reset(&mut self) {
        self.published = None;
    }

    /// Decides how to publish `next` and records it as the new baseline.
    ///
    /// Returns [`StatePublish::Unchanged`] when nothing moved, so callers can
    /// skip emitting entirely.
    pub fn publish(&mut self, next: Value) -> Result<StatePublish> {
        let Some(previous) = self.published.as_ref() else {
            self.published = Some(next.clone());
            return Ok(StatePublish::Snapshot(next));
        };

        if previous == &next {
            return Ok(StatePublish::Unchanged);
        }

        let patch = diff(previous, &next)?;
        // An empty patch with a non-equal value cannot happen for well-formed
        // JSON, but treating it as "unchanged" is safer than emitting a
        // STATE_DELTA the client would apply as a no-op.
        if patch.is_empty() {
            self.published = Some(next);
            return Ok(StatePublish::Unchanged);
        }

        let patch_size = serde_json::to_vec(&patch)?.len();
        let snapshot_size = serde_json::to_vec(&next)?.len();
        self.published = Some(next.clone());

        if patch_size < snapshot_size {
            Ok(StatePublish::Delta(patch))
        } else {
            Ok(StatePublish::Snapshot(next))
        }
    }
}

/// Computes an RFC 6902 patch and re-reads it as the protocol's own operation
/// type.
///
/// `json-patch` has its own `PatchOperation` with `jsonptr` paths; the wire
/// format is identical, so the round trip through `serde_json::Value` is the
/// conversion. It costs one allocation per publish, which is nothing next to
/// the diff itself.
fn diff(previous: &Value, next: &Value) -> Result<JsonPatch> {
    let patch = json_patch::diff(previous, next);
    let operations: Vec<PatchOperation> = serde_json::from_value(serde_json::to_value(patch)?)?;
    Ok(operations)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn first_publish_is_a_snapshot_even_when_tiny() {
        let mut states = StateManager::new();
        let published = states.publish(json!({"a": 1})).expect("publish failed");
        assert_eq!(published, StatePublish::Snapshot(json!({"a": 1})));
    }

    #[test]
    fn identical_state_publishes_nothing() {
        let mut states = StateManager::new();
        states.publish(json!({"a": 1})).expect("publish failed");
        let second = states.publish(json!({"a": 1})).expect("publish failed");
        assert_eq!(second, StatePublish::Unchanged);
    }

    #[test]
    fn small_change_to_large_state_is_a_delta() {
        let mut states = StateManager::new();
        let big = json!({"notes": ["a".repeat(200)], "step": 1});
        states.publish(big).expect("publish failed");
        let published = states
            .publish(json!({"notes": ["a".repeat(200)], "step": 2}))
            .expect("publish failed");
        assert_eq!(
            published,
            StatePublish::Delta(vec![PatchOperation::replace("/step", 2)])
        );
    }

    #[test]
    fn wholesale_change_falls_back_to_a_snapshot() {
        let mut states = StateManager::new();
        states.publish(json!({"a": 1})).expect("publish failed");
        let next = json!({"b": 2});
        let published = states.publish(next.clone()).expect("publish failed");
        assert_eq!(published, StatePublish::Snapshot(next));
    }

    #[test]
    fn reset_forces_the_next_publish_to_snapshot() {
        let mut states = StateManager::new();
        states
            .publish(json!({"notes": ["a".repeat(200)], "step": 1}))
            .expect("publish failed");
        states.reset();
        let next = json!({"notes": ["a".repeat(200)], "step": 2});
        let published = states.publish(next.clone()).expect("publish failed");
        assert_eq!(published, StatePublish::Snapshot(next));
    }
}
