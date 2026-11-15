//! What a test run shows in place of the conformance suite when the suite is
//! not compiled.
//!
//! The harness, `tests/conformance.rs`, needs the JSON Schema engine, for the
//! suites under `agent/` the agent layer, and for the A2A suite its adapter,
//! so it is compiled only with the `schema-validation`, `agent` and `a2a`
//! features. Without them a run would pass over the suite without a word. This
//! file is compiled exactly then, and its one test is ignored with the reason,
//! so the run lists it.

#![cfg(not(all(feature = "schema-validation", feature = "agent", feature = "a2a")))]

#[test]
#[ignore = "the conformance suite needs --features schema-validation,agent,a2a (or --all-features)"]
fn conformance_suite() {}
