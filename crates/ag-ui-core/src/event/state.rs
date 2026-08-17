//! Shared state and message-history snapshots: `STATE_SNAPSHOT`,
//! `STATE_DELTA`, `MESSAGES_SNAPSHOT`.

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::event::BaseEvent;
use crate::message::Message;
use crate::patch::PatchOperation;

/// Replaces the shared state wholesale.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
#[cfg_attr(feature = "schemars", derive(schemars::JsonSchema))]
#[cfg_attr(feature = "utoipa", derive(utoipa::ToSchema))]
pub struct StateSnapshotEvent {
    /// Timestamp and raw provider event.
    #[serde(flatten)]
    pub base: BaseEvent,
    /// The complete new state. Free-form JSON, opaque to the protocol.
    pub snapshot: Value,
}

impl StateSnapshotEvent {
    /// Publishes a new state.
    pub fn new(snapshot: impl Into<Value>) -> Self {
        Self {
            base: BaseEvent::default(),
            snapshot: snapshot.into(),
        }
    }
}

/// Mutates the shared state with a JSON Patch document.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
#[cfg_attr(feature = "schemars", derive(schemars::JsonSchema))]
#[cfg_attr(feature = "utoipa", derive(utoipa::ToSchema))]
pub struct StateDeltaEvent {
    /// Timestamp and raw provider event.
    #[serde(flatten)]
    pub base: BaseEvent,
    /// RFC 6902 operations, applied in order to the previous state.
    pub delta: Vec<PatchOperation>,
}

impl StateDeltaEvent {
    /// Publishes a patch against the current state.
    pub fn new(delta: impl Into<Vec<PatchOperation>>) -> Self {
        Self {
            base: BaseEvent::default(),
            delta: delta.into(),
        }
    }
}

/// Replaces the message history wholesale.
///
/// Used to reconcile after a reconnect, or when an agent rewrites history (for
/// example after summarizing older turns).
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
#[cfg_attr(feature = "schemars", derive(schemars::JsonSchema))]
#[cfg_attr(feature = "utoipa", derive(utoipa::ToSchema))]
pub struct MessagesSnapshotEvent {
    /// Timestamp and raw provider event.
    #[serde(flatten)]
    pub base: BaseEvent,
    /// The complete message list, oldest first.
    pub messages: Vec<Message>,
}

impl MessagesSnapshotEvent {
    /// Publishes a new message history.
    pub fn new(messages: impl Into<Vec<Message>>) -> Self {
        Self {
            base: BaseEvent::default(),
            messages: messages.into(),
        }
    }
}
