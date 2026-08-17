//! Cross-crate end-to-end coverage for the AG-UI Rust SDK.
//!
//! The per-crate suites prove each half in isolation. This crate proves they
//! meet: an agent hosted by `ag-ui-server` behind `ag-ui-axum`, driven over
//! real HTTP by `ag-ui-client`, with nothing mocked between them.
//!
//! The live tier lives here too: [`llm`] is an agent backed by a real streaming
//! model over the OpenAI-compatible `/chat/completions` format, served by the
//! `llm_agent` example and driven over HTTP by `tests/live_llm.rs`. It reaches
//! the model with `reqwest` alone, which is what makes it an architecture test
//! as much as an integration one.
//!
//! Nothing here is published.
#![forbid(unsafe_code)]

// The workspace README's quickstart is compiled here. No published crate can
// own it: `include_str!("../../README.md")` reaches outside the package
// directory and would break `cargo package`. This crate is unpublished and
// already depends on every other one, so it is the natural host — and it makes
// a stale quickstart a red build instead of a bad first impression.
#[cfg(doctest)]
#[doc = include_str!("../../README.md")]
mod workspace_readme {}

pub mod llm;
