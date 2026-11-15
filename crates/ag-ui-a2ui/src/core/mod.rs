//! The A2UI protocol itself, with no I/O.

pub mod binding;
pub mod catalog;
pub mod message;
pub mod model;
pub mod surface;
pub mod validate;

#[cfg(feature = "schema-validation")]
pub mod client_schema;
#[cfg(feature = "schema-validation")]
pub mod schema_validation;
