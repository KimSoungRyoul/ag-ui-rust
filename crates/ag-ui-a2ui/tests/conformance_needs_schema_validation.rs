//! What a test run shows in place of the conformance suite when the suite is
//! not compiled.
//!
//! The harness, `tests/conformance.rs`, needs the JSON Schema engine, so it is
//! compiled only with the `schema-validation` feature. Without that feature a
//! run would pass over the suite without a word. This file is compiled exactly
//! then, and its one test is ignored with the reason, so the run lists it.

#![cfg(not(feature = "schema-validation"))]

#[test]
#[ignore = "the conformance suite needs --features schema-validation (or --all-features)"]
fn conformance_suite() {}
