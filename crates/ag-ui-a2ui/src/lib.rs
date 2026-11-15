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
//! event loop, and a reactive data model. What this crate gives you instead:
//!
//! - [`core`] — the protocol itself, with no I/O. Its modules are also
//!   reachable at the crate root under the names they had before `core`
//!   existed, so `ag_ui_a2ui::validate` and `ag_ui_a2ui::core::validate` are
//!   the same module:
//!   - [`core::version`] and [`core::operation`] — which protocol versions and
//!     operations exist, each named in one place.
//!   - [`core::message`] — the v0.9-family protocol envelopes, in both
//!     directions.
//!   - [`core::catalog`] — what a surface may contain, including the standard
//!     18-component basic catalog.
//!   - [`core::surface`] — the surface state machine every stream of
//!     operations is folded through.
//!   - [`core::validate`] — the semantic checks JSON Schema cannot express:
//!     does every child reference resolve, is there a root, is the tree
//!     acyclic. Plus the two a generating model gets wrong often enough to be
//!     worth checking without a schema engine: the message envelope, and
//!     property values against the type the catalog declares.
//!   - [`core::binding`] and [`core::model`] — JSON Pointer resolution,
//!     template scopes, the `formatString` interpolation grammar and the data
//!     model, so an agent can check its own bindings before shipping them.
// Gate module links with their features so minimal rustdoc builds resolve them.
#![cfg_attr(
    feature = "toolkit",
    doc = "- [`toolkit`] (feature `toolkit`) — building ops, negotiating a catalog,",
    doc = "  assembling prompts, parsing a model's output as it streams, recovering a",
    doc = "  surface from conversation history, and the validate-and-retry loop around",
    doc = "  a generating model."
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
// See `ag_ui`'s lib.rs: marks feature-gated items in the rendered docs.
#![cfg_attr(docsrs, feature(doc_cfg))]

// Keep README examples covered by doctests.
#[cfg(doctest)]
#[doc = include_str!("../README.md")]
mod readme {}

pub mod constants;
pub mod core;
pub mod error;

// The protocol modules live in `core`. Their earlier paths at the crate root
// (`ag_ui_a2ui::validate`, `ag_ui_a2ui::catalog`, …) name the same modules, so
// code written against them keeps compiling.
pub use crate::core::{binding, catalog, message, model, surface, validate};
#[cfg(feature = "schema-validation")]
pub use crate::core::{client_schema, schema_validation};

#[cfg(feature = "author")]
pub mod author;
#[cfg(feature = "author")]
pub use author::{A2uiAuthor, AuthorRequest, ValidatedSurface};

#[cfg(feature = "toolkit")]
pub mod toolkit;

#[cfg(feature = "ag-ui")]
pub mod agui;

pub use crate::core::catalog::Catalog;
pub use crate::core::message::{
    AgentMessage, AgentPayload, ChildList, ChildTemplate, ClientDataModel, Component,
    DataModelUpdate, RendererMessage, RendererPayload,
};
pub use crate::core::model::{DataModel, ModelValue};
pub use crate::core::operation::Operation;
pub use crate::core::surface::{Lifecycle, SurfaceState, SurfaceStore};
pub use crate::core::validate::{
    ErrorCode, ValidateOptions, ValidationError, ValidationReport, Validator,
};
pub use crate::core::version::A2uiVersion;
pub use error::{Error, Result, ValidationErrors};

#[cfg(feature = "toolkit")]
pub use toolkit::{StreamParser, wrap_as_operations_envelope, wrap_error_envelope};

#[cfg(feature = "ag-ui")]
pub use agui::find_prior_surface_in;
