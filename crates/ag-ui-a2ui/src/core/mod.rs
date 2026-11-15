//! The A2UI protocol itself, with no I/O.
//!
//! Everything here is a pure function of the messages it is given: no model
//! call, no transport, no clock. An agent framework, a transport adapter and a
//! test all use the same pieces, and each fact about the protocol is stated in
//! exactly one of them.
//!
//! | Module | What it is the one place for |
//! |---|---|
//! | [`version`] | Which protocol versions exist, how each is spelled, and what differs between them. |
//! | [`operation`] | The names of the agent → renderer operations. |
//! | [`message`] | The wire types, in both directions. |
//! | [`catalog`] | What a surface may contain: components, their properties, functions. |
//! | [`binding`] | JSON Pointer resolution, template scopes and `formatString`. |
//! | [`model`] | The data model, including the undefined slots JSON cannot express. |
//! | [`surface`] | The surface state machine: lifecycle, and folding a stream of operations. |
//! | [`validate`] | Semantic validation: the envelope, the component graph, property types, bindings. |
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
//! # What depends on what
//!
//! Nothing in `core` reaches upward into the agent layer or an adapter, with
//! one exception that is on its way out: the JSON Schema validator takes its
//! three schema documents as a `toolkit::schema::SchemaBundle`, so the
//! `schema-validation` feature still implies `toolkit`.
//!
//! With default features off, `core` is all there is, and it needs only
//! `serde`, `serde_json`, `thiserror` and `jsonptr`.

pub mod binding;
pub mod catalog;
pub(crate) mod graph;
pub mod message;
pub mod model;
pub mod operation;
pub mod surface;
pub mod validate;
pub mod version;

#[cfg(feature = "schema-validation")]
pub mod client_schema;
#[cfg(feature = "schema-validation")]
pub mod schema_validation;
