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

## Changes after alpha.3 (unreleased)

These are on `main` and not yet in a published version.

- `MultiAgentCapabilities` reads and writes its sub-agent list under the AG-UI 1.0
  key `subagents`. Alpha.3 and earlier used `subAgents`, which 1.0 peers do not read;
  that spelling is no longer read here either. The Rust field is still `sub_agents`.
- An A2UI data-model path has at most `binding::MAX_VALUE_DEPTH` (256) segments.
  `DataModel::apply` and `DataModel::lookup` return `Error::Pointer` for a longer one,
  so `SurfaceStore`, history recovery and the validator report an over-deep
  `updateDataModel` path as an ordinary error. No error variant is added.
- The client follows the 1.0 run lifecycle at both ends of a stream. A stream that
  opens with `RUN_ERROR` ends as `RunEnd::Failed` with the server's `message` and
  `code`, not as a protocol error. `RAW` or `CUSTOM` before `RUN_STARTED` is now a
  protocol error.
- A `RUN_ERROR` after `RUN_FINISHED` fails the run, so a `Thread` yields `Done` only
  when the stream ends, not at `RUN_FINISHED`. Aborting or dropping the run after
  `RUN_FINISHED` keeps the outcome it reported. Another event there is a protocol
  error, except a later run's `RUN_STARTED`: the first run keeps its outcome and an
  `unsupported` diagnostic (the new `Error::Unsupported`) says the rest was not read.
  `Applier::interrupts` is cleared by `RUN_ERROR`.
- `Validator` rejects a child written inline. A catalog's `ComponentId`,
  `ChildList` and `Tabs` child references must be component id strings, never
  objects or bindings, and a `ChildList` template needs both `componentId` and
  `path`. These checks run even with `ValidateOptions.check_prop_types` set to
  `false`. With it `true`, a literal string must also be one of the values the
  catalog's `enum` lists. All of these values were accepted before.
- `validate_json_messages`, `validate_messages` and the recovery loop report
  each defect once, where the same mistake was often reported twice. Locators
  name the message that last wrote the component, for example
  `messages[1].updateComponents.components[0].child` rather than
  `components[0].child`. Update code that matches on the old paths.
- `ag-ui/http` and `ag-ui-a2ui/schema-validation` no longer pin `yoke-derive` to
  `=0.8.2`. Its 0.8.4 declares Rust 1.82, so a fresh Rust 1.85 resolution selects it.
- `ag-ui-a2ui/schema-validation` uses `jsonschema` 0.61.2 instead of 0.29.1. The same
  messages pass and fail with the same diagnostics, and an `enum` failure still names
  every allowed value. A message with several errors can list them in a different order.
- `ag-ui-a2ui` requires exactly its own version of `ag-ui` (`=0.5.0-…`) while both are
  prereleases, instead of accepting any later alpha.
- An expired interrupt can be declined. `Thread::decline`, or a cancelled entry in
  `resume_many`, is no longer refused because `expiresAt` has passed or does not parse;
  only resolving such an interrupt is. Before, that interrupt blocked every later request.
- `Thread::unanswered_tool_calls` lists the tool calls `send`, `run` and `resume_many` refuse
  to run past, including one a failed or aborted run left without a result.
  `Thread::abandon_tool_call(&id, reason)` answers such a call with a tool message whose
  `content` and `error` carry `reason`; the call and the answer stay in the transcript and
  are sent with the next run. The refusal now reads "unanswered tool calls must be answered
  with tool messages, or abandoned", not "pending frontend tool calls".
- A hosted run cancelled while a message, tool call, reasoning block, step or subagent is
  open now ends with `RUN_FINISHED` and the cancelled outcome. Alpha.3 ended it with
  `RUN_ERROR`, code `PROTOCOL_VIOLATION`. A handle's terminator — `end()`, `finish()`,
  `fail()`, `suspend()` or `Drop` — now goes out after cancellation, and the driver closes a
  subagent left active with `SUBAGENT_ERROR`, code `CANCELLED`. Every other emit still fails
  with `Error::Cancelled`.
