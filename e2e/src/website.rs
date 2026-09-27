//! Compiles Rust snippets in the current, versioned English and Korean guides.
//!
//! The website is outside Cargo's source tree, so this unpublished E2E crate
//! includes each 0.5.0-alpha.2 page as rustdoc input. Its Rust fences are
//! checked by `cargo test --doc`. The frozen 0.4.5 archive belongs to the
//! v0.4.5 tag and is not compiled against the newer Rust API.
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

doc_page!(start_index, "v0.5.0-alpha.2/start/index.md");
doc_page!(start_protocol, "v0.5.0-alpha.2/start/protocol.md");
doc_page!(start_crates, "v0.5.0-alpha.2/start/crates.md");

doc_page!(server_index, "v0.5.0-alpha.2/server/index.md");

doc_page!(server_agent, "v0.5.0-alpha.2/server/agent.md");
doc_page!(server_text, "v0.5.0-alpha.2/server/text.md");
doc_page!(server_tools, "v0.5.0-alpha.2/server/tools.md");
doc_page!(server_state, "v0.5.0-alpha.2/server/state.md");
doc_page!(server_interrupts, "v0.5.0-alpha.2/server/interrupts.md");
doc_page!(server_subagents, "v0.5.0-alpha.2/server/subagents.md");
doc_page!(server_errors, "v0.5.0-alpha.2/server/errors.md");
doc_page!(server_axum, "v0.5.0-alpha.2/server/axum.md");

doc_page!(client_index, "v0.5.0-alpha.2/client/index.md");
doc_page!(client_tools, "v0.5.0-alpha.2/client/tools.md");
doc_page!(client_state, "v0.5.0-alpha.2/client/state.md");
doc_page!(client_interrupts, "v0.5.0-alpha.2/client/interrupts.md");

doc_page!(client_thread, "v0.5.0-alpha.2/client/thread.md");
doc_page!(client_updates, "v0.5.0-alpha.2/client/updates.md");
doc_page!(client_rendering, "v0.5.0-alpha.2/client/rendering.md");
doc_page!(client_transports, "v0.5.0-alpha.2/client/transports.md");

doc_page!(a2ui_index, "v0.5.0-alpha.2/a2ui/index.md");
doc_page!(a2ui_authoring, "v0.5.0-alpha.2/a2ui/authoring.md");
doc_page!(a2ui_validation, "v0.5.0-alpha.2/a2ui/validation.md");

doc_page!(design_commitments, "v0.5.0-alpha.2/design/commitments.md");
doc_page!(design_verification, "v0.5.0-alpha.2/design/verification.md");
doc_page!(design_testing, "v0.5.0-alpha.2/design/testing.md");

doc_page!(reference_events, "v0.5.0-alpha.2/reference/events.md");
doc_page!(reference_features, "v0.5.0-alpha.2/reference/features.md");
doc_page!(reference_platforms, "v0.5.0-alpha.2/reference/platforms.md");

doc_page!(examples_task_board, "v0.5.0-alpha.2/examples/task-board.md");
doc_page!(
    examples_board_watch,
    "v0.5.0-alpha.2/examples/board-watch.md"
);

// The Korean translations mirror the English page list.
doc_page!(ko_start_index, "ko/v0.5.0-alpha.2/start/index.md");
doc_page!(ko_start_protocol, "ko/v0.5.0-alpha.2/start/protocol.md");
doc_page!(ko_start_crates, "ko/v0.5.0-alpha.2/start/crates.md");

doc_page!(ko_server_index, "ko/v0.5.0-alpha.2/server/index.md");

doc_page!(ko_server_agent, "ko/v0.5.0-alpha.2/server/agent.md");
doc_page!(ko_server_text, "ko/v0.5.0-alpha.2/server/text.md");
doc_page!(ko_server_tools, "ko/v0.5.0-alpha.2/server/tools.md");
doc_page!(ko_server_state, "ko/v0.5.0-alpha.2/server/state.md");
doc_page!(
    ko_server_interrupts,
    "ko/v0.5.0-alpha.2/server/interrupts.md"
);
doc_page!(ko_server_subagents, "ko/v0.5.0-alpha.2/server/subagents.md");
doc_page!(ko_server_errors, "ko/v0.5.0-alpha.2/server/errors.md");
doc_page!(ko_server_axum, "ko/v0.5.0-alpha.2/server/axum.md");

doc_page!(ko_client_index, "ko/v0.5.0-alpha.2/client/index.md");
doc_page!(ko_client_tools, "ko/v0.5.0-alpha.2/client/tools.md");
doc_page!(ko_client_state, "ko/v0.5.0-alpha.2/client/state.md");
doc_page!(
    ko_client_interrupts,
    "ko/v0.5.0-alpha.2/client/interrupts.md"
);

doc_page!(ko_client_thread, "ko/v0.5.0-alpha.2/client/thread.md");
doc_page!(ko_client_updates, "ko/v0.5.0-alpha.2/client/updates.md");
doc_page!(ko_client_rendering, "ko/v0.5.0-alpha.2/client/rendering.md");
doc_page!(
    ko_client_transports,
    "ko/v0.5.0-alpha.2/client/transports.md"
);

doc_page!(ko_a2ui_index, "ko/v0.5.0-alpha.2/a2ui/index.md");
doc_page!(ko_a2ui_authoring, "ko/v0.5.0-alpha.2/a2ui/authoring.md");
doc_page!(ko_a2ui_validation, "ko/v0.5.0-alpha.2/a2ui/validation.md");

doc_page!(
    ko_design_commitments,
    "ko/v0.5.0-alpha.2/design/commitments.md"
);
doc_page!(
    ko_design_verification,
    "ko/v0.5.0-alpha.2/design/verification.md"
);
doc_page!(ko_design_testing, "ko/v0.5.0-alpha.2/design/testing.md");

doc_page!(ko_reference_events, "ko/v0.5.0-alpha.2/reference/events.md");
doc_page!(
    ko_reference_features,
    "ko/v0.5.0-alpha.2/reference/features.md"
);
doc_page!(
    ko_reference_platforms,
    "ko/v0.5.0-alpha.2/reference/platforms.md"
);

doc_page!(
    ko_examples_task_board,
    "ko/v0.5.0-alpha.2/examples/task-board.md"
);
doc_page!(
    ko_examples_board_watch,
    "ko/v0.5.0-alpha.2/examples/board-watch.md"
);
