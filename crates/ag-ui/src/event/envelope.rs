//! Standard events with host-owned top-level extension fields.
//!
//! This type keeps the protocol value typed and available to the verifier. It
//! never permits an extension to replace a field defined by AG-UI, even when
//! that optional field is absent from this particular event.

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::{Error, Event, JsonObject, Result};

/// An SDK event and host-specific extension fields in one flat wire object.
///
/// Deserialization follows the standard event's normalization rules. This is
/// not a lossless historical JSON container; transports can separately accept
/// a legacy replay representation when exact old omissions must be preserved.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct EventEnvelope {
    #[serde(flatten)]
    protocol: Event,
    #[serde(flatten)]
    extensions: JsonObject,
}

impl EventEnvelope {
    /// Wraps an event without changing its wire representation.
    pub fn new(protocol: Event) -> Self {
        Self {
            protocol,
            extensions: JsonObject::new(),
        }
    }

    /// Adds extension fields, rejecting all names owned by the protocol.
    ///
    /// Names are reserved across event types. An extension cannot become a
    /// standard field merely because the enclosing event type later changes.
    pub fn with_extensions(mut self, extensions: JsonObject) -> Result<Self> {
        for key in extensions.keys() {
            if reserved_field(key) {
                return Err(Error::Protocol(format!(
                    "event extension collides with protocol field {key:?}"
                )));
            }
        }
        self.extensions.extend(extensions);
        Ok(self)
    }

    /// The actual standard event being serialized, suitable for verification.
    pub const fn protocol(&self) -> &Event {
        &self.protocol
    }

    /// Mutable access to the standard event. Extensions cannot shadow any
    /// standard field, including fields belonging to a different event type.
    pub fn protocol_mut(&mut self) -> &mut Event {
        &mut self.protocol
    }

    /// The host extension fields. Their contents have no AG-UI semantics.
    pub const fn extensions(&self) -> &JsonObject {
        &self.extensions
    }

    /// Removes the host extension fields and returns the standard event.
    pub fn into_protocol(self) -> Event {
        self.protocol
    }
}

impl From<Event> for EventEnvelope {
    fn from(protocol: Event) -> Self {
        Self::new(protocol)
    }
}

impl AsRef<Event> for EventEnvelope {
    fn as_ref(&self) -> &Event {
        self.protocol()
    }
}

impl<'de> Deserialize<'de> for EventEnvelope {
    fn deserialize<D>(deserializer: D) -> std::result::Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let fields = JsonObject::deserialize(deserializer)?;
        let protocol =
            Event::deserialize(Value::Object(fields.clone())).map_err(serde::de::Error::custom)?;
        let extensions = fields
            .into_iter()
            .filter(|(key, _)| !reserved_field(key))
            .collect();
        Ok(Self {
            protocol,
            extensions,
        })
    }
}

// The union of top-level fields in Event's payloads, including flattened base
// fields and the discriminator. This reserves optional fields too; checking
// only the serialized object would allow e.g. a missing `subagentRunId` to be
// injected as an unverified extension. Nested payload fields are not reserved.
fn reserved_field(key: &str) -> bool {
    matches!(
        key,
        "type"
            | "timestamp"
            | "rawEvent"
            | "metadata"
            | "threadId"
            | "runId"
            | "parentRunId"
            | "input"
            | "result"
            | "outcome"
            | "usage"
            | "message"
            | "code"
            | "stepName"
            | "messageId"
            | "role"
            | "name"
            | "delta"
            | "toolCallId"
            | "toolCallName"
            | "parentMessageId"
            | "content"
            | "activityType"
            | "replace"
            | "patch"
            | "snapshot"
            | "messages"
            | "subtype"
            | "entityId"
            | "encryptedValue"
            | "title"
            | "event"
            | "source"
            | "value"
            | "subagentRunId"
            | "description"
            | "parentSubagentRunId"
            | "parentToolCallId"
    )
}