- Without the `verify` feature, nothing is sent after `RUN_FINISHED` or `RUN_ERROR` any more:
  such an emit fails with `Rule::RunEnded`, as it already did with `verify` on.
- A hosted run learns the conversation it continues from the request's `messages`, whether
  or not `echo_input` is on. A resumed run can now report `TOOL_CALL_RESULT` for a tool call
  an earlier run opened; alpha.3 ended such a run with `RUN_ERROR` (rule `unknown-id`)
  unless the input was echoed. `FilterToolCalls` and `SubagentVisibility::hidden()` judge
  that result by the earlier call, with or without the echo. `StreamTransformer` gains a
  provided `seed(&mut self, history: &[Message])` method that receives the same history;
  existing implementations compile unchanged.
- The Axum endpoint and the `AgUiInput` extractor refuse a request with no `Content-Type`
  with `415`, code `UNSUPPORTED_MEDIA_TYPE` (`axum::Error::MissingContentType`). Alpha.3
  read such a body as JSON. Send `Content-Type: application/json`; for a client that cannot,
  `AgentEndpoint::allow_missing_content_type()` accepts an unlabelled body on a mounted
  endpoint, and a hand-written handler can take `Bytes` and call `axum::extract::decode`.
  The documentation no longer presents the check as a CSRF defence: a required JSON media
  type means a browser must preflight a cross-origin run request, which leaves the decision
  to your CORS policy, and it is not authentication.
- An explicit `null` in one of the 23 optional fields AG-UI 1.0 registers in its
  deprecations list (`parentRunId` on `RUN_STARTED`, `rawEvent` on any event,
  `outcome` on `RUN_FINISHED`, a tool's `parameters`, …) reads as the omitted field
  instead of failing the run. The hosted endpoint does the same for the four that sit
  in a request: `forwardedProps`, a tool's `parameters`, a resume entry's `payload`
  and a media part's `metadata`. A `null` in any other optional field is still
  rejected. `ResumeEntry::resolved(id, Value::Null)` is now written without `payload`.
- The SDK no longer prints to stderr. What the compatibility boundary removes or
  translates (an unrecognised field, enum member or event, a retired `THINKING_*`
  event, a deprecated `null`, a newer `protocolVersion`) is a warning returned to the
  caller. A `Thread` run yields each as the new `Update::Warning(String)`, not as an
  error, and keeps it in `RunReport.diagnostics` with kind `warning`, within
  `diagnostic_limit`; the run is unaffected. A match on `Update` that treats every
  variant it does not name as an error should let warnings through.
  `RemoteAgent::run_events`, `decode_events` and `Transport::run` discard warnings,
  and the new `transport::enforce_events` hands each to a callback. Within a run, a
  warning already delivered is not delivered again, so a pre-1.0 producer streaming
  `THINKING_*` tokens yields one warning per kind of event, not one per token. On the
  server, `AgentEndpoint::on_input_warnings` receives a request's warnings and
  `extract::decode_with_warnings` returns them; `AgUiInput` and `extract::decode`
  discard them. A warning repeats at most 64 characters of input text, with control
  characters escaped, and names a dropped event by its type.
- A path in a protocol error or warning names a list member by its index in the
  input. It used to skip the members dropped before it, so the third of
  `[unknown, unknown, malformed]` was reported as `/0`.
- `BaseEvent::new()` is removed. Use `BaseEvent::default()`, which it returned.
- `client::Verifier::is_finished()` is removed, and nothing replaces it. A caller that needs
  to know whether a run ended watches for `RUN_FINISHED` or `RUN_ERROR` itself.
- `client::RemoteAgent::into_transport()` is removed. Borrow the transport with
  `transport()`, or keep a clone or an `Arc` of it before passing it to `RemoteAgent::new`.
- The constructors `SseFrame::event(e)` and `SseFrame::comment(text)` on
  `encode::sse::SseFrame` (also `axum::SseFrame`) are removed. Write the variants instead:
  `SseFrame::Event(e)` and `SseFrame::Comment(text.into())`.

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
