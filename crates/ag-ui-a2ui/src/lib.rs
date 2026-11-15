//! A2UI protocol types, semantic validator, and agent-side authoring toolkit.
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
//! | [`message`] | The v0.9-family wire types, in both directions. |
//! | [`catalog`] | What a surface may contain: components, their properties, functions. Includes the standard 18-component basic catalog. |
//! | [`binding`] | JSON Pointer resolution, template scopes and the `formatString` interpolation grammar, so an agent can check its own bindings before shipping them. |
//! | [`model`] | The data model, including the undefined slots JSON cannot express. |
//! | [`surface`] | The surface state machine: lifecycle, and folding a stream of operations. Every stream is folded through it. |
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
//! cycles and depth for the validator and for the streaming parser alike.
//!
//! ## What depends on what
//!
//! The protocol modules reach into nothing above them. The `schema-validation`
//! feature still turns `toolkit` on, for the callers that came to rely on it;
//! nothing in the validator needs it.
//!
//! With default features off, the protocol modules are all there is, and they
//! need only `serde`, `serde_json`, `thiserror` and `jsonptr`.
//!
//! # Around the protocol
//!
#![cfg_attr(
    feature = "agent",
    doc = "- [`agent`] (feature `agent`) — what an agent framework delegates A2UI to:",
    doc = "  catalog negotiation, the prompt snippet, reading a model's answer whole or",
    doc = "  streamed, the mirror of the renderer's state, and the retry decision. The",
    doc = "  framework makes the model call; nothing in the layer does I/O."
)]
#![cfg_attr(
    not(feature = "agent"),
    doc = "- `agent` (feature `agent`, off in this build) — what an agent framework",
    doc = "  delegates A2UI to: negotiation, prompt snippet, answer parsing, renderer",
    doc = "  state and the retry decision, with no model call and no I/O."
)]
#![cfg_attr(
    feature = "toolkit",
    doc = "- [`toolkit`] (feature `toolkit`) — building ops, negotiating a catalog,",
    doc = "  assembling prompts, parsing a model's output as it streams, recovering a",
    doc = "  surface from conversation history, and the validate-and-retry loop around",
    doc = "  a generating model."
)]
#![cfg_attr(
    not(feature = "toolkit"),
    doc = "- `toolkit` (feature `toolkit`, off in this build) — building ops, prompts,",
    doc = "  parsing a model's output, and the validate-and-retry loop."
)]
#![cfg_attr(
    feature = "ag-ui",
    doc = "- [`agui`] (feature `ag-ui`) — the glue for an agent hosted on AG-UI:",
    doc = "  history entries from [`ag_ui::Message`](https://kimsoungryoul.github.io/ag-ui-rust/api/ag_ui/message/enum.Message.html), toolkit tool definitions as",
    doc = "  offerable [`ag_ui::Tool`](https://kimsoungryoul.github.io/ag-ui-rust/api/ag_ui/tool/struct.Tool.html)s."
)]
//!
//! # Transport
//!
//! A2UI says nothing about how messages reach the renderer, and everything
//! outside one module keeps it that way — turn the `ag-ui` feature off and the
//! dependency goes with it, leaving a crate you can drive over A2A or MCP. That
//! module is
#![cfg_attr(feature = "ag-ui", doc = "[`agui`].")]
#![cfg_attr(
    not(feature = "ag-ui"),
    doc = "`agui`, and this build does not have it."
)]
//! The optional AG-UI integration wraps a batch of operations in a
//! `{"a2ui_operations": [...]}` envelope and let the frontend sniff for that
//! key.
#![cfg_attr(
    feature = "toolkit",
    doc = "[`toolkit::envelope`] produces exactly that, as a plain JSON string that",
    doc = "fits in an AG-UI assistant message, an A2A data part, or an MCP tool result",
    doc = "without further wrapping."
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
//! Supports `v0.9` and `v0.9.1`. Low-level constructors retain `v0.9` as
//! their default; use [`A2uiVersion`] to select a discriminator. It is the one
//! place a version is parsed or named, and what differs between versions is a
//! method on it. Candidate RPC payloads are not accepted or serialized as
//! v0.9-family messages.
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
    binding, catalog, message, model, operation, schema, surface, validate, version,
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

#[cfg(feature = "toolkit")]
pub mod toolkit;

#[cfg(feature = "ag-ui")]
pub mod agui;

pub use crate::protocol::catalog::Catalog;
pub use crate::protocol::message::{
    AgentMessage, AgentPayload, ChildList, ChildTemplate, ClientDataModel, Component,
    DataModelUpdate, RendererMessage, RendererPayload,
};
pub use crate::protocol::model::{DataModel, ModelValue};
pub use crate::protocol::operation::Operation;
pub use crate::protocol::surface::{Lifecycle, SurfaceState, SurfaceStore};
pub use crate::protocol::validate::{
    ErrorCode, ValidateOptions, ValidationError, ValidationReport, Validator,
};
pub use crate::protocol::version::A2uiVersion;
pub use error::{Error, Result, ValidationErrors};

#[cfg(feature = "toolkit")]
pub use toolkit::{StreamParser, wrap_as_operations_envelope, wrap_error_envelope};

#[cfg(feature = "ag-ui")]
pub use agui::find_prior_surface_in;
