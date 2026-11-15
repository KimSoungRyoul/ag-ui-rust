//! A2UI protocol types, semantic validator, and the agent layer a framework
//! delegates A2UI to.
//!
//! [A2UI](https://a2ui.org) is a declarative, agent-driven UI protocol: an agent
//! streams JSON describing a surface, and a renderer draws it. This crate is the
//! **agent half** of that exchange.
//!
//! # This crate does not render
//!
//! Nothing here draws pixels, lays out a tree, or evaluates a UI at runtime. It
//! produces A2UI, validates it, and transports it. Rendering is the client's
//! job, and it is a genuinely different program — one with a widget toolkit, an
//! event loop, and a reactive data model. What this crate gives you instead is
//! the protocol, and what an agent needs around it.
//!
//! # The protocol
//!
//! The protocol modules sit at the crate root. Everything in them is a pure
//! function of the messages it is given: no model call, no transport, no clock.
//! An agent framework, a transport adapter and a test all use the same pieces,
//! and each fact about the protocol is stated in exactly one of them.
//!
//! | Module | What it is the one place for |
//! |---|---|
//! | [`version`] | Which protocol versions exist, how each is spelled, and what differs between them. |
//! | [`operation`] | The names of the agent → renderer operations. |
//! | [`message`] | The wire types, in both directions. A message is written and read as its `version` spells it. |
//! | [`catalog`] | What a surface may contain: components, their properties, functions. Includes the basic catalog of each version, read from the published document. |
//! | [`binding`] | JSON Pointer resolution, template scopes and the `formatString` interpolation grammar, so an agent can check its own bindings before shipping them. |
//! | [`model`] | The data model, including the undefined slots JSON cannot express. |
//! | [`surface`] | The surface state machine: lifecycle, and folding a stream of operations. Every stream is folded through it. |
//! | [`processor`] | Taking raw messages into a surface store: the catalogs a context speaks, the order of the checks, a batch as a transaction, and the kinds of refusal. |
//! | [`validate`] | The semantic checks JSON Schema cannot express: does every child reference resolve, is there a root, is the tree acyclic. Plus the two a generating model gets wrong often enough to be worth checking without a schema engine: the message envelope, and property values against the type the catalog declares. |
//! | [`schema`] | The three JSON Schema documents that describe a surface (envelope, common types, catalog), and pruning them to what a model needs. |
// Gate module links with their features so minimal rustdoc builds resolve them.
#![cfg_attr(
    feature = "schema-validation",
    doc = "| [`schema_validation`] | Draft 2020-12 validation of agent → renderer messages (feature `schema-validation`). |",
    doc = "| [`client_schema`] | The same for renderer → agent messages (feature `schema-validation`). |"
)]
//!
//! One further piece is internal: the child-reference graph of a component
//! list, and the single depth-first walk over it that answers reachability,
//! cycles and depth for every surface the validator is shown, finished or
//! still arriving.
//!
//! ## What depends on what
//!
//! The protocol modules reach into nothing above them, and neither does the
//! `schema-validation` feature.
//!
//! With default features off, the protocol modules are all there is, and they
//! need only `serde`, `serde_json`, `thiserror`, `jsonptr` and `unicode-ident`.
//!
//! # Around the protocol
//!
//! There is one generation path: the agent layer. Its prompt snippet, its
//! reader for each output format and its retry decision are the only ones in
//! the crate, and everything else that generates is a use of it.
//!
#![cfg_attr(
    feature = "agent",
    doc = "- [`agent`] (feature `agent`, on by default) — what an agent framework",
    doc = "  delegates A2UI to: catalog negotiation, the prompt snippet, reading a",
    doc = "  model's answer whole or streamed, the mirror of the renderer's state, and",
    doc = "  the retry decision. The framework makes the model call; nothing in the",
    doc = "  layer does I/O."
)]
#![cfg_attr(
    not(feature = "agent"),
    doc = "- `agent` (feature `agent`, off in this build) — what an agent framework",
    doc = "  delegates A2UI to: negotiation, prompt snippet, answer parsing, renderer",
    doc = "  state and the retry decision, with no model call and no I/O."
)]
#![cfg_attr(
    feature = "author",
    doc = "- [`author`] (feature `author`) — one surface created or edited by a model,",
    doc = "  with a fixed target, version and catalog, held to the JSON Schemas: the",
    doc = "  agent layer for the case where that is all an application wants."
)]
#![cfg_attr(
    not(feature = "author"),
    doc = "- `author` (feature `author`, off in this build) — one surface created or",
    doc = "  edited by a model, held to the JSON Schemas, on the agent layer."
)]
//!
//! # Transport
//!
//! A2UI says nothing about how messages reach the renderer, and everything
//! outside [`adapters`] keeps it that way. An adapter is one transport's
//! conventions, each behind its own feature:
//!
#![cfg_attr(
    feature = "ag-ui",
    doc = "- [`adapters::agui`] (feature `ag-ui`, and `ag-ui-server` to paint from a",
    doc = "  run) — AG-UI, as AG-UI's own A2UI middleware speaks it: a batch of",
    doc = "  operations as a `{\"a2ui_operations\": [...]}` tool result, the",
    doc = "  `a2ui-surface` activity a frontend paints from, the two tools, and",
    doc = "  surfaces read back out of a conversation."
)]
#![cfg_attr(
    not(feature = "ag-ui"),
    doc = "- `adapters::agui` (feature `ag-ui`, off in this build) — AG-UI, as AG-UI's",
    doc = "  own A2UI middleware speaks it. The dependency on `ag-ui` comes with the",
    doc = "  feature and goes with it."
)]
#![cfg_attr(
    feature = "a2a",
    doc = "- [`adapters::a2a`] (feature `a2a`) — A2A: data parts, the agent-card",
    doc = "  extension and a renderer's capabilities, as JSON, with no A2A library."
)]
#![cfg_attr(
    not(feature = "a2a"),
    doc = "- `adapters::a2a` (feature `a2a`, off in this build) — A2A: data parts, the",
    doc = "  agent-card extension and a renderer's capabilities, as JSON."
)]
//!
//! # Conformance
//!
//! The A2UI project publishes a language-agnostic conformance suite as YAML.
//! It is vendored under `tests/conformance/` and run as a normal test; the
//! report prints what passed, what was skipped, and why. See the README there
//! for the current standing.
//!
//! # Version
//!
//! The crate speaks `v0.9`, `v0.9.1` and `1.0`, in the protocol modules, the
//! agent layer and the adapters alike. [`A2uiVersion`] is the one place a
//! version is parsed or named, and what differs between versions is a method
//! on it. There is one set of types: a message is written and read as its
//! `version` spells it, a surface speaks the version it was created in, and
//! nothing is translated from one family to the other.
//!
//! A constructor stamps `v0.9` ([`A2uiVersion::DEFAULT`]) unless it is told
//! otherwise, so nothing changes for a caller that never names
//! [`A2uiVersion::V1_0`]. A session of the agent layer speaks the version of
//! the catalogs it was given: register v1.0 catalogs, and the prompt, the
//! reading of an answer and the renderer's capabilities are v1.0's. The
//! function-call messages of v1.0 (`callRendererFunction`,
//! `callAgentFunction` and their responses) are not accepted or serialized.
//!
//! # Example
//!
//! ```
//! use ag_ui_a2ui::{Catalog, Component, Validator};
//! use serde_json::json;
//!
//! let catalog = Catalog::basic();
//! let components = vec![
//!     Component::new("root", "Card").with("child", json!("greeting")),
//!     Component::new("greeting", "Text").with("text", json!("Hello!")),
//! ];
//!
//! let report = Validator::new(&catalog).validate(&components);
//! assert!(report.is_valid());
//! ```
//!
//! The same surface in v1.0, taken in as the message an agent sends. The
//! creation carries its components and its data, and a binding is written
//! `@path`:
//!
//! ```
//! use ag_ui_a2ui::{A2uiVersion, Catalog, MessageProcessor};
//! use serde_json::json;
//!
//! let catalog = Catalog::basic_for(A2uiVersion::V1_0);
//! let mut processor = MessageProcessor::new();
//! processor.register(catalog.clone())?;
//! processor
//!     .process(&json!({
//!         "version": "1.0",
//!         "createSurface": {
//!             "surfaceId": "hello",
//!             "catalogId": catalog.catalog_id,
//!             "dataModel": {"name": "Ada"},
//!             "components": [
//!                 {"id": "root", "component": "Card", "child": "greeting"},
//!                 {"id": "greeting", "component": "Text", "text": {"@path": "/name"}}
//!             ]
//!         }
//!     }))
//!     .map_err(|rejection| rejection.error)?;
//!
//! let surface = processor.surfaces().get("hello").unwrap();
//! assert_eq!(surface.spoken_version(), A2uiVersion::V1_0);
//! assert_eq!(surface.components.len(), 2);
//! # Ok::<(), ag_ui_a2ui::Error>(())
//! ```

