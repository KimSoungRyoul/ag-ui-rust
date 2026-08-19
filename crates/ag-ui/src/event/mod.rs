//! The AG-UI event stream.
//!
//! An agent run is a sequence of these events. [`Event`] is the closed union of
//! all of them, tagged on the wire by a `type` field holding a
//! SCREAMING_SNAKE_CASE name:
//!
//! ```
//! # use ag_ui::{Event, EventType};
//! let event = Event::text_message_content("msg-1", "Hello");
//! assert_eq!(event.event_type(), EventType::TextMessageContent);
//! assert_eq!(
//!     serde_json::to_string(&event).unwrap(),
//!     r#"{"type":"TEXT_MESSAGE_CONTENT","messageId":"msg-1","delta":"Hello"}"#
//! );
//! ```
//!
//! Every event also carries the optional [`BaseEvent`] fields, flattened into
//! the same JSON object.

// The THINKING_* events are deprecated but still part of the protocol, so this
// module has to name them constantly — in the union, in `event_type()`, in the
// factories. Downstream users still get the warnings; we just do not warn at
// ourselves for implementing the spec as written.
#![allow(deprecated)]

pub mod activity;
pub mod factories;
pub mod lifecycle;
pub mod reasoning;
pub mod special;
pub mod state;
pub mod text;
pub mod tool;

use std::fmt;
use std::str::FromStr;

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::error::{Error, Result};

pub use activity::{ActivityDeltaEvent, ActivitySnapshotEvent};
pub use lifecycle::{
    RunErrorEvent, RunFinishedEvent, RunStartedEvent, StepFinishedEvent, StepStartedEvent,
};
pub use reasoning::{
    ReasoningEncryptedValueEvent, ReasoningEncryptedValueSubtype, ReasoningEndEvent,
    ReasoningMessageChunkEvent, ReasoningMessageContentEvent, ReasoningMessageEndEvent,
    ReasoningMessageStartEvent, ReasoningRole, ReasoningStartEvent, ThinkingEndEvent,
    ThinkingStartEvent, ThinkingTextMessageContentEvent, ThinkingTextMessageEndEvent,
    ThinkingTextMessageStartEvent,
};
pub use special::{CustomEvent, RawEvent};
pub use state::{MessagesSnapshotEvent, StateDeltaEvent, StateSnapshotEvent};
pub use text::{
    TextMessageChunkEvent, TextMessageContentEvent, TextMessageEndEvent, TextMessageRole,
    TextMessageStartEvent,
};
pub use tool::{
    ToolCallArgsEvent, ToolCallChunkEvent, ToolCallEndEvent, ToolCallResultEvent,
    ToolCallStartEvent, ToolResultRole,
};

/// The fields every event may carry, flattened into the event's own JSON
/// object rather than nested under a key.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
#[cfg_attr(feature = "schemars", derive(schemars::JsonSchema))]
#[cfg_attr(feature = "utoipa", derive(utoipa::ToSchema))]
pub struct BaseEvent {
    /// When the event was produced, in milliseconds since the Unix epoch.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timestamp: Option<i64>,
    /// The provider event this was translated from, for debugging.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub raw_event: Option<Value>,
}

impl BaseEvent {
    /// An empty base — no timestamp, no raw event.
    pub fn new() -> Self {
        Self::default()
    }

    /// Whether both fields are absent, in which case the base contributes
    /// nothing to the serialized event.
    pub const fn is_empty(&self) -> bool {
        self.timestamp.is_none() && self.raw_event.is_none()
    }
}

