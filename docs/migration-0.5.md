# Migrating from 0.4.5 to 0.5.0-alpha.3

`0.5.0-alpha.3` combines the `ag-ui` work proposed in
[upstream PR #2778](https://github.com/ag-ui-protocol/ag-ui/pull/2778) with the separate
[Rust A2UI follow-up at `5033ef049`](https://github.com/KimSoungRyoul/ag-ui/commit/5033ef049).
It does not establish the first upstream release version or transfer package
ownership. The previous stable release is `0.4.5`. Read the matching
[0.5.0-alpha.3](https://kimsoungryoul.github.io/ag-ui-rust/v0.5.0-alpha.3/start/)
or [0.4.5](https://kimsoungryoul.github.io/ag-ui-rust/v0.4.5/start/)
guide when updating an application.

Alpha.1 can resolve `yoke-derive 0.8.3`, which requires Rust 1.87 despite its
missing registry `rust-version` metadata. Alpha.2 constrains that transitive
derive to 0.8.2 on the HTTP and A2UI schema-validation paths, preserving the
declared Rust 1.85 minimum for a newly generated consumer lockfile. Alpha.3
retains that constraint; the [alpha.2 guide](https://kimsoungryoul.github.io/ag-ui-rust/v0.5.0-alpha.2/start/)
remains available for applications pinned to it.

## Install

For registry dependencies, pin the prerelease explicitly:

```toml
[dependencies]
ag-ui = { version = "=0.5.0-alpha.3", features = ["http"] }
ag-ui-a2ui = "=0.5.0-alpha.3"
```

Use `ag-ui/axum` for a server instead of `ag-ui/http`. To test before the
registry version is indexed, use paths to `crates/ag-ui` and `crates/ag-ui-a2ui`
in a checkout of this repository. Update the application's `Cargo.lock` after
changing its dependencies.

## A2UI changes from alpha.2

`ag-ui-a2ui` now enables only `toolkit` by default. Applications using its AG-UI
history or tool-definition helpers must request `features = ["ag-ui"]`.
`features = ["ag-ui-server"]` includes that adapter, the author, and server
emission. `author` alone validates and generates A2UI without adding AG-UI.

```toml
ag-ui-a2ui = { version = "=0.5.0-alpha.3", features = ["ag-ui"] }
```

- `StreamParser` keeps component, root, deletion and data-update state per
  surface. Partial output waits for an explicit surface ID, a supported version,
  and a settled data path. Only catalog-declared component references become
  placeholders; similarly named application properties remain unchanged.
- Missing or undefined bound values are accepted by default so user input and
  later updates can populate them. Set `ValidateOptions.require_bound_values`
  to `true` to require populated bindings. Invalid pointers and defined
  non-array template collections still fail; defined nested collection items
  are checked individually.
- Use `RendererMessage::from_json` or a reusable
  `client_schema::ClientSchemaValidator` with `schema-validation` to validate
  raw renderer messages before serde applies compatibility defaults.
  `Action.user_message` moves to `extensions["userMessage"]`.
  `RendererError.path` becomes `path()` for string pointers; raw `path` and
  the former `function_call_id` are retained in `extensions["path"]` and
  `extensions["functionCallId"]`, preserving null and structured values.
- `A2uiAuthor::with_send_data_model(true)` requires `sendDataModel: true` in
  generated and restored creations. `ClientDataModel::from_json` validates
  renderer snapshots carried in `a2uiClientDataModel` metadata. The application
  chooses an owned surface and validates the snapshot as a root data-model
  edit before requesting a follow-up; decoding metadata does not apply it.
- Manual recovery returns `RecoveredSurface.surface_id`. If a generated batch
  touches multiple live surfaces, choose one with
  `RecoveryOptions.target_surface_id`; an untouched or deleted target fails.
  The returned `operations` still contains the full batch.

Explicit struct literals must account for the new `extensions` maps on `Action`,
`RendererError`, `ClientCapabilities` and `ClientCapabilitiesWire`, along with
`RecoveryOptions.target_surface_id`, `RecoveredSurface.surface_id`, and
`ValidateOptions.require_bound_values`. Use `..Default::default()` for option
structs when the defaults match the application.

See [A2UI authoring](https://kimsoungryoul.github.io/ag-ui-rust/v0.5.0-alpha.3/a2ui/authoring/)
and [validation](https://kimsoungryoul.github.io/ag-ui-rust/v0.5.0-alpha.3/a2ui/validation/)
for complete examples. Both A2UI protocol discriminators remain `v0.9` and
`v0.9.1`; the package alpha version does not select a new wire protocol.

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
