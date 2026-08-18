---
title: Platforms and MSRV
description: Minimum supported Rust version, the targets CI builds for, the wasm story, and how the executor-agnostic commitment is actually enforced.
---

## MSRV is 1.85, and there is no slack in it

The workspace declares `rust-version = "1.85"` and `edition = "2024"`. Those two are the same
fact: edition 2024 stabilised in exactly 1.85, so 1.85 is not a cautious floor with room below
it — it is the first compiler that can build this code at all.

That has a practical consequence. Ordinarily an MSRV is a promise you can loosen if it becomes
inconvenient; here you cannot lower it by one release without changing the edition, which is a
different and much larger change. If you are pinned below 1.85, this SDK is not usable, and no
feature flag changes that.

CI holds the line with a job that installs exactly that toolchain:

```sh
# .github/workflows/ci.yml, job `msrv`
cargo check --workspace --all-features --all-targets
```

`--all-features` matters, because a feature is a way to accidentally require a newer compiler
without noticing. `--all-targets` matters for the same reason one step down: tests and examples
are code too.

The repository ships no `rust-toolchain.toml`. Local builds use whatever toolchain you have,
which is deliberate — the pin belongs in CI, where it is a check, rather than in the working
tree, where it would silently downgrade every contributor's compiler.

## What CI builds, and for what

| Target | Scope | Checked how |
| --- | --- | --- |
| host (`ubuntu-latest`) | the whole workspace | `cargo test --workspace --all-features`, plus clippy at `-D warnings` |
| host, Rust 1.85 | the whole workspace | `cargo check --workspace --all-features --all-targets` |
| `wasm32-unknown-unknown` | four crates, see below | `cargo check --target wasm32-unknown-unknown` |

The wasm row is five checks, and the feature set on each is part of what is being claimed:

```sh
# .github/workflows/ci.yml, job `executor-agnostic`
cargo check -p ag-ui-core   --target wasm32-unknown-unknown --no-default-features
cargo check -p ag-ui-core   --target wasm32-unknown-unknown --all-features
cargo check -p ag-ui-server --target wasm32-unknown-unknown --all-features
cargo check -p ag-ui-client --target wasm32-unknown-unknown --no-default-features
cargo check -p ag-ui-a2ui   --target wasm32-unknown-unknown --all-features
```

`ag-ui-client` appears with `--no-default-features` because the default `http` feature pulls
`reqwest`, which is not a wasm story this workspace tells. `ag-ui-axum` does not appear at all,
and that is not an oversight: it is the web binding — axum, tower, and tokio, running a
server — and nothing about it is meant to run in a browser.

These are `cargo check`, not a test run. They prove the crates *compile* for the target with no
native-only assumptions; they do not prove anything executes in a browser, because nothing in
this repository runs a headless one. Read it as "the types and the dependency graph are
wasm-clean", not as "verified in a browser".

## Executor-agnostic below the web binding

`ag-ui-core`, `ag-ui-server`, and `ag-ui-client` use `futures` primitives rather than tokio's.
The emit path is the clearest case: an emitter handle pushes into a
`futures_channel::mpsc::UnboundedSender` and the transport layer drains it, where the obvious
alternative would have been `tokio::sync::mpsc`. tokio enters the workspace at `ag-ui-axum` and
nowhere else.

This is what keeps a non-tokio executor — or a browser — a viable host. It is also why the emit
path is synchronous end to end: a handle emits its terminating event on `Drop`, `Drop` cannot
be async, so a handle cannot `await` while emitting.

`ag-ui-client` is executor-agnostic **only with the `http` feature off**. `http` pulls
`reqwest`, and `reqwest` pulls tokio. That is a deliberate default, not an accident: most
consumers want the HTTP transport and are already on tokio. Everything else in the crate —
event application, normalisation, verification — is a plain synchronous state machine, so
turning `http` off leaves a fully usable client with one hole where the transport goes.

