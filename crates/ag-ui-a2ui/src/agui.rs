//! AG-UI transport binding for A2UI (feature `ag-ui`).
//!
//! A2UI is transport-agnostic; this module is where it meets AG-UI specifically.
//! An agent emits A2UI operations inside an
//! [`A2UI_OPERATIONS_KEY`](crate::constants::A2UI_OPERATIONS_KEY) envelope, and
//! the AG-UI event stream carries that envelope to the frontend, which detects
//! it by that key and hands it to a renderer.
//!
//! Disable the `ag-ui` feature to use this crate standalone over A2A or MCP —
//! everything else here is transport-independent, and
//! [`crate::toolkit::envelope`] already produces the envelope as a plain JSON
//! string that any transport can carry.
//!
//! # Status: not implemented yet
//!
//! `ag-ui-core` is being written in parallel, so this module is a deliberate
//! placeholder rather than a guess at an API that does not exist. A wrong
//! binding would be worse than an absent one, because callers would build
//! against it.
//!
//! Planned surface, once `ag_ui_core`'s event and message types land:
//!
//! - detect an A2UI envelope inside an AG-UI assistant message or tool result;
//! - emit A2UI operations as AG-UI events;
//! - map a renderer `action` message back onto an AG-UI input event.

// TODO(integration-phase): implement the three items above against ag_ui_core.
