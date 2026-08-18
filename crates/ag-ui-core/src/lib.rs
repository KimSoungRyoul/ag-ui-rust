//! Core protocol types, events, and wire encoding for the [AG-UI protocol].
//!
//! AG-UI is the protocol between a user-facing application and an agent
//! backend. A run is a stream of [`Event`]s: the agent opens messages, streams
//! text and reasoning, calls tools, publishes state, and finishes — or pauses
//! for human input.
//!
//! This crate is the shared vocabulary. It has no runtime, no I/O and no async:
//! just the types, their exact JSON representation, and the SSE framing that
//! carries them. Servers and clients build on top.
//!
//! ```
//! # #[cfg(feature = "sse")] {
//! use ag_ui_core::{Event, EventStreamFormatter, SseFormatter, TextMessageRole};
//!
//! let formatter = SseFormatter::new();
//! let run = [
//!     Event::run_started("thread-1", "run-1"),
//!     Event::text_message_start("msg-1", TextMessageRole::Assistant),
//!     Event::text_message_content("msg-1", "Hello"),
//!     Event::text_message_end("msg-1"),
//!     Event::run_finished_success("thread-1", "run-1"),
//! ];
//!
//! let body: String = run
//!     .iter()
//!     .map(|event| formatter.encode_to_string(event).unwrap())
//!     .collect();
//!
//! assert!(body.starts_with(r#"data: {"type":"RUN_STARTED","threadId":"thread-1""#));
//! # }
//! ```
//!
//! # Identifiers are strings
//!
//! [`ThreadId`], [`RunId`] and friends wrap [`String`], not `Uuid`. Producers
//! send arbitrary strings and a stricter type would reject valid traffic — see
//! the [`ids`] module for the history.
//!
//! # Features
//!
// A feature list is the one place a doc link is guaranteed to name something
// the current build may not have. Gated so the link stays live where the item
// exists — see `doc-features` in CI.
#![cfg_attr(
    feature = "sse",
    doc = "- `sse` *(default)* — [`SseFormatter`] and `text/event-stream` framing."
)]
#![cfg_attr(
    not(feature = "sse"),
    doc = "- `sse` *(default, off in this build)* — `SseFormatter` and `text/event-stream` framing."
)]
//! - `protobuf` — the binary transport's media type and a documented stub; the
//!   `encode::protobuf` module explains why there is no encoder.
//! - `schemars` — derives `schemars::JsonSchema` on the public types.
//! - `utoipa` — derives `utoipa::ToSchema` on the public types.
//!
//! [AG-UI protocol]: https://github.com/ag-ui-protocol/ag-ui

#![forbid(unsafe_code)]
#![warn(missing_docs)]
#![warn(missing_debug_implementations)]
// Stamps "Available on crate feature X" on every gated item in the rendered
// docs. Only ever set by docs.rs and the `cargo doc` recipe in CONTRIBUTING, so
// it costs a stable build nothing.
#![cfg_attr(docsrs, feature(doc_cfg))]

// `readme = "README.md"` in Cargo.toml makes that file the crate's front page
// wherever the package is presented, so its examples are doctested: a stale one
// is a red build rather than a bad first impression. `cfg(doctest)` is what
// keeps this module out of the rendered docs — it compiles the examples rather
// than publishing them.
// Gated on `sse` because that is the feature the example demonstrates.
#[cfg(all(doctest, feature = "sse"))]
#[doc = include_str!("../README.md")]
mod readme {}

pub mod capabilities;
pub mod context;
pub mod error;
pub mod event;
pub mod ids;
pub mod input;
pub mod message;
pub mod outcome;
pub mod patch;
pub mod token_usage;
pub mod tool;

#[cfg(any(feature = "sse", feature = "protobuf"))]
pub mod encode;

/// A JSON object — the Rust spelling of TypeScript's `Record<string, any>`.
///
/// Key order is preserved, so a payload that round-trips through this crate
/// comes back out in the order it arrived.
pub type JsonObject = serde_json::Map<String, serde_json::Value>;

pub use capabilities::{
    AgentCapabilities, ExecutionCapabilities, HumanInTheLoopCapabilities, IdentityCapabilities,
    MultiAgentCapabilities, MultimodalCapabilities, MultimodalInputCapabilities,
    MultimodalOutputCapabilities, OutputCapabilities, ReasoningCapabilities, StateCapabilities,
    SubAgentInfo, ToolsCapabilities, TransportCapabilities,
};
pub use context::Context;
pub use error::{Error, Result};
pub use event::{
    ActivityDeltaEvent, ActivitySnapshotEvent, BaseEvent, CustomEvent, Event, EventType,
    MessagesSnapshotEvent, RawEvent, ReasoningEncryptedValueEvent, ReasoningEncryptedValueSubtype,
    ReasoningEndEvent, ReasoningMessageChunkEvent, ReasoningMessageContentEvent,
    ReasoningMessageEndEvent, ReasoningMessageStartEvent, ReasoningRole, ReasoningStartEvent,
    RunErrorEvent, RunFinishedEvent, RunStartedEvent, StateDeltaEvent, StateSnapshotEvent,
    StepFinishedEvent, StepStartedEvent, TextMessageChunkEvent, TextMessageContentEvent,
    TextMessageEndEvent, TextMessageRole, TextMessageStartEvent, ToolCallArgsEvent,
    ToolCallChunkEvent, ToolCallEndEvent, ToolCallResultEvent, ToolCallStartEvent, ToolResultRole,
};
// Still part of the protocol, so still re-exported; downstream users get the
// deprecation warning at their use site, not here.
#[allow(deprecated)]
pub use event::{
    ThinkingEndEvent, ThinkingStartEvent, ThinkingTextMessageContentEvent,
    ThinkingTextMessageEndEvent, ThinkingTextMessageStartEvent,
};
pub use ids::{AgentId, MessageId, RunId, StepName, ThreadId, ToolCallId};
pub use input::RunAgentInput;
pub use message::{
    ActivityMessage, AssistantMessage, BinaryInputContent, DeveloperMessage, InputContent,
    InputContentSource, MediaInputContent, Message, ReasoningMessage, Role, SystemMessage,
    TextInputContent, ToolMessage, UserContent, UserMessage,
};
pub use outcome::{Interrupt, ResumeEntry, ResumeStatus, RunOutcome};
pub use patch::{JsonPatch, PatchOperation};
pub use token_usage::{TokenUsage, aggregate_token_usage};
pub use tool::{FunctionCall, Tool, ToolCall, ToolCallKind};

#[cfg(feature = "protobuf")]
pub use encode::protobuf::ProtobufFormatter;
#[cfg(feature = "sse")]
pub use encode::sse::SseFormatter;
#[cfg(any(feature = "sse", feature = "protobuf"))]
pub use encode::{
    EventStreamFormatter, PROTOBUF_MEDIA_TYPE, SSE_MEDIA_TYPE, media_type, supported_media_types,
};
