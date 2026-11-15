//! What a test run shows in place of the conformance suite when the suite is
//! not compiled.
//!
//! The harness, `tests/conformance.rs`, needs the JSON Schema engine and, for
//! the suites under `agent/`, the agent layer, so it is compiled only with the
//! `schema-validation` and `agent` features. Without both a run would pass over
//! the suite without a word. This file is compiled exactly then, and its one
//! test is ignored with the reason, so the run lists it.

#![cfg(not(all(feature = "schema-validation", feature = "agent")))]

#[test]
#[ignore = "the conformance suite needs --features schema-validation,agent (or --all-features)"]
fn conformance_suite() {}
