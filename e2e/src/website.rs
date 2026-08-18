//! The documentation site's Rust snippets, compiled.
//!
//! `website/` is an Astro site, so nothing in the Rust build would otherwise
//! look at it, and a snippet there could go stale the moment an API changed —
//! silently, and on the page a newcomer reads first. Every page that carries
//! Rust is therefore included here as module documentation, which makes
//! rustdoc extract its ```rust blocks and compile them exactly as it does the
//! ones in `lib.rs`.
//!
//! This is the same trick the workspace README already uses, and for the same
//! reason: `include_str!` reaching outside the package directory would break
//! `cargo package`, and this crate is the one that is never published.
//!
//! Everything around the code blocks — YAML frontmatter, `import` statements,
//! `:::note` directives, JSX components — is prose to rustdoc and passes
//! through untouched. Only the fenced Rust blocks are compiled. A block that
//! must not be run (it binds a port, or reaches the network) is marked
//! ```rust,no_run and is still type-checked; `ignore` is a last resort,
//! because an ignored block is a snippet with no gate at all.
//!
//! Adding a page to the site does not add it here. That is deliberate: the
//! list is what a reader can trust, so it is written out rather than globbed,
//! and a page whose snippets are not compiled has to be left off it on
//! purpose.

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

doc_page!(index, "index.mdx");

doc_page!(start_index, "start/index.md");
doc_page!(start_protocol, "start/protocol.md");
doc_page!(start_crates, "start/crates.md");

doc_page!(server_agent, "server/agent.md");
doc_page!(server_text, "server/text.md");
doc_page!(server_tools, "server/tools.md");
doc_page!(server_state, "server/state.md");
doc_page!(server_interrupts, "server/interrupts.md");
doc_page!(server_errors, "server/errors.md");
doc_page!(server_axum, "server/axum.md");

doc_page!(client_session, "client/session.md");
doc_page!(client_updates, "client/updates.md");
doc_page!(client_rendering, "client/rendering.md");
doc_page!(client_transports, "client/transports.md");

doc_page!(a2ui_index, "a2ui/index.md");
doc_page!(a2ui_authoring, "a2ui/authoring.md");
doc_page!(a2ui_validation, "a2ui/validation.md");

doc_page!(design_commitments, "design/commitments.md");
doc_page!(design_verification, "design/verification.md");
doc_page!(design_testing, "design/testing.md");

doc_page!(reference_events, "reference/events.md");
doc_page!(reference_features, "reference/features.md");
doc_page!(reference_platforms, "reference/platforms.md");

doc_page!(examples_task_board, "examples/task-board.md");
doc_page!(examples_board_watch, "examples/board-watch.md");