macro_rules! define_events {
    ($(
        $(#[$meta:meta])*
        $variant:ident($payload:ty) => $tag:literal,
    )*) => {
        /// One event in an AG-UI stream.
        ///
        /// Serializes as the payload's fields plus a `type` discriminator, so
        /// there is no nesting on the wire.
        ///
        /// # Exhaustive on purpose
        ///
        /// This enum is deliberately *not* `#[non_exhaustive]`, unlike every
        /// error type in this workspace. A new protocol event **should** be a
        /// compile error where you match on events: that is what a typed SDK
        /// buys you over `serde_json::Value`, and the alternative — a `_` arm
        /// in every consumer — is exactly how the previous Rust SDK came to be
        /// missing eight event types without anyone noticing.
        ///
        /// The consequence is that adding an event is a major version of this
        /// crate. That is the intended price; see `docs/DESIGN.md`.
        #[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
        #[serde(tag = "type")]
        #[cfg_attr(feature = "schemars", derive(schemars::JsonSchema))]
        #[cfg_attr(feature = "utoipa", derive(utoipa::ToSchema))]
        pub enum Event {
            $(
                $(#[$meta])*
                #[serde(rename = $tag)]
                $variant($payload),
            )*
        }

        /// The `type` discriminator of an [`Event`], on its own.
        ///
        /// Useful for routing and filtering without matching the payload.
        /// Exhaustive for the same reason [`Event`] is, and
        /// [`EventType::ALL`] is the list, in upstream order.
        #[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
        #[cfg_attr(feature = "schemars", derive(schemars::JsonSchema))]
        #[cfg_attr(feature = "utoipa", derive(utoipa::ToSchema))]
        pub enum EventType {
            $(
                $(#[$meta])*
                #[serde(rename = $tag)]
                $variant,
            )*
        }

        impl EventType {
            /// Every event type the protocol defines, in upstream order.
            pub const ALL: &'static [EventType] = &[$(EventType::$variant),*];

            /// The wire string for this type.
            pub const fn as_str(&self) -> &'static str {
                match self {
                    $(Self::$variant => $tag,)*
                }
            }
        }

        impl FromStr for EventType {
            type Err = Error;

            fn from_str(s: &str) -> Result<Self> {
                match s {
                    $($tag => Ok(Self::$variant),)*
                    other => Err(Error::UnknownEventType(other.to_owned())),
                }
            }
        }

        impl Event {
            /// The discriminator for this event.
            pub const fn event_type(&self) -> EventType {
                match self {
                    $(Self::$variant(_) => EventType::$variant,)*
                }
            }

            /// The timestamp and raw-event fields, whatever the variant.
            pub const fn base(&self) -> &BaseEvent {
                match self {
                    $(Self::$variant(payload) => &payload.base,)*
                }
            }

            /// Mutable access to the timestamp and raw-event fields.
            pub const fn base_mut(&mut self) -> &mut BaseEvent {
                match self {
                    $(Self::$variant(payload) => &mut payload.base,)*
                }
            }
        }

        $(
            impl From<$payload> for Event {
                fn from(payload: $payload) -> Self {
                    Self::$variant(payload)
                }
            }
        )*
    };
}

define_events! {
    /// Opens a text message. See [`TextMessageStartEvent`].
    TextMessageStart(TextMessageStartEvent) => "TEXT_MESSAGE_START",
    /// Appends to a text message. See [`TextMessageContentEvent`].
    TextMessageContent(TextMessageContentEvent) => "TEXT_MESSAGE_CONTENT",
    /// Closes a text message. See [`TextMessageEndEvent`].
    TextMessageEnd(TextMessageEndEvent) => "TEXT_MESSAGE_END",
    /// A whole text update in one event. See [`TextMessageChunkEvent`].
    TextMessageChunk(TextMessageChunkEvent) => "TEXT_MESSAGE_CHUNK",
    /// Opens a tool call. See [`ToolCallStartEvent`].
    ToolCallStart(ToolCallStartEvent) => "TOOL_CALL_START",
    /// Appends tool-call arguments. See [`ToolCallArgsEvent`].
    ToolCallArgs(ToolCallArgsEvent) => "TOOL_CALL_ARGS",
    /// Closes a tool call. See [`ToolCallEndEvent`].
    ToolCallEnd(ToolCallEndEvent) => "TOOL_CALL_END",
    /// A whole tool call in one event. See [`ToolCallChunkEvent`].
    ToolCallChunk(ToolCallChunkEvent) => "TOOL_CALL_CHUNK",
    /// A tool call's result. See [`ToolCallResultEvent`].
    ToolCallResult(ToolCallResultEvent) => "TOOL_CALL_RESULT",
    /// Opens a thinking block. See [`ThinkingStartEvent`].
    #[cfg_attr(not(feature = "utoipa"), deprecated(note = "use Event::ReasoningStart"))]
    ThinkingStart(ThinkingStartEvent) => "THINKING_START",
    /// Closes a thinking block. See [`ThinkingEndEvent`].
    #[cfg_attr(not(feature = "utoipa"), deprecated(note = "use Event::ReasoningEnd"))]
    ThinkingEnd(ThinkingEndEvent) => "THINKING_END",
    /// Opens a thinking message. See [`ThinkingTextMessageStartEvent`].
    #[cfg_attr(not(feature = "utoipa"), deprecated(note = "use Event::ReasoningMessageStart"))]
    ThinkingTextMessageStart(ThinkingTextMessageStartEvent) => "THINKING_TEXT_MESSAGE_START",
    /// Appends thinking text. See [`ThinkingTextMessageContentEvent`].
    #[cfg_attr(not(feature = "utoipa"), deprecated(note = "use Event::ReasoningMessageContent"))]
    ThinkingTextMessageContent(ThinkingTextMessageContentEvent) => "THINKING_TEXT_MESSAGE_CONTENT",
    /// Closes a thinking message. See [`ThinkingTextMessageEndEvent`].
    #[cfg_attr(not(feature = "utoipa"), deprecated(note = "use Event::ReasoningMessageEnd"))]
    ThinkingTextMessageEnd(ThinkingTextMessageEndEvent) => "THINKING_TEXT_MESSAGE_END",
    /// Replaces the shared state. See [`StateSnapshotEvent`].
    StateSnapshot(StateSnapshotEvent) => "STATE_SNAPSHOT",
    /// Patches the shared state. See [`StateDeltaEvent`].
    StateDelta(StateDeltaEvent) => "STATE_DELTA",
    /// Replaces the message history. See [`MessagesSnapshotEvent`].
    MessagesSnapshot(MessagesSnapshotEvent) => "MESSAGES_SNAPSHOT",
    /// Publishes an activity. See [`ActivitySnapshotEvent`].
    ActivitySnapshot(ActivitySnapshotEvent) => "ACTIVITY_SNAPSHOT",
    /// Patches an activity. See [`ActivityDeltaEvent`].
    ActivityDelta(ActivityDeltaEvent) => "ACTIVITY_DELTA",
    /// Forwards a provider event. See [`RawEvent`].
    Raw(RawEvent) => "RAW",
    /// An application-defined event. See [`CustomEvent`].
    Custom(CustomEvent) => "CUSTOM",
    /// Starts a run. See [`RunStartedEvent`].
    RunStarted(RunStartedEvent) => "RUN_STARTED",
    /// Finishes or pauses a run. See [`RunFinishedEvent`].
    RunFinished(RunFinishedEvent) => "RUN_FINISHED",
    /// Fails a run. See [`RunErrorEvent`].
    RunError(RunErrorEvent) => "RUN_ERROR",
    /// Starts a step. See [`StepStartedEvent`].
    StepStarted(StepStartedEvent) => "STEP_STARTED",
    /// Finishes a step. See [`StepFinishedEvent`].
    StepFinished(StepFinishedEvent) => "STEP_FINISHED",
    /// Opens a reasoning block. See [`ReasoningStartEvent`].
    ReasoningStart(ReasoningStartEvent) => "REASONING_START",
    /// Opens a reasoning message. See [`ReasoningMessageStartEvent`].
    ReasoningMessageStart(ReasoningMessageStartEvent) => "REASONING_MESSAGE_START",
    /// Appends reasoning text. See [`ReasoningMessageContentEvent`].
    ReasoningMessageContent(ReasoningMessageContentEvent) => "REASONING_MESSAGE_CONTENT",
    /// Closes a reasoning message. See [`ReasoningMessageEndEvent`].
    ReasoningMessageEnd(ReasoningMessageEndEvent) => "REASONING_MESSAGE_END",
    /// A whole reasoning update in one event. See [`ReasoningMessageChunkEvent`].
    ReasoningMessageChunk(ReasoningMessageChunkEvent) => "REASONING_MESSAGE_CHUNK",
    /// Closes a reasoning block. See [`ReasoningEndEvent`].
    ReasoningEnd(ReasoningEndEvent) => "REASONING_END",
    /// Carries an encrypted reasoning blob. See [`ReasoningEncryptedValueEvent`].
    ReasoningEncryptedValue(ReasoningEncryptedValueEvent) => "REASONING_ENCRYPTED_VALUE",
}

impl fmt::Display for EventType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl Event {
    /// Stamps the event with a millisecond Unix timestamp.
    #[must_use]
    pub fn with_timestamp(mut self, timestamp: i64) -> Self {
        self.base_mut().timestamp = Some(timestamp);
        self
    }

    /// Attaches the provider event this was translated from.
    #[must_use]
    pub fn with_raw_event(mut self, raw_event: impl Into<Value>) -> Self {
        self.base_mut().raw_event = Some(raw_event.into());
        self
    }

    /// Whether this event's type is deprecated in favour of a `REASONING_*`
    /// one.
    pub const fn is_deprecated(&self) -> bool {
        matches!(
            self.event_type(),
            EventType::ThinkingStart
                | EventType::ThinkingEnd
                | EventType::ThinkingTextMessageStart
                | EventType::ThinkingTextMessageContent
                | EventType::ThinkingTextMessageEnd
        )
    }
}