The hole is a trait —
[`Transport`](/ag-ui-rust/api/ag_ui_client/transport/trait.Transport.html) — and you fill it:

```rust
use ag_ui_client::transport::{Transport, TransportFuture, boxed_stream};
use ag_ui_client::Result;
use ag_ui_core::{Event, RunAgentInput};
use futures_util::stream;

/// Replays a fixed script — the shape a browser transport built on `fetch`
/// and `EventSource` would also take.
struct Canned(Vec<Event>);

impl Transport for Canned {
    // Failing to connect is an error from the future; failing mid-stream is an
    // error item in the stream. That split is the whole interface.
    fn run(&self, _input: RunAgentInput) -> TransportFuture {
        let events: Vec<Result<Event>> = self.0.iter().cloned().map(Ok).collect();
        Box::pin(async move { Ok(boxed_stream(stream::iter(events))) })
    }
}
```

Two small wasm accommodations live in that module. `EventStream` and `TransportFuture` are
`Send` everywhere *except* wasm, where the browser APIs a transport would be built on are
single-threaded and not `Send` at all — requiring it there would make the wasm case, the reason
the transport is abstracted in the first place, impossible to satisfy. `boxed_stream` has the
matching pair of signatures.

Nothing in this workspace ships a browser transport. The trait and the cfgs are the
accommodation; the `fetch`-based implementation is yours to write.

## How the tokio ban is actually enforced

The wasm build does **not** prove the tokio ban, and CI does not pretend it does. tokio's `rt`,
`sync`, `macros`, `io-util`, and `time` features all compile for `wasm32-unknown-unknown`. The
CI comment records the experiment: adding `tokio.workspace = true` to `ag-ui-server`'s
`[dependencies]` passed every wasm check above. The build stayed green.

So the guarantee is carried by the dependency graph, and CI asserts on the graph directly:

```sh
# .github/workflows/ci.yml, job `executor-agnostic`
cargo tree -p ag-ui-core   -e normal --prefix none --no-dedupe --all-features
cargo tree -p ag-ui-server -e normal --prefix none --no-dedupe --all-features
cargo tree -p ag-ui-a2ui   -e normal --prefix none --no-dedupe --all-features
cargo tree -p ag-ui-client -e normal --prefix none --no-dedupe --no-default-features
```

Each tree is searched for a line beginning `tokio v`, and a hit fails the job with a message
pointing at the design decision rather than at the grep.

Three details in those four lines:

`-e normal` excludes dev-dependencies. Tests may use tokio freely, and they do —
`ag-ui-server`'s `[dev-dependencies]` pull it in for `#[tokio::test]`. What the commitment is
about is what a *consumer* of the crate gets, and that is the normal graph.

`ag-ui-client` is checked with `--no-default-features`, because with `http` on the assertion
would simply be false. The scope of the check is exactly the scope of the claim.

The script avoids `grep -q`. Closing the pipe early would `SIGPIPE` `cargo tree` and, under
`pipefail`, turn a real hit into a silent pass — so it prints the whole tree and greps without
`-q`.

You can run the same check locally:

```sh
cargo tree -p ag-ui-server --all-features -e normal --prefix none --no-dedupe | grep '^tokio v'
```

On this workspace that prints nothing: `ag-ui-server`'s normal graph is 26 crates including
itself, and none of them is tokio. The same command against `ag-ui-client --all-features`
prints `tokio v1.53.1`, reached through `reqwest`; against `ag-ui-client
--no-default-features` it prints nothing.

## Summary

| Guarantee | Enforced by |
| --- | --- |
| Builds on Rust 1.85 | `msrv` job, `cargo check` on a pinned 1.85.0 toolchain |
| core / server / client / a2ui compile for wasm | `executor-agnostic` job, `cargo check --target wasm32-unknown-unknown` |
| tokio is not in those crates' dependency graphs | `executor-agnostic` job, `cargo tree -e normal` |
| Every feature combination that matters still builds | `features` job — see [Feature flags](/ag-ui-rust/reference/features/) |
