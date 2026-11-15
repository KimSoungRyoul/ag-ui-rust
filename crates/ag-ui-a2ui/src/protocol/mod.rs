//! The A2UI protocol itself, with no I/O.
//!
//! This module is private. It groups the files; each module below is public at
//! the crate root (`ag_ui_a2ui::validate`), and that is its only path. What the
//! modules are for, and what they may depend on, is in the crate documentation,
//! since nothing written here is rendered.

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
