//! Compiles Rust snippets in the stable English and Korean guides.
//!
//! The website is outside Cargo's source tree, so this unpublished E2E crate
//! includes each current page as rustdoc input. Its Rust fences are
//! checked by `cargo test --doc`. Historical guides describe their corresponding
//! tags and are not compiled against the newer Rust API.
//!
//! Keep this explicit list aligned with the current guide tree. The website
//! build verifies that every current page with Rust snippets appears here.

/// Compiles one documentation page's Rust blocks under `cargo test --doc`.
///
/// `#[cfg(doctest)]` keeps the module out of every other build, including
/// `cargo doc` — which matters, because prose written for a browser is full of
/// bracketed text that rustdoc would otherwise try to resolve as intra-doc
/// links and, under the workspace's `RUSTDOCFLAGS: -D warnings`, fail on.
macro_rules! doc_page {
    ($name:ident, $path:literal) => {
        #[cfg(doctest)]
        #[doc = include_str!(concat!("../../website/src/content/docs/", $path))]
        mod $name {}
    };
}

doc_page!(start_index, "v1.0.0/start/index.md");
doc_page!(start_protocol, "v1.0.0/start/protocol.md");
doc_page!(start_crates, "v1.0.0/start/crates.md");

doc_page!(server_index, "v1.0.0/server/index.md");

doc_page!(server_agent, "v1.0.0/server/agent.md");
doc_page!(server_text, "v1.0.0/server/text.md");
doc_page!(server_tools, "v1.0.0/server/tools.md");
doc_page!(server_state, "v1.0.0/server/state.md");
doc_page!(server_interrupts, "v1.0.0/server/interrupts.md");
doc_page!(server_subagents, "v1.0.0/server/subagents.md");
doc_page!(server_errors, "v1.0.0/server/errors.md");
doc_page!(server_axum, "v1.0.0/server/axum.md");

doc_page!(client_index, "v1.0.0/client/index.md");
doc_page!(client_tools, "v1.0.0/client/tools.md");
doc_page!(client_state, "v1.0.0/client/state.md");
doc_page!(client_interrupts, "v1.0.0/client/interrupts.md");

doc_page!(client_thread, "v1.0.0/client/thread.md");
doc_page!(client_updates, "v1.0.0/client/updates.md");
doc_page!(client_rendering, "v1.0.0/client/rendering.md");
doc_page!(client_transports, "v1.0.0/client/transports.md");

doc_page!(a2ui_index, "v1.0.0/a2ui/index.md");
doc_page!(a2ui_authoring, "v1.0.0/a2ui/authoring.md");
doc_page!(a2ui_validation, "v1.0.0/a2ui/validation.md");

doc_page!(design_commitments, "v1.0.0/design/commitments.md");
doc_page!(design_verification, "v1.0.0/design/verification.md");
doc_page!(design_testing, "v1.0.0/design/testing.md");

doc_page!(reference_events, "v1.0.0/reference/events.md");
doc_page!(reference_features, "v1.0.0/reference/features.md");
doc_page!(reference_platforms, "v1.0.0/reference/platforms.md");

doc_page!(examples_task_board, "v1.0.0/examples/task-board.md");
doc_page!(examples_board_watch, "v1.0.0/examples/board-watch.md");

// The Korean translations mirror the English page list.
doc_page!(ko_start_index, "ko/v1.0.0/start/index.md");
doc_page!(ko_start_protocol, "ko/v1.0.0/start/protocol.md");
doc_page!(ko_start_crates, "ko/v1.0.0/start/crates.md");

doc_page!(ko_server_index, "ko/v1.0.0/server/index.md");

doc_page!(ko_server_agent, "ko/v1.0.0/server/agent.md");
doc_page!(ko_server_text, "ko/v1.0.0/server/text.md");
doc_page!(ko_server_tools, "ko/v1.0.0/server/tools.md");
doc_page!(ko_server_state, "ko/v1.0.0/server/state.md");
doc_page!(ko_server_interrupts, "ko/v1.0.0/server/interrupts.md");
doc_page!(ko_server_subagents, "ko/v1.0.0/server/subagents.md");
doc_page!(ko_server_errors, "ko/v1.0.0/server/errors.md");
doc_page!(ko_server_axum, "ko/v1.0.0/server/axum.md");

doc_page!(ko_client_index, "ko/v1.0.0/client/index.md");
doc_page!(ko_client_tools, "ko/v1.0.0/client/tools.md");
doc_page!(ko_client_state, "ko/v1.0.0/client/state.md");
doc_page!(ko_client_interrupts, "ko/v1.0.0/client/interrupts.md");

doc_page!(ko_client_thread, "ko/v1.0.0/client/thread.md");
doc_page!(ko_client_updates, "ko/v1.0.0/client/updates.md");
doc_page!(ko_client_rendering, "ko/v1.0.0/client/rendering.md");
doc_page!(ko_client_transports, "ko/v1.0.0/client/transports.md");

doc_page!(ko_a2ui_index, "ko/v1.0.0/a2ui/index.md");
doc_page!(ko_a2ui_authoring, "ko/v1.0.0/a2ui/authoring.md");
doc_page!(ko_a2ui_validation, "ko/v1.0.0/a2ui/validation.md");

doc_page!(ko_design_commitments, "ko/v1.0.0/design/commitments.md");
doc_page!(ko_design_verification, "ko/v1.0.0/design/verification.md");
doc_page!(ko_design_testing, "ko/v1.0.0/design/testing.md");

doc_page!(ko_reference_events, "ko/v1.0.0/reference/events.md");
doc_page!(ko_reference_features, "ko/v1.0.0/reference/features.md");
doc_page!(ko_reference_platforms, "ko/v1.0.0/reference/platforms.md");

doc_page!(ko_examples_task_board, "ko/v1.0.0/examples/task-board.md");
doc_page!(ko_examples_board_watch, "ko/v1.0.0/examples/board-watch.md");
doc_page!(server_detached_runs, "v1.0.0/server/detached-runs.md");
doc_page!(a2ui_agent, "v1.0.0/a2ui/agent.md");
doc_page!(a2ui_versions, "v1.0.0/a2ui/versions.md");
doc_page!(a2ui_functions, "v1.0.0/a2ui/functions.md");
doc_page!(ko_server_detached_runs, "ko/v1.0.0/server/detached-runs.md");
doc_page!(ko_a2ui_agent, "ko/v1.0.0/a2ui/agent.md");
doc_page!(ko_a2ui_versions, "ko/v1.0.0/a2ui/versions.md");
doc_page!(ko_a2ui_functions, "ko/v1.0.0/a2ui/functions.md");
