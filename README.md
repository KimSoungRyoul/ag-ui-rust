# ag-ui-rust

A Rust SDK for the [AG-UI protocol](https://docs.ag-ui.com) — build agent backends **and** agent clients in Rust.

AG-UI standardises how an AI agent talks to a user-facing application: a POST carrying
`RunAgentInput`, answered by a stream of typed events. Official SDKs exist for TypeScript,
Python, and .NET. Rust has a community SDK, but it is client-only and has drifted from the
spec. This project fills the gap, with the server story as the priority.

## Crates

| Crate | What it is |
| --- | --- |
| `ag-ui-core` | Protocol types, all event variants, and wire encoding. `serde` + `serde_json` only. |
| `ag-ui-server` | Host an agent: `Agent` trait, typestate event emitters, automatic state deltas, protocol verification. Executor-agnostic. |
| `ag-ui-axum` | Mount an agent on an axum router. The only crate that pulls in tokio. |
| `ag-ui-client` | Consume a remote agent: transport, event application, materialised messages and state. |
| `ag-ui-a2ui` | [A2UI](https://a2ui.org) protocol types, semantic validator, and agent-side authoring toolkit. |

## Quickstart

Serving an agent. Implement `Agent`, mount it, and the endpoint speaks AG-UI:

```rust
use ag_ui_axum::RouterExt;
use ag_ui_core::RunOutcome;
use ag_ui_server::{Agent, Result, RunContext};
use axum::Router;

struct Greeter;

impl Agent for Greeter {
    type State = ();

    async fn run(&self, ctx: &mut RunContext<()>) -> Result<RunOutcome> {
        // Streams as TEXT_MESSAGE_START / _CONTENT / _END.
        let mut message = ctx.assistant_message()?;
        message.delta("Hello from Rust.")?;
        message.end()?;

        Ok(RunOutcome::Success)
    }
}

let app: Router = Router::new().route_agui("/agent", Greeter);
```

Consuming one. `Session` folds the delta stream back into messages and state:

```rust,no_run
use ag_ui_client::{Session, Update, transport::HttpTransport};
use futures_util::StreamExt;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let transport = HttpTransport::new("http://localhost:3000/agent")?;
    let mut session = Session::<_>::new(transport, "thread-1");

    let mut run = session.send("hello");
    while let Some(update) = run.next().await {
        if let Update::Message(message) = update {
            println!("{:?}", message.change);
        }
    }
    drop(run);

    println!("{} messages so far", session.messages().len());
    Ok(())
}
```

Both snippets are compiled by the test suite (`e2e/src/lib.rs` doctests this file), so a
stale quickstart is a red build.

## Design commitments

**The `Agent` trait is the boundary.** The .NET SDK builds on `Microsoft.Extensions.AI`
because .NET has a blessed chat abstraction. Rust does not — the ecosystem is split across
`async-openai`, `rig-core`, and `genai`. So this SDK depends on no LLM crate at all. Bring
your own client; implement `Agent`.

**Executor-agnostic below the web binding.** `core`, `server`, and `client` use
`futures` primitives rather than tokio, so wasm targets and non-tokio executors keep working.
tokio enters at `ag-ui-axum` and nowhere else. CI enforces this two ways: by building those
crates for `wasm32-unknown-unknown`, and — because tokio itself compiles for wasm — by
asserting tokio is absent from their dependency graphs.

**Protocol misuse should not compile.** Event ordering (`Start` → `Content*` → `End`) is
enforced by typestate handles that borrow the run context, so interleaving two messages is a
borrow-check error. Handles emit their terminating event on `Drop`, so it cannot be forgotten.
Because Rust has no async `Drop`, the emit path is synchronous by design. What the borrow
checker cannot catch, a runtime ordering verifier catches — on the server and on the client,
on by default in release builds too, and compiled out via the `verify` feature if you want
the last handful of `HashSet` lookups back. Neither the TypeScript SDK (which verifies only
on the client) nor the .NET one (which does not verify) checks ordering server-side, which is
where the bug is actually caused.

**IDs are strings.** `ThreadId`, `RunId`, and friends are newtypes over `String`, not `Uuid`.
The spec says string; real backends such as LangGraph send arbitrary strings.

## Keeping up with the spec

The port is hand-written against the upstream TypeScript Zod schemas, so nothing in the
compiler links the two. `cargo run -p xtask -- drift-check` is that link: it compares a
vendored snapshot of the upstream event surface against the Rust types and fails the build
when they diverge. It is offline and deterministic, so it runs on every pull request; a
scheduled job additionally asks GitHub whether the snapshot itself has gone stale.

## Running the tests

Two commands, and the second one is not optional:

```sh
cargo nextest run --workspace --all-features
cargo test --doc --workspace --all-features
```

**`cargo nextest` does not run doctests.** It says nothing about them — it does not skip
them loudly, it never sees them — so a green nextest run is a partial result. A lot of what
this workspace proves lives in doctests: every crate README, the quickstart above, and the
`compile_fail` example in `crates/ag-ui-server/src/emit/mod.rs` that is the only executable
proof that two overlapping message handles fail to compile. Weaken the emitter API and
nextest stays green.

`cargo test --workspace --all-features` does run both, if you would rather have one command
and can live without nextest's output. CI runs both forms.

One caveat on `compile_fail` doctests that name the error they expect, as the emitter one
names `E0499`: **stable rustdoc ignores that error code**. The example need only fail to
compile, for any reason at all — including a typo that has nothing to do with the guarantee.
CI therefore runs the doctests on nightly as well, which does enforce it.

## Before you commit

Hygiene is gated by [prek](https://github.com/j178/prek) — pre-commit's hook runner rebuilt
as a single Rust binary, so there is no Python to install. Two commands, once:

```sh
brew install prek   # or: cargo install --locked prek
prek install
```

That installs both shims. Whitespace, file syntax, spelling and
`cargo fmt --all -- --check` run on every commit; clippy at `-D warnings` runs on every
push, where the wait buys something. `prek run --all-files` runs the lot by hand, and CI's
`hygiene` job runs exactly the same `.pre-commit-config.yaml`.

## Status

Early. See `docs/` for the design rationale and the upstream analysis this is based on.

## License

MIT