#![forbid(unsafe_code)]
#![warn(missing_docs)]
#![warn(missing_debug_implementations)]
// See `ag_ui`'s lib.rs: a library never prints.
#![warn(clippy::print_stdout, clippy::print_stderr)]
// See `ag_ui`'s lib.rs: marks feature-gated items in the rendered docs.
#![cfg_attr(docsrs, feature(doc_cfg))]

// Keep README examples covered by doctests.
#[cfg(doctest)]
#[doc = include_str!("../README.md")]
mod readme {}

pub mod constants;
pub mod error;

// The protocol modules are files under `src/protocol/` and public at the crate
// root, which is their one path: `ag_ui_a2ui::validate`, not a second one
// through a public `protocol`. The directory only groups the files.
mod protocol;

#[doc(inline)]
pub use crate::protocol::{
    binding, catalog, message, model, operation, processor, rpc, schema, surface, validate, version,
};
#[cfg(feature = "schema-validation")]
#[doc(inline)]
pub use crate::protocol::{client_schema, schema_validation};

#[cfg(feature = "agent")]
pub mod agent;

#[cfg(feature = "author")]
pub mod author;
#[cfg(feature = "author")]
pub use author::{A2uiAuthor, AuthorRequest, ValidatedSurface};

pub mod adapters;

