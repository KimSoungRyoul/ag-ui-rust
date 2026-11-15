//! The agent → renderer operations, by name.
//!
//! A message is `{"version": …, "<operation>": {…}}`: the payload key is what
//! gives it its type. [`Operation`] is the one list of those keys. The typed
//! payload ([`AgentPayload`](super::message::AgentPayload)), the envelope check,
//! the streaming scanner and the history reader all read the names from here
//! rather than spelling them again.

use std::fmt;

use serde_json::Value;

use super::catalog::PropType;

/// One field of an operation's payload: name, JSON type, and whether the
/// protocol requires it. See `A2uiVersion::envelope`.
pub(crate) type EnvelopeField = (&'static str, PropType, bool);

/// One agent → renderer operation: the payload key of a message.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Operation {
    /// `createSurface`: allocate a surface and fix its catalog.
    CreateSurface,
    /// `updateComponents`: add or replace components on a surface.
    UpdateComponents,
    /// `updateDataModel`: write or remove part of a surface's data model.
    UpdateDataModel,
    /// `deleteSurface`: drop a surface and everything under it.
    DeleteSurface,
}

impl Operation {
    /// Every operation, in envelope order.
    pub const ALL: [Self; 4] = [
        Self::CreateSurface,
        Self::UpdateComponents,
        Self::UpdateDataModel,
        Self::DeleteSurface,
    ];

    /// The payload key, exactly as it appears on the wire.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::CreateSurface => "createSurface",
            Self::UpdateComponents => "updateComponents",
            Self::UpdateDataModel => "updateDataModel",
            Self::DeleteSurface => "deleteSurface",
        }
    }

    /// Reads a payload key. `None` for a key that names no operation.
    pub fn parse(key: &str) -> Option<Self> {
        Self::ALL
            .into_iter()
            .find(|operation| operation.as_str() == key)
    }

    /// The operation a raw message carries: the first of [`Operation::ALL`]
    /// it has a key for, or `None` when it has none or is not an object.
    ///
    /// Says nothing about whether the message is otherwise well formed; a
    /// message carrying two operations is reported by the envelope check.
    pub fn of(message: &Value) -> Option<Self> {
        Self::ALL
            .into_iter()
            .find(|operation| message.get(operation.as_str()).is_some())
    }
}

impl fmt::Display for Operation {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::protocol::message::{AgentMessage, Component};
    use serde_json::json;

    #[test]
    fn every_operation_round_trips_through_its_key() {
        for operation in Operation::ALL {
            assert_eq!(Operation::parse(operation.as_str()), Some(operation));
            assert_eq!(operation.to_string(), operation.as_str());
        }
        assert_eq!(Operation::parse("beginRendering"), None);
        assert_eq!(Operation::parse("CreateSurface"), None);
    }

    /// The typed payload derives its keys with serde; this is what ties that
    /// derive to the list, so the two cannot name an operation differently.
    #[test]
    fn the_typed_payload_serializes_under_the_listed_key() {
        let messages = [
            AgentMessage::create_surface("s", "c"),
            AgentMessage::update_components("s", vec![Component::new("root", "Text")]),
            AgentMessage::update_data_model("s", "/", json!({})),
            AgentMessage::delete_surface("s"),
        ];
        let mut seen = Vec::new();
        for message in &messages {
            let operation = message
                .payload
                .operation()
                .expect("a v0.9-family payload names an operation");
            let wire = serde_json::to_value(message).unwrap();
            assert!(
                wire.get(operation.as_str()).is_some(),
                "{wire} has no {operation} key"
            );
            assert_eq!(Operation::of(&wire), Some(operation));
            seen.push(operation);
        }
        assert_eq!(seen, Operation::ALL, "one message per operation, in order");
    }

    #[test]
    fn a_value_that_carries_no_operation_names_none() {
        assert_eq!(Operation::of(&json!({"version": "v0.9"})), None);
        assert_eq!(Operation::of(&json!(["createSurface"])), None);
        assert_eq!(Operation::of(&json!("createSurface")), None);
    }
}
