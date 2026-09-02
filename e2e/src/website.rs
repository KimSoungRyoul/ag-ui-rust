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
//!
//! The Korean pages are held to the same standard, for a sharper reason than
//! the English ones. A snippet in a translation is a copy, and a copy can drift
//! from its original without anyone reading the two files side by side.
//! Compiling it is the only check on that which stays honest.
//!
//! Those pages are listed here before they are written, and the stubs that make
//! that possible are a deliberate trade rather than a shortcut. `include_str!`
//! on a missing path is a compile error, so the alternative was to add the
//! Korean half of this list only once the translations landed — and an entry
//! that has to be added later is exactly the entry nobody notices is missing,
//! which is 26 pages of Rust with no gate at all, in the language least likely
//! to be re-read. The stub costs nothing on the other side of the trade:
//! Starlight drops `draft` pages from the content collection *before* it works
//! out which fallback routes a locale needs, so /ko/ serves the English page
//! either way. That was checked rather than reasoned about — the built page
//! list is identical with the stubs present and with them deleted, and none of
//! their text appears anywhere in `dist/`.

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
doc_page!(server_subagents, "server/subagents.md");
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

// The Korean translations. Every path mirrors the English list above, file
// extension included, so the two halves read as one table and a page that
// exists in one language but not the other is visible at a glance rather than
// only when someone goes looking.
doc_page!(ko_index, "ko/index.mdx");

doc_page!(ko_start_index, "ko/start/index.md");
doc_page!(ko_start_protocol, "ko/start/protocol.md");
doc_page!(ko_start_crates, "ko/start/crates.md");

doc_page!(ko_server_agent, "ko/server/agent.md");
doc_page!(ko_server_text, "ko/server/text.md");
doc_page!(ko_server_tools, "ko/server/tools.md");
doc_page!(ko_server_state, "ko/server/state.md");
doc_page!(ko_server_interrupts, "ko/server/interrupts.md");
doc_page!(ko_server_subagents, "ko/server/subagents.md");
doc_page!(ko_server_errors, "ko/server/errors.md");
doc_page!(ko_server_axum, "ko/server/axum.md");

doc_page!(ko_client_session, "ko/client/session.md");
doc_page!(ko_client_updates, "ko/client/updates.md");
doc_page!(ko_client_rendering, "ko/client/rendering.md");
doc_page!(ko_client_transports, "ko/client/transports.md");

doc_page!(ko_a2ui_index, "ko/a2ui/index.md");
doc_page!(ko_a2ui_authoring, "ko/a2ui/authoring.md");
doc_page!(ko_a2ui_validation, "ko/a2ui/validation.md");

doc_page!(ko_design_commitments, "ko/design/commitments.md");
doc_page!(ko_design_verification, "ko/design/verification.md");
doc_page!(ko_design_testing, "ko/design/testing.md");

doc_page!(ko_reference_events, "ko/reference/events.md");
doc_page!(ko_reference_features, "ko/reference/features.md");
doc_page!(ko_reference_platforms, "ko/reference/platforms.md");

doc_page!(ko_examples_task_board, "ko/examples/task-board.md");
doc_page!(ko_examples_board_watch, "ko/examples/board-watch.md");
