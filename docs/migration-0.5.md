# 0.5 candidate migration notes

This is an unpublished `0.5.0-alpha.1` QA build of the `ag-ui` work proposed in
[upstream PR #2778](https://github.com/ag-ui-protocol/ag-ui/pull/2778). The
published `ag-ui` and `ag-ui-a2ui` packages remain at `0.4.5`. The version in
this checkout does not establish the first upstream release version or transfer
package ownership.

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

The standalone `ag-ui-a2ui` crate stays separate. Its AG-UI adapter has been
updated to read text from `ToolContent`, including ordered parts; A2UI protocol
authoring and validation remain in that crate. The A2UI source proposed for
upstream is reviewed separately from PR #2778.

## Try the candidate

Use path dependencies pointing at this checkout, then test the application
against its own real agent and transport. The workspace's independent
`examples/` consumers exercise HTTP, conversation state, interrupts and A2UI.

```sh
cargo fmt --all -- --check
cargo test --locked --workspace --all-features
cargo clippy --locked --workspace --all-targets --all-features -- -D warnings
cargo run --locked -p xtask -- drift-check
cargo run --locked -p xtask -- drift-check --local
```

The TypeScript and A2UI renderer comparisons live in `e2e/interop` and
`examples/review-desk/interop`. `cargo publish` is outside this QA stage.
