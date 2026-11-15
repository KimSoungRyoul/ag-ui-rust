//! The AG-UI binary transport — **not implemented**.
//!
//! This module exists so that a build with the `protobuf` feature can name the
//! media type, and so the reason it does nothing is written down next to the
//! code rather than discovered at runtime. [`ProtobufFormatter`] reports the
//! binary media type and refuses every event; negotiation never picks that
//! media type, with or without this feature.
//!
//! # Why there is no encoder here
//!
//! The binary transport is defined by `events.proto` in the upstream
//! `@ag-ui/proto` package. When this module was written, its `Event` message
//! was a `oneof` over **21** of the **36** event types this crate defines:
//!
//! `TEXT_MESSAGE_START`, `TEXT_MESSAGE_CONTENT`, `TEXT_MESSAGE_END`,
//! `TEXT_MESSAGE_CHUNK`, `TOOL_CALL_START`, `TOOL_CALL_ARGS`, `TOOL_CALL_END`,
//! `TOOL_CALL_CHUNK`, `STATE_SNAPSHOT`, `STATE_DELTA`, `MESSAGES_SNAPSHOT`,
//! `RAW`, `CUSTOM`, `RUN_STARTED`, `RUN_FINISHED`, `RUN_ERROR`, `STEP_STARTED`,
//! `STEP_FINISHED`, `SUBAGENT_STARTED`, `SUBAGENT_FINISHED`, `SUBAGENT_ERROR`.
//!
//! (Upstream's own documentation said 19: the two `*_CHUNK` events had a
//! `oneof` arm but no `EventType` enum value to select it with, so they could
//! not be encoded in practice either.)
//!
//! The other 15 had no wire representation at all: every `REASONING_*` event,
//! both `ACTIVITY_*` events, all five deprecated `THINKING_*` events, and
//! `TOOL_CALL_RESULT`. An agent that reasons, reports activities, or returns a
//! tool result — which is most of them — could not express its stream in that
//! format. Encoding such a run would have meant silently dropping events, so
//! this crate declined to encode any. [`COVERED_EVENT_TYPES`] still lists the
//! 21, as a record of that file.
//!
//! That reason is gone. Upstream now generates `events.proto` from the AG-UI
//! 1.0 JSON Schema, and both its `EventType` enum and its `oneof` cover all 31
//! 1.0 event types. Only the five deprecated `THINKING_*` events, which 1.0
//! retired, are absent.
//!
//! The encoder is still not written
//! ([#46](https://github.com/KimSoungRyoul/ag-ui-rust/issues/46)), for two
//! reasons:
//!
//! - Nothing would ask for it. The stock TypeScript client, `HttpAgent` in
//!   `@ag-ui/client`, sends `Accept: text/event-stream`, so a server that
//!   could encode protobuf would still not be asked to.
//! - It is a second encoding to keep in step with a generated schema, for a
//!   path nobody has asked this SDK for. Generating the Rust types means
//!   either a `build.rs` that requires `protoc` on every consumer's machine or
//!   checked-in generated code to refresh each time upstream regenerates.
//!
//! An encoder can be added behind this same feature later without a breaking
//! change.
//!
//! # What to do instead
//!
#![cfg_attr(
    feature = "sse",
    doc = "Use [`sse`](crate::encode::sse), which carries every event type this crate defines."
)]
#![cfg_attr(
    not(feature = "sse"),
    doc = "Use the `sse` module — enable the `sse` feature — which carries every event",
    doc = "type this crate defines."
)]

use crate::encode::{EventStreamFormatter, PROTOBUF_MEDIA_TYPE};
use crate::error::{Error, Result};
use crate::event::{Event, EventType};

/// Placeholder for the binary formatter.
///
/// [`content_type`](EventStreamFormatter::content_type) reports the binary
/// media type, but [`encode`](EventStreamFormatter::encode) always fails with
/// [`Error::UnsupportedTransport`] — see the [module docs](self) for why.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ProtobufFormatter;

impl ProtobufFormatter {
    /// Builds the placeholder formatter.
    pub const fn new() -> Self {
        Self
    }
}

impl EventStreamFormatter for ProtobufFormatter {
    fn content_type(&self) -> &'static str {
        PROTOBUF_MEDIA_TYPE
    }

    fn encode(&self, _event: &Event) -> Result<Vec<u8>> {
        Err(Error::UnsupportedTransport(
            "this crate has no AG-UI protobuf encoder; use SSE",
        ))
    }
}

/// The event types the pre-1.0 upstream `events.proto` could represent.
///
/// Everything not listed here had no binary encoding in that file. The 1.0
/// `events.proto` covers every 1.0 event type; see the [module docs](self).
pub const COVERED_EVENT_TYPES: &[EventType] = &[
    EventType::TextMessageStart,
    EventType::TextMessageContent,
    EventType::TextMessageEnd,
    EventType::TextMessageChunk,
    EventType::ToolCallStart,
    EventType::ToolCallArgs,
    EventType::ToolCallEnd,
    EventType::ToolCallChunk,
    EventType::StateSnapshot,
    EventType::StateDelta,
    EventType::MessagesSnapshot,
    EventType::Raw,
    EventType::Custom,
    EventType::RunStarted,
    EventType::RunFinished,
    EventType::RunError,
    EventType::StepStarted,
    EventType::StepFinished,
    EventType::SubagentStarted,
    EventType::SubagentFinished,
    EventType::SubagentError,
];

/// Whether the pre-1.0 upstream `events.proto` could represent `event_type`:
/// whether it is in [`COVERED_EVENT_TYPES`]. Every 1.0 event type has a
/// representation in the 1.0 file.
pub fn is_covered(event_type: EventType) -> bool {
    COVERED_EVENT_TYPES.contains(&event_type)
}
