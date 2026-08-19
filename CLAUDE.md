# ag-ui-rust

## What this is

A Rust SDK for the AG-UI protocol — hosting an agent and consuming one, in a single workspace.

It exists to replace the upstream
[community Rust SDK](https://github.com/ag-ui-protocol/ag-ui/tree/main/sdks/community/rust):
that one consumes an agent but cannot host one, declares 24 of the protocol's 33 event types —
an unrecognised `type` fails to deserialize and stops the run rather than being skipped — and
carries no `RunFinished.outcome`, so a run cannot pause for a human at all.

The server story is the priority here; the client is written against the same types, and `e2e/` proves
the two halves against each other over a real port rather than against a mock.

## Crate layout

Two published crates. **`ag-ui`** is the SDK: the protocol types at the crate root, always
compiled, with `server` (host an agent), `client` / `http` (consume one) and `axum` behind
features. Each runtime keeps its own `Error` under its own module — `ag_ui::Error` is a
protocol error, `ag_ui::server::Error` is a hosting error. **`ag-ui-a2ui`** stays separate
because A2UI is a different protocol, usable with no AG-UI at all.

Features rather than crates because in Rust features are how a dependency is kept out of a
build, and this workspace versions in lockstep so a split bought no independent versioning
either — `docs/DESIGN.md`, "Crate boundaries", has the reasoning. The one cost is feature
unification: a graph that wants `server` in one place and `client` in another compiles both.

`ag-ui-core`, `ag-ui-server` and `ag-ui-client` on crates.io are an earlier, unrelated
community SDK. Do not write them as dependency lines for this project.

## Links

| What | Where |
| --- | --- |
| AG-UI protocol docs | <https://docs.ag-ui.com> |
| AG-UI spec repository | <https://github.com/ag-ui-protocol/ag-ui> |
| The community Rust SDK this replaces | <https://github.com/ag-ui-protocol/ag-ui/tree/main/sdks/community/rust> |
| Why it is not enough, with the numbers | `docs/DESIGN.md` — "Why another Rust AG-UI SDK" |
| This project's documentation | <https://kimsoungryoul.github.io/ag-ui-rust/> |
| rustdoc for both crates | <https://kimsoungryoul.github.io/ag-ui-rust/api/ag_ui/index.html> |
| A2UI, the surface protocol `ag-ui-a2ui` speaks | <https://a2ui.org> |
| Agent skills this repo publishes | `skills/`, installed per the README |

## Working here

Two test commands, and the second is not optional — **`cargo nextest` never sees doctests**, and a
lot of what this workspace proves lives in them (every crate README, the README quickstart, the
website's pages, the skills, and the `compile_fail` example that is the only executable proof that
two overlapping message handles do not compile):

```sh
cargo nextest run --workspace --all-features
cargo test --doc --workspace --all-features
```

Hygiene is `prek run --all-files`, which is what CI runs. The rest — the design rationale, the
upstream analysis, the spec drift check — is in `README.md` and `docs/`.
