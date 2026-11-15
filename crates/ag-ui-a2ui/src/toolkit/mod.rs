//! The conventions an agent hosted on AG-UI shares with its frontend (feature
//! `toolkit`).
//!
//! Generating A2UI is the [agent layer](crate::agent)'s: the prompt, reading
//! the model's answer, validation, the retry decision. What is here is what
//! goes around it when the surface travels as an AG-UI tool result:
//!
//! | Module | Job |
//! |---|---|
//! | [`ops`] | Build the operation stream of a surface from a spec, skipping `createSurface` on update. |
//! | [`envelope`] | Wrap operations as `{"a2ui_operations": [...]}`, or report a failure. |
//! | [`history`] | Recover a previously rendered surface from the conversation, so it can be edited. |
//! | [`tools`] | The `generate_a2ui` and `render_a2ui` tool definitions. |
//!
//! None of it is A2UI itself: the envelope key, the tool names and the way a
//! surface is found in a transcript are conventions of AG-UI's A2UI toolkit.
//! They are kept in one place so that the protocol modules and the agent layer
//! stay free of any transport.

pub mod envelope;
pub mod history;
pub mod ops;
pub mod tools;

pub use envelope::{wrap_as_operations_envelope, wrap_error_envelope};
pub use history::{
    HistoryMessage, PriorSurface, find_prior_surface, try_find_prior_surface,
    try_find_prior_surface_by_id,
};
pub use ops::{Intent, SurfaceSpec, assemble_ops};
pub use tools::{ToolDefinition, generate_a2ui_tool, render_a2ui_tool};
