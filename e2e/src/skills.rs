//! The agent skills' Rust snippets, compiled.
//!
//! `skills/` holds what an application developer's coding agent is told about
//! this SDK — installed as a Claude Code plugin, or written into a project by
//! `npx skills add`. A skill is markdown, so nothing in the Rust build would
//! otherwise look at it, and a snippet there could go stale the moment an API
//! changed. It would go stale *invisibly*, too: the reader is a model, and a
//! model handed a plausible-looking wrong signature does not stop to check it.
//!
//! Every page is therefore included here as module documentation, which makes
//! rustdoc extract its ```rust blocks and compile them exactly as it does the
//! ones in the README and on the documentation site. Same trick as
//! [`crate::website`], same reason, and the same restriction: `include_str!`
//! reaching outside the package directory would break `cargo package`, so it
//! lives in the crate that is never published.
//!
//! What this does *not* check is the prose. A skill can still describe a method
//! that no longer exists, as long as it does so in a sentence rather than in a
//! code block — which is why each skill directory carries a `sources.md` naming
//! the files its sections were written from, and why the skills state the
//! workspace version they were written against.
//!
//! Adding a skill does not add it here. The list is written out rather than
//! globbed, for the same reason the website's is: a page whose snippets are not
//! compiled has to be left off it on purpose.

/// Compiles one skill page's Rust blocks under `cargo test --doc`.
///
/// `#[cfg(doctest)]` keeps the module out of every other build, including
/// `cargo doc` — the YAML frontmatter and the prose are full of bracketed text
/// that rustdoc would otherwise try to resolve as intra-doc links and, under
/// the workspace's `RUSTDOCFLAGS: -D warnings`, fail on.
macro_rules! skill_page {
    ($name:ident, $path:literal) => {
        #[cfg(doctest)]
        #[doc = include_str!(concat!("../../skills/", $path))]
        mod $name {}
    };
}

skill_page!(server, "ag-ui-rust-server/SKILL.md");
skill_page!(
    server_state_and_interrupts,
    "ag-ui-rust-server/references/state-and-interrupts.md"
);

skill_page!(client, "ag-ui-rust-client/SKILL.md");
skill_page!(
    client_rendering,
    "ag-ui-rust-client/references/rendering.md"
);

// `ag-ui-rust-update` and `ag-ui-rust-qwen` carry no Rust, only shell. They
// are listed so the set of skills and the set of compiled pages stay visibly
// the same size.
skill_page!(update, "ag-ui-rust-update/SKILL.md");
skill_page!(qwen, "ag-ui-rust-qwen/SKILL.md");
