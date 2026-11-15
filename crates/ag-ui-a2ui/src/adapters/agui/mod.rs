//! A2UI over [`ag_ui`](https://kimsoungryoul.github.io/ag-ui-rust/api/ag_ui/index.html)
//! (feature `ag-ui`).
//!
//! A2UI is transport-agnostic and the rest of this crate keeps it that way.
//! This module is the one place that knows what AG-UI is. It follows AG-UI's
//! own A2UI middleware and toolkit, so that a frontend which paints behind
//! the TypeScript middleware paints from a Rust agent with no middleware in
//! front: the same tool names and arguments, the same envelope, the same
//! activity.
//!
//! Read from `ag-ui-protocol/ag-ui` at commit
//! `2330368a2a74573902858867072406638ba3073d`: `middlewares/a2ui-middleware`
//! (0.0.16) and `sdks/typescript/packages/a2ui-toolkit`.
//!
//! | Piece | What it is |
//! |---|---|
//! | [`wrap_as_operations_envelope`], [`error_envelope`], [`recovery_exhausted_envelope`] | What a tool result carries: a batch of operations, or a failure. |
//! | [`SurfacePainter`], [`SurfacePainter::status`] | The `a2ui-surface` activity a frontend paints from, and a surface's life before it is painted. |
//! | [`operations_in`], [`surfaces_in`], [`latest_surface`] | Reading surfaces back out of a conversation. |
//! | [`user_action`], [`catalog_context`] | What a request says: an action on a surface, the frontend's catalog. |
//! | [`generate_a2ui_tool`], [`render_a2ui_tool`], [`render_a2ui_tool_for`] | The two tools, with their arguments. |
//! | [`SurfaceSpec`], [`assemble_ops`] | The operations of a surface built in code. |
//! | [`basic_catalog`], [`basic_catalog_id`] | The basic catalog as an AG-UI frontend registers it, which is what a spec that names no catalog is drawn from. |
//!
//! # Protocol 1.0
//!
//! The frontend's catalog decides the protocol of a run, as it does for
//! AG-UI's middleware: [`CatalogContext::protocol_version`] is v1.0 when the
//! catalog the frontend sends declares it, and v0.9 otherwise. With that
//! version, [`render_a2ui_tool_for`] tells the model which protocol it
//! writes, [`SurfaceSpec::with_version`] stamps the operations, and
//! [`CatalogContext::capabilities`] negotiates for it. Nothing else changes
//! on this wire: the envelope, the activity and the tool's arguments are the
//! same, and a surface is still created, drawn and filled by three
//! operations (`a2ui-middleware/src/index.ts:711-730`).
//!
//! # What a client receives
//!
//! For one batch of operations, in this order:
//!
//! ```text
//! TOOL_CALL_START   render_a2ui (or the tool the model called)
//! TOOL_CALL_ARGS    {"surfaceId": …, "components": […], "data": {…}}
//! TOOL_CALL_END
//! TOOL_CALL_RESULT  {"a2ui_operations": [the batch]}
//! ACTIVITY_SNAPSHOT a2ui-surface, messageId a2ui-surface-<tool call id>, replace: true,
//!                   content {"a2ui_operations": [the whole surface]}
//! ```
//!
//! The tool result is the model's record and the next request's. The activity
//! is the frontend's: it is replayed on its own, so it always carries the
//! surface's creation, every component and the data, whatever the batch sent.
//! An update later in the same run replaces the activity in place. An update
//! in a later run arrives as a second activity, under that run's tool call,
//! again carrying the whole surface; the first one stays in the transcript as
//! what was shown then.
//!
//! A call may show where its surface is before it is painted:
//! [`SurfacePainter::status`] gives the same activity with
//! `{"status": "building"}`, `retrying` or `failed` for content. The paint of
//! that call then goes in that activity, so the skeleton gives way to the
//! surface. [`SurfacePainter`] has the rules for which activity a paint goes
//! under, with the lines of the middleware they follow.
//!
//! A `deleteSurface` for a surface that is not live does nothing on any
//! layer: [`MessageProcessor`](crate::processor::MessageProcessor) accepts it
//! and changes nothing, the agent layer returns it among an answer's
//! messages, and here it is passed over when a batch is painted and when a
//! conversation is read back.
//!
//! Turn the feature off to use A2UI over A2A or MCP; the dependency on
//! `ag-ui` goes with it.
//!
//! # A turn on a server
//!
#![cfg_attr(feature = "ag-ui-server", doc = "```")]
#![cfg_attr(not(feature = "ag-ui-server"), doc = "```ignore")]
//! use ag_ui::server::{Result, RunContext};
//! use ag_ui_a2ui::adapters::agui::{Intent, SurfacePainter, SurfaceSpec, assemble_ops};
//! use ag_ui_a2ui::Component;
//! use serde_json::json;
//!
//! fn show_cart(ctx: &mut RunContext<()>, total: &str) -> Result<()> {
//!     // What the frontend holds is in the request: the agent keeps nothing.
//!     let mut painter =
//!         SurfacePainter::from_history(ctx.messages()).map_err(ag_ui::server::Error::agent)?;
//!     let on_screen = painter.surfaces().get("cart").is_some_and(|cart| !cart.deleted);
//!
//!     let spec = SurfaceSpec::new("cart")
//!         .with_components(vec![Component::new("root", "Text").with("text", json!({"path": "/total"}))])
//!         .with_data_model(json!({"total": total}));
//!     let intent = if on_screen { Intent::Update } else { Intent::Create };
//!     painter.paint(ctx, &assemble_ops(intent, &spec))?;
//!     Ok(())
//! }
//! ```

mod activity;
mod envelope;
mod history;
mod input;
mod ops;
mod tools;

pub use activity::{
    Painted, SURFACE_ACTIVITY_TYPE, SurfacePainter, SurfaceStatus, surface_activity_id,
};
pub use envelope::{
    Attempt, RECOVERY_EXHAUSTED_CODE, error_envelope, is_operations_envelope, operations_envelope,
    recovery_exhausted_envelope, unwrap_operations_envelope, wrap_as_operations_envelope,
};
pub use history::{latest_surface, operations_in, surfaces_in};
pub use input::{
    CatalogContext, SCHEMA_CONTEXT_DESCRIPTION, UserAction, catalog_context, user_action,
};
pub use ops::{
    Intent, SurfaceSpec, assemble_ops, basic_catalog, basic_catalog_id, create_surface,
    update_components, update_data_model,
};
pub use tools::{
    GenerateArgs, RenderArgs, generate_a2ui_tool, render_a2ui_tool, render_a2ui_tool_for,
};
