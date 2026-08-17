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
//! - [`message`] — the ten protocol envelopes, in both directions.
//! - [`catalog`] — what a surface may contain, including the standard
//!   18-component basic catalog.
//! - [`validate`] — the semantic checks JSON Schema cannot express: does every
//!   child reference resolve, is there a root, is the tree acyclic. Plus the two
//!   a generating model gets wrong often enough to be worth checking without a
//!   schema engine: the message envelope, and property values against the type
//!   the catalog declares.
//! - [`binding`] — JSON Pointer resolution, template scopes, and the
//!   `formatString` interpolation grammar, so an agent can check its own
//!   bindings before shipping them.
//! - [`toolkit`] (feature `toolkit`) — building ops, negotiating a catalog,
//!   assembling prompts, parsing a model's output as it streams, recovering a
//!   surface from conversation history, and the validate-and-retry loop around
//!   a generating model.
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
//! Messages are stamped `v0.9`. The specification has moved on to v1.0, but the
//! shipping toolkits in every other language still speak v0.9 on the wire, and
//! interoperating with them matters more than tracking the newest revision. See
//! [`constants`] before changing anything there.
//!
//! # Example
//!
//! ```
//! use ag_ui_a2ui::{catalog::Catalog, message::Component, validate::Validator};
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

// The README is the crate's front page on crates.io, so its examples are
// doctested: a stale one is a red build rather than a bad first impression.
#[cfg(doctest)]
#[doc = include_str!("../README.md")]
mod readme {}

pub mod binding;
pub mod catalog;
pub mod constants;
pub mod error;
pub mod message;
pub mod validate;

#[cfg(feature = "ag-ui")]
pub mod agui;

#[cfg(feature = "toolkit")]
pub mod toolkit;

pub use error::{Error, Result, ValidationErrors};
