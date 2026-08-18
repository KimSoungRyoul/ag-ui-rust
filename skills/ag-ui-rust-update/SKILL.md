---
name: ag-ui-rust-update
description: "Use when the ag-ui-rust SKILLS themselves need refreshing — not the SDK, not the user's project. The skills carry the workspace version they were written against, so they go stale silently when the SDK moves: a name that no longer exists, an API that changed shape, an example that no longer compiles. Triggers on: 'update ag-ui-rust skills', 'refresh skills', 'skills are stale/outdated', 'wrong API name', 'that method does not exist on RunContext', 'no such function in ag_ui_client', 'ag-ui-rust skill seems old', 'reinstall skills', a compile error that contradicts what the skill said."
---

# Refreshing the ag-ui-rust skills

The `ag-ui-rust-server` and `ag-ui-rust-client` skills each state the workspace version they
were written against. When the SDK has moved past it, refresh before trusting them.

## Check first

```sh
# What the project actually depends on.
cargo tree -p ag-ui-server --depth 0
grep -n 'ag-ui-' Cargo.toml
```

Compare that against the version line near the top of the skill. A mismatch is not proof of
staleness — the crates are a git dependency and most changes are additive — but it is the
signal to prefer `cargo doc --open` and the source over the skill's prose.

## Refresh

Two channels, one source. Pick the one this project installed from.

**Claude Code plugin** (namespaced `ag-ui-rust:…`, updates in place):

```text
/plugin update ag-ui-rust@ag-ui-rust
```

**The standalone installer** (Claude Code, Codex, Cursor, OpenCode and others; writes into
the project, or `-g` for the user directory):

```sh
npx skills add KimSoungRyoul/ag-ui-rust -y
```

Both pull from `skills/` in <https://github.com/KimSoungRyoul/ag-ui-rust>, so they deliver
the same files. **Start a new session afterwards** — a loaded skill is not re-read mid-run.

## When the skill is right and the compiler disagrees

The compiler wins, always. Then:

1. Read the matching page under <https://kimsoungryoul.github.io/ag-ui-rust/> — the site is
   built from the same commit as the crates and every Rust block on it is compiled by the
   test suite, so it cannot be stale in the way a skill can.
2. Read `sources.md` in the skill's own directory. It lists the repository files each section
   was written from; that is where to look, and what to re-read.
3. If the skill is wrong rather than merely old, that is a bug in this repository — the
   skills live in `skills/` alongside the crates, and their snippets are compiled by
   `e2e/src/skills.rs`. A snippet that drifts should already have been a red build, so a
   wrong one means something was written outside a compiled block. Worth an issue at
   <https://github.com/KimSoungRyoul/ag-ui-rust/issues>.
