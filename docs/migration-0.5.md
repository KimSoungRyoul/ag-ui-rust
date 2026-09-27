# Migrating from 0.4.5 to 0.5.0-alpha.1

`0.5.0-alpha.1` is a prerelease of the `ag-ui` work proposed in
[upstream PR #2778](https://github.com/ag-ui-protocol/ag-ui/pull/2778).
It does not establish the first upstream release version or transfer package
ownership. The previous stable release is `0.4.5`. Read the matching
[0.5.0-alpha.1](https://kimsoungryoul.github.io/ag-ui-rust/v0.5.0-alpha.1/start/)
or [0.4.5](https://kimsoungryoul.github.io/ag-ui-rust/v0.4.5/start/)
guide when updating an application.

## Install

For registry dependencies, pin the prerelease explicitly:

```toml
[dependencies]
ag-ui = { version = "=0.5.0-alpha.1", features = ["http"] }
ag-ui-a2ui = "=0.5.0-alpha.1"
```

Use `ag-ui/axum` for a server instead of `ag-ui/http`. To test before the
registry version is indexed, use paths to `crates/ag-ui` and `crates/ag-ui-a2ui`
in a checkout of this repository. Update the application's `Cargo.lock` after
changing its dependencies.

## AG-UI 1.0 wire behavior

New client requests declare `protocolVersion: "1.0"`. Hosted runs declare the
version they emit on `RUN_STARTED`. For a *known* older peer whose parser cannot
accept the field, `ThreadBuilder::protocol_version(None)` or
`RunParams::protocol_version(None)` omits it from the request. Do not infer an
older peer merely because it omitted the field.

The new producer rejects retired `THINKING_*` events and emits the `REASONING_*`
family. The client translates historical input before typed delivery. Remote
cancellation is reported as `RUN_FINISHED` with a cancelled outcome; local stream
abortion remains a separate client decision. A successful outcome can name
`pendingToolCallIds`, which are frontend calls still awaiting results.

## Rust API changes from 0.4.5

- `ToolMessage.content` and `ToolCallResultEvent.content` are `ToolContent`:
  `Text(String)` or ordered `Parts(Vec<InputContent>)`. Existing string
  constructors still work. Use `as_text()` for an expected string, `to_text()`
  only when deliberately flattening text portions, and preserve `Parts` when
  forwarding a multimodal result.
- Exhaustive `RunOutcome` matches must handle
  `SuccessWithPendingToolCalls` and `Cancelled`. `RunEnd` now distinguishes
  those from ordinary success and local `Aborted`.
- The HTTP client normalizes the new event boundary before presenting thread
  updates. Truncated SSE has no `Last-Event-ID` offset recovery; investigate an
  uncertain external effect before starting another run.
- The hosted Axum endpoint sends periodic SSE keep-alive comments by default.
  `AgentEndpoint::keep_alive` configures this separately from the optional
  `event_buffer_capacity` bound for slow readers.

For example, code that previously treated every tool result as a `String`
should now choose whether text is sufficient:

```rust
use ag_ui::{InputContent, Message, ToolContent};

let result = Message::tool(
    "result-1",
    "call-1",
    vec![InputContent::text("first"), InputContent::text("second")],
);
let Message::Tool(tool) = result else { unreachable!() };
assert_eq!(tool.content.to_text(), "first\nsecond");
assert!(matches!(tool.content, ToolContent::Parts(_)));
```

Keep the new end states explicit when deciding whether application work remains:

```rust
use ag_ui::client::RunEnd;

fn needs_application_input(end: &RunEnd) -> bool {
    match end {
        RunEnd::SuccessWithPendingToolCalls { .. } | RunEnd::Interrupted { .. } => true,
        RunEnd::Success { .. }
        | RunEnd::Failed { .. }
        | RunEnd::Cancelled
        | RunEnd::Aborted => false,
    }
}

assert!(!needs_application_input(&RunEnd::Cancelled));
```

The standalone `ag-ui-a2ui` crate stays separate. Its AG-UI adapter has been
updated to read text from `ToolContent`, including ordered parts; A2UI protocol
authoring and validation remain in that crate. The A2UI source proposed for
upstream is reviewed separately from PR #2778.

## Verify the upgrade

Test the application against its own real agent and transport. The workspace's
independent `examples/` consumers exercise HTTP, conversation state, interrupts
and A2UI.

```sh
cargo fmt --all -- --check
cargo test --locked --workspace --all-features
cargo clippy --locked --workspace --all-targets --all-features -- -D warnings
cargo run --locked -p xtask -- drift-check
cargo run --locked -p xtask -- drift-check --local
```

The TypeScript and A2UI renderer comparisons live in `e2e/interop` and
`examples/review-desk/interop`. The [QA record](QA.md#05-candidate-verification-2026-09-27)
describes source and consumer validation; check
[crates.io](https://crates.io/crates/ag-ui) for the current registry status.
