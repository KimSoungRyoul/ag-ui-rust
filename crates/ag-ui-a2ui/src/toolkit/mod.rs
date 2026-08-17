//! Agent-side authoring: producing A2UI, not rendering it (feature `toolkit`).
//!
//! Everything an agent needs between "the user asked for a UI" and "valid A2UI
//! is on the wire":
//!
//! | Module | Job |
//! |---|---|
//! | [`ops`] | Build the operation stream, skipping `createSurface` on update. |
//! | [`envelope`] | Wrap operations for transport, or report a failure. |
//! | [`prompt`] | Assemble the generating model's prompt from catalog, context, and current surface. |
//! | [`parser`] | Pull A2UI blocks back out of the model's response. |
//! | [`history`] | Recover a previously rendered surface so it can be edited. |
//! | [`recovery`] | Validate, feed the errors back, retry — up to three times. |
//! | [`tools`] | The `generate_a2ui` and `render_a2ui` tool definitions. |
//!
//! # The loop these compose into
//!
//! ```text
//!   history::find_prior_surface  ─┐
//!   catalog::Catalog             ─┼─▶ prompt::build_subagent_prompt
//!   the user's request           ─┘             │
//!                                               ▼
//!                                        the model
//!                                               │
//!                                  parser::parse_response
//!                                               │
//!                                    validate::Validator
//!                                     ┌─────────┴─────────┐
//!                                  valid              invalid
//!                                     │                   │
//!                          ops::assemble_ops   prompt::augment_prompt_with_errors
//!                                     │                   │
//!                 envelope::wrap_as_operations_envelope   retry (max 3)
//! ```
//!
//! [`recovery::generate_with_recovery`] runs the middle of that diagram for you;
//! the rest is there so you can assemble a different shape if your agent needs
//! one.

pub mod envelope;
pub mod history;
pub mod ops;
pub mod parser;
pub mod prompt;
pub mod recovery;
pub mod tools;

pub use envelope::{wrap_as_operations_envelope, wrap_error_envelope};
pub use history::{HistoryMessage, PriorSurface, find_prior_surface};
pub use ops::{Intent, SurfaceSpec, assemble_ops};
pub use recovery::{RecoveredSurface, RecoveryActivity, RecoveryOptions, generate_with_recovery};
pub use tools::{ToolDefinition, generate_a2ui_tool, render_a2ui_tool};
