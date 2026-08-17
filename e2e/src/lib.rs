//! Cross-crate end-to-end coverage for the AG-UI Rust SDK.
//!
//! The per-crate suites prove each half in isolation. This crate proves they
//! meet: an agent hosted by `ag-ui-server` behind `ag-ui-axum`, driven over
//! real HTTP by `ag-ui-client`, with nothing mocked between them.
//!
//! The live tier lives here too: [`gemini`] is an agent backed by a real
//! streaming model, served by the `gemini_agent` example and driven over HTTP by
//! `tests/live_gemini.rs`. It reaches the model with `reqwest` alone, which is
//! what makes it an architecture test as much as an integration one.
//!
//! Nothing here is published.
#![forbid(unsafe_code)]

pub mod gemini;
