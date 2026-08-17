# ag-ui-server

Host an [AG-UI](https://github.com/ag-ui-protocol/ag-ui) agent in Rust.

AG-UI is the protocol between a user-facing application and an agent backend: a POST
carrying `RunAgentInput`, answered by a stream of typed events. This crate is the server
half — implement `Agent`, hand it to `run()`, and you have a stream a transport can
serialize. [`ag-ui-axum`](https://crates.io/crates/ag-ui-axum) mounts it on a router;
nothing here depends on a web framework, an executor or an LLM client.

```toml
[dependencies]
ag-ui-server = "0.1"
ag-ui-core = "0.1"
```

```rust
use ag_ui_core::RunOutcome;
use ag_ui_server::{Agent, Result, RunContext};
use serde::{Deserialize, Serialize};

/// State the client mirrors and the agent updates.
#[derive(Default, Serialize, Deserialize)]
struct Draft {
    revision: u32,
    title: String,
}

struct Editor;

impl Agent for Editor {
    type State = Draft;

    async fn run(&self, ctx: &mut RunContext<Draft>) -> Result<RunOutcome> {
        // A step brackets a phase of the run. Its guard emits STEP_FINISHED on
        // drop, so an early `?` cannot skip it.
        let mut step = ctx.step("draft")?;

        // Reasoning the client can render, in its own REASONING_* block.
        step.think("The user wants a title.")?;

        // A message streams as TEXT_MESSAGE_START / _CONTENT* / _END.
        let mut message = step.assistant_message()?;
        message.delta("Naming it ")?;
        message.delta("\"Q3 plan\".")?;
        message.end()?;

        // Publishing state diffs against the last snapshot and sends whichever
        // of STATE_SNAPSHOT / STATE_DELTA is smaller.
        step.update_state(|draft| {
            draft.revision += 1;
            draft.title = "Q3 plan".into();
        })?;

        Ok(RunOutcome::Success)
    }
}
```

`run(Editor, input)` turns that into a `Stream<Item = Result<Event>>` — `RUN_STARTED`,
`STEP_STARTED`, the `REASONING_*` block, the `TEXT_MESSAGE_*` triple, `STATE_SNAPSHOT`,
`STEP_FINISHED`, `RUN_FINISHED`. Serializing that stream is the transport's job;
`ag-ui-axum` does it in one line.

## Why the emit path is synchronous

The emitter API is typestate: `ctx.assistant_message()` returns a handle that borrows the
run context mutably, so starting a second overlapping message is a borrow-check error
rather than a runtime protocol violation. The handle emits its terminating event on `Drop`,
so forgetting `end()` is harmless.

That last guarantee is what forces the design. `Drop` cannot be async, so a handle cannot
`await` while emitting its terminator. The emit path is therefore synchronous end to end —
handles push into an unbounded channel and the transport layer drains it.

## Protocol verification

Emitting `TEXT_MESSAGE_CONTENT` without a preceding `START` is a bug that otherwise
surfaces as a confused frontend. This crate runs an ordering state machine, on by default,
so it surfaces where it was caused instead.

## Features

| Feature | Default | What it adds |
| --- | --- | --- |
| `verify` | yes | Protocol ordering verification. Cheap in release. |

## Executor-agnostic

This crate uses `futures` primitives — notably `futures::channel::mpsc` for the emit path
rather than `tokio::sync::mpsc`. tokio enters at `ag-ui-axum` and nowhere else, so wasm
targets and non-tokio executors keep working. CI enforces it, both by building for
`wasm32-unknown-unknown` and by asserting tokio is absent from the dependency graph.

See the [repository](https://github.com/KimSoungRyoul/ag-ui-rust) for the design rationale.

## License

MIT