pub use crate::protocol::catalog::Catalog;
pub use crate::protocol::message::{
    AgentMessage, AgentPayload, ChildList, ChildTemplate, ClientDataModel, Component,
    DataModelUpdate, RendererMessage, RendererPayload,
};
pub use crate::protocol::model::{DataModel, ModelChange, ModelValue};
pub use crate::protocol::operation::Operation;
pub use crate::protocol::processor::MessageProcessor;
pub use crate::protocol::surface::{Lifecycle, Placed, SurfaceState, SurfaceStore};
pub use crate::protocol::validate::{
    ErrorCode, ValidateOptions, ValidationError, ValidationReport, Validator,
};
pub use crate::protocol::version::A2uiVersion;
pub use error::{Error, Result, ValidationErrors};

// What `#[non_exhaustive]` holds a caller to, outside this crate: a type that
// later work extends cannot be written out field by field or matched without a
// wildcard arm. Each block compiled before the types were marked (#95) and
// must not now. A stable rustdoc does not check the error code, only that the
// block fails, so a block is kept to code that compiles without the mark.
#[cfg(all(doctest, feature = "ag-ui"))]
/// ```compile_fail,E0639
/// use ag_ui_a2ui::agent::{AfterRejectedBlock, StreamOptions};
/// let _ = StreamOptions { wrapped: false, after_rejected_block: AfterRejectedBlock::End };
/// ```
///
/// ```compile_fail,E0639
/// let _ = ag_ui_a2ui::adapters::agui::Attempt { attempt: 1, ok: false, errors: Vec::new() };
/// ```
///
/// ```compile_fail,E0639
/// let _ = ag_ui_a2ui::adapters::agui::Painted { envelope: String::new(), activities: Vec::new() };
/// ```
///
/// ```compile_fail,E0639
/// let _ = ag_ui_a2ui::agent::Candidate { message: serde_json::json!({}), closed: true };
/// ```
///
/// ```compile_fail,E0639
/// use ag_ui_a2ui::adapters::agui::UserAction;
/// let _ = UserAction { name: Some("checkout".into()), ..UserAction::default() };
/// ```
///
/// ```compile_fail,E0639
/// let _ = ag_ui_a2ui::adapters::agui::CatalogContext { catalog_id: None, schema: serde_json::json!({}) };
/// ```
///
/// ```compile_fail,E0004
/// use ag_ui_a2ui::agent::Verdict;
/// fn act(verdict: Verdict) {
///     match verdict {
///         Verdict::Done(_) => {}
///         Verdict::Retry(_) => {}
///         Verdict::GiveUp(_) => {}
///     }
/// }
/// ```
///
/// ```compile_fail,E0004
/// use ag_ui_a2ui::agent::StreamPart;
/// fn show(part: StreamPart) {
///     match part {
///         StreamPart::Text(_) => {}
///         StreamPart::A2ui(_) => {}
///         StreamPart::Rejected(_) => {}
///     }
/// }
/// ```
///
/// ```compile_fail,E0004
/// use ag_ui_a2ui::agent::ReaderEvent;
/// fn read(event: ReaderEvent) {
///     match event {
///         ReaderEvent::Text(_) => {}
///         ReaderEvent::Block(_) => {}
///         ReaderEvent::BlockEnd(_) => {}
///     }
/// }
/// ```
///
/// ```compile_fail,E0004
/// use ag_ui_a2ui::agent::AfterRejectedBlock;
/// fn go_on(after: AfterRejectedBlock) -> bool {
///     match after {
///         AfterRejectedBlock::End => false,
///         AfterRejectedBlock::Continue => true,
///     }
/// }
/// ```
///
/// ```compile_fail,E0004
/// use ag_ui_a2ui::adapters::agui::SurfaceStatus;
/// fn name(status: &SurfaceStatus) -> &'static str {
///     match status {
///         SurfaceStatus::Building => "building",
///         SurfaceStatus::Retrying { .. } => "retrying",
///         SurfaceStatus::Failed { .. } => "failed",
///     }
/// }
/// ```
mod non_exhaustive {}
