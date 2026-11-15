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
  error, except the `RUN_STARTED` of the next run on the same stream, which the last
  bullets of this section describe. `Applier::interrupts` is cleared by `RUN_ERROR`.
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
  `RemoteAgent::run_events` and `decode_events` discard warnings, and the new
  `transport::enforce_events` and `TransportStream::into_events` hand each to a
  callback. Within a run, a
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
- The client ends a chunk stream where AG-UI 1.0 says it ends. A `CUSTOM`, `STATE_SNAPSHOT`,
  `STATE_DELTA`, `STEP_STARTED`, `STEP_FINISHED`, `REASONING_START` or `REASONING_END`
  event closes the open `*_CHUNK` stream of the agent or subagent it belongs to;
  `RUN_STARTED` and `MESSAGES_SNAPSHOT` close every one, as `RUN_FINISHED` already did; and
  `SUBAGENT_FINISHED` or `SUBAGENT_ERROR` closes that subagent's, so the synthesized end
  now comes before the subagent's terminal event. A chunk that names no id after one of
  these is a protocol error ("carries no messageId and no message is open"), where alpha.3
  appended it to the message before. A chunk that repeats the id re-opens the message and
  the text assembles as before, with a second `Started` and `Ended` update. `RAW`,
  `ACTIVITY_SNAPSHOT`, `ACTIVITY_DELTA`, `REASONING_ENCRYPTED_VALUE` and `SUBAGENT_STARTED`
  still close nothing.
- A continuation chunk that repeats `role` or `name` (`TEXT_MESSAGE_CHUNK`), or
  `toolCallName` or `parentMessageId` (`TOOL_CALL_CHUNK`), with a value other than the one
  its stream was opened with is a protocol error; alpha.3 ignored the repeat. An opener
  without a `role` opened an `assistant` message, so a later chunk naming another role
  contradicts it. Repeating the same value, or leaving the field out, is unchanged.
- An `ACTIVITY_SNAPSHOT` with `replace: false` for an activity that already exists changes
  nothing: content, `activityType` and owner stay, and `Applier::apply` returns
  `Changed::Nothing`. Alpha.3 merged the content (RFC 7396) and overwrote `activityType`.
  Its own values still apply when it creates the activity.
- An `ACTIVITY_DELTA` for an id that has no activity message, or whose message is not an
  activity, is skipped. `Applier::apply` returns `Error::Patch` ("no activity message has
  this id, so the delta was skipped"), and a `Thread` reports it as a `patch` diagnostic and
  carries on. Alpha.3 created an empty activity from the delta and patched that.
- An `ACTIVITY_DELTA` that applies gives the activity its own `activityType`, so a delta
  can retype the activity it amends. Alpha.3 kept the type the snapshot set. A producer
  that does not mean to retype repeats the current type, which 1.0 requires of it.
- The client verifier brackets a reasoning block. `REASONING_START` for a block that is
  already open, `REASONING_END` for one that is not, and `RUN_FINISHED` while one is open
  are protocol errors, as they already were for a hosted run. Alpha.3 accepted all three.
- A request carries a legacy `binary` content part as the 1.0 media part it stands for:
  `image`, `audio`, `video` or `document` by MIME type, with its `data` or else its `url` as
  the source and `filename` under `metadata`. A part that only repeats a media part already
  in the message is dropped, and one with only an `id` is sent unchanged. This applies to
  `Thread` runs and `RemoteAgent::run_events`; the messages the application holds are not
  rewritten. Alpha.3 sent the `binary` part, which a 1.0 agent strips.
- The same translation applies on the way in. A `binary` part in the messages of
  `MESSAGES_SNAPSHOT` or of the `RUN_STARTED` input echo, or in a `TOOL_CALL_RESULT`, is
  read as its media part, with a warning naming it ("translated retired binary content part
  at /messages/0/content/1 to image"). The client used to drop the part as unrecognised.
  One with only an `id` is still dropped, with the "unrecognised union member" warning.
- A `Thread` reports a `RUN_STARTED` inside a run that has not ended as "RUN_STARTED
  arrived while a run was still active" whatever ids it carries. Alpha.3 reported a
  different run id as "RUN_STARTED does not match the requested thread and run IDs". With
  `verify(false)` it is reported the same way, unless it repeats the ids of the run it
  arrived in, which is let through as before.
- `server::EventVerifier` rejects a first event that is neither `RUN_STARTED` nor
  `RUN_ERROR`, with `Rule::OutOfOrder`. Alpha.3 accepted any first event. A hosted run is
  unaffected, since the driver emits `RUN_STARTED` first, and so is `RunContext::new`, whose
  harness stands in for the driver. Code that feeds a standalone `EventVerifier` a recorded
  stream has to start with the run's `RUN_STARTED`.
- `A2uiVersion` is now the one place an A2UI version is parsed and named:
  `A2uiVersion::parse`, `ALL`, `DEFAULT`, `FromStr` and `Display` are new, and what
  differs between versions is a method on it (`operations`, `accepts`). It has a module
  of its own, `ag_ui_a2ui::version`; `ag_ui_a2ui::A2uiVersion` and
  `ag_ui_a2ui::message::A2uiVersion` still name it. The wire spelling is unchanged.
- New in `ag-ui-a2ui`: `operation::Operation`, the list of the agent → renderer payload
  keys, with `AgentPayload::operation`; `surface::Lifecycle` and
  `SurfaceStore::lifecycle`; and `Validator::validate_json_updates`, which validates raw
  messages against a prior `SurfaceStore`. `Operation`, `SurfaceStore`, `SurfaceState`
  and `Lifecycle` are also re-exported at the crate root.
- `A2uiVersion` is `#[non_exhaustive]`, as is the new `Operation`, so that A2UI v1.0 can
  add a version and operations without another break. A `match` on `A2uiVersion` outside
  the crate needs a wildcard arm. For the same reason `A2uiVersion::ALL` and
  `Operation::ALL` are slices (`&'static [Self]`), not arrays whose length is part of
  their type.
- `StreamParser::process_chunk` returns `Error::Validation` carrying a `child_cycle`
  for a reference loop, as its documentation said. It returned `Error::Parse` when the
  loop was closed by a later message, was seen in a message still open, or validation
  was off. Code that matched `Error::Parse` to catch a loop has to match
  `Error::Validation`.
- An envelope failure from `StreamParser` carries the validator's code and locator
  (`invalid_value` at `message.version`, `missing_field` at
  `message.createSurface.catalogId`) and lists every failure. Each was a single
  `missing_field` at `message`. The message texts are unchanged.
- `StreamParser` no longer replays a repeated `createSurface` for a live surface when
  components arrive for a surface that has not been created.
- `Validator::validate_updates`, `SurfaceValidator::push` and `finish`, and
  `A2uiAuthor` locate a component in the message that last wrote it,
  `messages[1].updateComponents.components[0].child`, as `validate_json_messages`
  does. They named its position in the folded surface, `components[3].child`; a
  component carried over from prior state still has that form. Update code that
  matches on the old paths.
- `Catalog::from_schema` reads an `enum` that is one alternative of a `oneOf` or
  `anyOf`, the way the specification writes `Icon.name`. A literal string such a
  property does not list is now `invalid_value`; it was accepted for a catalog
  parsed from a document.
- The message of `Error::Pointer` quotes at most `error::MAX_SHOWN_PATH_CHARS` (120)
  characters of the path and states its length; so do the validator's sentences about
  a binding path. The `pointer` field still holds the whole path.
- `client::Transport` has one method. `run_raw` is removed, and `run` returns a
  `TransportStream` where it returned typed events. A transport no longer decodes or
  checks anything: it hands over wire JSON with `TransportStream::json`, or events built
  in this process with `TransportStream::typed`, and the client applies the compatibility
  boundary to either. A transport that reads JSON used to implement both methods, and
  the one the client never called had to panic:

  ```rust,ignore
  impl Transport for Socket {
      fn run(&self, _input: RunAgentInput) -> TransportFuture {
          panic!("the client reads run_raw")
      }

      fn run_raw(&self, input: RunAgentInput) -> RawTransportFuture {
          let frames = self.frames(input);
          Box::pin(async move { Ok(boxed_raw_stream(frames)) })
      }
  }
  ```

  Now:

  ```rust
  use ag_ui::RunAgentInput;
  use ag_ui::client::transport::{Transport, TransportFuture, TransportStream};
  use futures_util::stream;
  use serde_json::Value;

  struct Socket(Vec<Value>);

  impl Transport for Socket {
      fn run(&self, _input: RunAgentInput) -> TransportFuture {
          let frames = stream::iter(self.0.clone().into_iter().map(Ok));
          Box::pin(async move { Ok(TransportStream::json(frames)) })
      }
  }
  ```

  A transport that returned typed events replaces `Ok(boxed_stream(events))`, or
  `Ok(Box::pin(events) as EventStream)`, with `Ok(TransportStream::typed(events))`. Such
  events used to be serialized, walked against the schema and deserialized again, one by
  one. They are now delivered as given, with what 1.0 retired still translated as it is on
  the wire (a `THINKING_*` event, and an event that carries a `binary` content part) and a
  newer `protocolVersion` still reported, so the warnings a typed stream can cause still
  reach a `Thread`. `TransportStream::typed(events).enforced()` restores
  the full check, and `ReplayTransport` does that, so tests written against it behave as
  before. A caller of `transport.run_raw(input)` writes
  `transport.run(input).await?.into_json()`; a caller that read typed events from
  `transport.run(input)` adds `.into_events(|warning| …)`. `RawTransportFuture`,
  `boxed_stream` and `boxed_raw_stream` are removed, and `TransportFuture` now resolves
  to a `TransportStream`. None of the old names is kept as a deprecated alias: the return
  type of the one required method changed, so no alias could make an existing
  `impl Transport` compile.
- `server::Runner::run` and `server::run` yield `Event`, not `Result<Event>`. The stream
  never produced an `Err`: a failure inside a run is its `RUN_ERROR`. Before:

  ```rust,ignore
  let events: Vec<Event> = run(Greeter, input)
      .map(|event| event.expect("the stream should not break"))
      .collect()
      .await;
  ```

  Now:

  ```rust
  use ag_ui::server::{Agent, Result, RunContext, run};
  use ag_ui::{Event, EventType, RunAgentInput, RunOutcome};
  use futures_util::StreamExt;

  struct Greeter;

  impl Agent for Greeter {
      type State = ();

      async fn run(&self, ctx: &mut RunContext<()>) -> Result<RunOutcome> {
          ctx.say("hello")?;
          Ok(RunOutcome::Success)
      }
  }

  #[tokio::main]
  async fn main() {
      let input = RunAgentInput::new("thread-1", "run-1");
      let events: Vec<Event> = run(Greeter, input).collect().await;
      assert_eq!(events.last().map(Event::event_type), Some(EventType::RunFinished));
  }
  ```

  Drop the `.map(…expect…)`, the `?` on an item and any `Ok(event)` pattern.
  `axum::SseResponse::stream` takes that stream, so `response.stream(runner.run(input))`
  is unchanged. A run-owned stream that can fail, which `stream` also accepted, now goes
  to the new `SseResponse::try_stream`; a source error there still ends the run as
  `RUN_ERROR` and trips the cancellation token. `stream_frames` is unchanged.
- `ToolCallHandle::result` takes `impl Into<ToolContent>`, so a multimodal result needs no
  raw `emit`: `call.result(vec![InputContent::text("…"), …])`. A `String` or `&str` argument
  compiles as before. `&String`, `Box<str>` and `Cow<str>`, which `impl Into<String>`
  accepted, need `.as_str()` or `.to_string()`.
- A request that `Thread::send`, `send_message`, `run` or `resume_many` refuses before
  dispatch says why in its variant in two cases: `client::Error::UnansweredToolCalls(ids)`
  for tool calls without a result, and `client::Error::PendingInterrupts(ids)` for
  interrupts without an answer. Both were `Error::Request(text)`. The message text is
  unchanged; a match on `Error::Request` no longer sees these two.
- `MessageUpdate`, `ReasoningUpdate`, `SubagentUpdate`, `RunDiagnostic`, `RunReport`,
  `ResumeSubmission` and `Subagent` in `client` are `#[non_exhaustive]`. They are still
  read field by field; a struct pattern needs `..`, and code outside the crate can no
  longer build one with a struct literal.
- `ExecutionCapabilities.max_iterations` and `max_execution_time` are `Option<u64>`, as
  the schema's `minimum: 0` says; they were `Option<i64>`. A negative number, which some
  frameworks send for "no limit", now reads as `None` instead of being kept, so it is
  never written back.
- `SubAgentInfo` is renamed `SubagentInfo`, the AG-UI 1.0 name. The old name stays as a
  deprecated type alias, because an alias costs nothing here.
- `RunContext::cancel_token()` is renamed `cancellation_token()`, the name
  `Runner::cancellation_token()` already had. The old name stays as a deprecated method
  that calls the new one.
- Added, and not breaking: `Interrupt::with_message`, `with_tool_call_id` and
  `with_response_schema`; `RunContext::tool_call_with(ToolCallStartEvent)` with
  `ToolCallStartEvent::with_parent_message_id`, for a call that names the message that
  requested it; `RunContext::set_result` and `add_usage`, which the driver writes as
  `result` and `usage` on the terminal event.
- `encode::protobuf::COVERED_EVENT_TYPES` and `encode::protobuf::is_covered` are removed,
  and nothing replaces them. In alpha.3 they listed the 21 event types that upstream's
  pre-1.0 `events.proto` could carry. Upstream's 1.0 `events.proto` covers every 1.0 event
  type, so there is no gap left to check. `ProtobufFormatter` is unchanged and still
  refuses to encode.
- `binding::absolute_path(scope, path)` is new: the absolute data-model path a binding
  path resolves to in a scope, without a data model.
- The empty path and `.` are relative, and name the scope a binding is in: inside a
  template they are the item being drawn. `Scope::resolve`, `Scope::resolve_pointer`,
  `Scope::item`, `ModelScope::resolve`, `ModelScope::item` and the validator read the
  empty path as the whole model in every scope, and `.` as a key named `.`. Write `/`
  for the whole model from inside a template. Outside a template nothing changes for
  the empty path, and `.` is no longer reported as a relative path with no template.
- `DataModel::apply_reporting` and `ModelChange` are new: a write that reports, for any
  path, whether the value there is different afterwards.
- `DataModel` refuses a path with a segment `__proto__`, `constructor` or `prototype`
  (`model::FORBIDDEN_PATH_SEGMENTS`), in `apply`, `apply_reporting` and `lookup`, with
  `Error::Pointer`. alpha.3 stored and read such a key. A JavaScript renderer refuses
  these paths, so a model that took them held a state the renderer did not. Name the
  key something else. A value written at another path may still have such a key; only
  the path is checked.
- `DataModel::apply` refuses a list index above `model::MAX_LIST_INDEX` (10,000), which
  is the bound the reference renderers put on a write, for the same reason. alpha.3
  took any index the growth budget allowed, up to 65,536 new slots. The budget
  (`DEFAULT_ARRAY_GROWTH_LIMIT`) still counts every slot one update adds, so it is the
  bound for a path that grows several lists at once, and for a caller that lowers it.
- `binding::parse_template` and `TemplatePart` are new: a `formatString` template
  parsed into its parts without evaluating it. `Scope::format_string` and
  `format_string_with` now parse with it, so they accept the grammar the reference
  renderers do, with `~0` and `~1` in a path besides, and nothing else:
  - An expression that does not parse is `Error::Binding`. The old scanner read
    anything it did not recognize as a path, so `${a b}` or `${count:}` resolved to
    an empty string.
  - In a quoted string `\n`, `\t` and `\r` are the control characters and a
    backslash before any other character is that character. Only `\'` and `\"` were
    unescaped.
  - `+1`, `.5` and `1.` are numbers, and a trailing comma after the last argument of
    a call is accepted.
  - An expression may nest 100 levels deep (`MAX_TEMPLATE_DEPTH`); the limit was 10.
    A template of more than 10,000 characters or 1,000 parts is an error
    (`MAX_TEMPLATE_CHARS`, `MAX_TEMPLATE_PARTS`); there was no limit.
- `ag-ui-a2ui` depends on `unicode-ident`, for the `XID_Continue` table.
- The `processor` module is new: `MessageProcessor` takes raw messages into a
  `SurfaceStore`, with the catalogs a context speaks registered by id, each message or
  batch as a transaction, and a refusal as a `Rejection` that names its kind
  (`RejectionKind`: integrity, recursion, catalog, validation). A `deleteSurface` for
  a surface that is not live does nothing there; `SurfaceStore::apply` still refuses
  it.
- `SurfaceStore::client_data_model` is new: the `a2uiClientDataModel` metadata for the
  surfaces created with `sendDataModel: true`.
- `Catalog::document` is new: the document a catalog was read from with
  `Catalog::from_schema`, unchanged. Two catalogs are equal when their documents are
  too, so two documents that summarize alike are no longer equal catalogs.
- `Catalog::from_schema` reads a function's return type from the schema of its call,
  `properties.returnType.const`, which is how the specification's catalogs state it.
  `FunctionDef::return_type` was `None` for every function of a published catalog.
- `schema_validation::validate_common_type` is new: one value against one definition
  of the published common types (`DynamicValue`, `ChildList`, `Action`, …).
- `SchemaValidator::new` and `SurfaceValidator::register` take a catalog that declares
  no `$defs/theme`, as the published VegaChart and MCP catalogs do not. alpha.3 failed
  on it with `Pointer '/$defs/theme' does not exist`, because the envelope schema
  refers to that definition. A surface of such a catalog takes any theme that is an
  object. A catalog that declares a theme is held to it, as before.
- The chunked and the explicit form no longer mix within one message, tool call or
  reasoning message, as AG-UI 1.0 requires. An explicit `*_CONTENT`, `*_ARGS` or `*_END`
  event for an id a `*_CHUNK` opened ends that chunk stream and is then a protocol error
  ("TEXT_MESSAGE_END for message "m1", which is not open"); alpha.3 read it as part of
  the chunk stream. A chunk that names no id after an explicit `*_START` is a protocol
  error ("carries no messageId and no message is open as a chunk stream"), and one that
  names the open id is a second start ("which is already open"); alpha.3 appended both
  to the open message. A producer finishes an item in the form it opened it in: chunks
  only, or `*_START`, `*_CONTENT`, `*_END`. An explicit `*_START` for an id a chunk
  opened is still accepted, as a re-open. `ChunkNormalizer::is_open` is `true` only
  while a chunk-opened stream is open, no longer after an explicit `*_START`.
- `client::Verifier` and `verify_all` judge a `*_CHUNK` event as the events it stands
  for; alpha.3 skipped chunks. A raw stream gets the verdict it gets through a `Thread`:
  a chunk that cannot be expanded is a protocol error, and a chunk stream is closed by
  the event that ends it. A stream already run through `ChunkNormalizer` is unaffected.
- A hosted run and `server::EventVerifier` apply the chunk rules a consumer applies, with
  the new `Rule::MalformedChunk`: a `*_CHUNK` that names no id with no chunk stream to
  continue, a `TOOL_CALL_CHUNK` that opens a call without `toolCallName`, and a
  continuation that repeats `role`, `name`, `toolCallName` or `parentMessageId` with
  another value. A chunk stream ends where AG-UI 1.0 says it ends, so a chunk that names
  no id after a `CUSTOM`, `STATE_*`, `STEP_*` or reasoning event of the same agent or
  subagent is rejected. A chunk for an id that is explicitly open is
  `Rule::DuplicateStart`. Alpha.3 accepted all of these, and such a chunk also closed
  the explicitly open id. Repeat the id on the first chunk after such an event, and
  name the tool on the chunk that opens a call.
- `FilterToolCalls`, `SubagentVisibility::inline()` and `SubagentVisibility::hidden()`
  resolve a chunk that names no id by the same rules. After an event that ends the chunk
  stream, or after an explicit `*_START`, such a chunk is no longer treated as part of
  the earlier stream: `inline()` passes it on without an id, where alpha.3 filled in the
  id of the stream that had ended, and `FilterToolCalls` drops a `TOOL_CALL_CHUNK` it can
  place in no call, as it always dropped one with nothing open.
- `server::EventVerifier` accepts one `RUN_ERROR` after `RUN_FINISHED`, which AG-UI 1.0
  lets a producer send; alpha.3 rejected it with `Rule::RunEnded`. A hosted run is
  unchanged: its sink still refuses every emit after a terminal event.
- `server::EventVerifier` rejects a `RUN_FINISHED` whose `interrupt` outcome carries no
  interrupt, with the new `Rule::InvalidOutcome`, as the client verifier already did.
  In a hosted run this is an `Event::run_finished_interrupt(.., vec![])` emitted by hand;
  an agent that returns such an outcome already ended with `RUN_ERROR`.
- Verifier messages changed. The client says "which is not open" where it said "which
  was never opened", and a chunk with nothing to continue ends "is open as a chunk
  stream". A `VerificationError::detail` quotes an id as `"m1"`, not as
  `MessageId("m1")`.
- When several things are open at `RUN_FINISHED`, the `Rule::OpenAtFinish` detail names
  the same one every time: a message before a tool call, a reasoning message, a
  reasoning block, a step and a subagent, and of several of a kind the one opened first.
  Alpha.3 named an arbitrary one of a kind, and looked at reasoning before tool calls.
  The client verifier already named the first opened, in this order.
- `Applier::apply` applies the explicit form only. For a `TEXT_MESSAGE_CHUNK`,
  `TOOL_CALL_CHUNK` or `REASONING_MESSAGE_CHUNK`, and for a retired `THINKING_*` event,
  it returns the new `Error::Unsupported` and changes nothing; alpha.3 assembled chunks by rules
  of its own, which had drifted from the normalizer's, and turned `THINKING_*` into
  reasoning under ids `thinking-1`, `thinking-2`, …. A `Thread` is unaffected: it expands
  and translates before it applies. Code that drives an `Applier` itself puts a
  `ChunkNormalizer` in front, applying what `normalize` appends and calling `finish` when
  the stream ends, and reads a stream that may carry `THINKING_*` through
  `TransportStream::into_events` or `transport::enforce_events`, which translate it.
- `ag-ui-a2ui` has a module `ag_ui_a2ui::agent` (feature `agent`), and it is the one
  place A2UI is generated: catalog negotiation (`RendererCapabilities`,
  `resolve_catalogs`), `A2uiGenerator` and the per-request `A2uiRequestProcessor`, the
  `OutputFormat` trait with `DirectJson`, `RendererState`, `ResponseStream` and
  `RetryDriver`. The framework makes the model call; the layer supplies the prompt
  snippet, reads the answer, validates it against the catalogs the renderer can draw,
  and says whether to ask again. alpha.3 had three loops, three parsers and two prompt
  builders in `toolkit` and `author`; they are removed (the table below) or, for
  `A2uiAuthor`, rebuilt on the layer.
- The default feature of `ag-ui-a2ui` is `agent`; in alpha.3 it was `toolkit`, which
  is gone (the bullet after the table below says what each use of it maps to). A
  dependency with no features named gets the protocol modules and the agent layer.
  `schema-validation` turns no other feature on, and `author` is `agent` plus
  `schema-validation`.
- `SchemaBundle`, `SCHEMA_BLOCK_START`, `SCHEMA_BLOCK_END` and `DEFAULT_CUTTABLE_KEYS`
  are in the module `ag_ui_a2ui::schema`, which needs no feature. alpha.3 had them in
  `toolkit::schema` and `toolkit::streaming`, which are gone. Write
  `ag_ui_a2ui::schema::SchemaBundle`.
- A streamed reasoning message is a message of the conversation, as AG-UI 1.0 has it.
  `Thread::messages()`, `Applier::messages()`, `Update::Messages`,
  `RunReport::new_messages` and a snapshot's `messages` hold it as `Message::Reasoning`,
  where it arrived: before the reply it preceded. Alpha.3 kept it out of all of them, in
  a list of its own. So a view that draws every message now draws the agent's reasoning
  too. To draw the conversation without it, leave the role out:
  `thread.messages().iter().filter(|m| m.role() != Role::Reasoning)`.

  ```rust
  use ag_ui::client::Applier;
  use ag_ui::{Event, Role, TextMessageRole};

  let mut applier = Applier::new();
  for event in [
      Event::reasoning_message_start("think-1"),
      Event::reasoning_message_content("think-1", "weighing the options"),
      Event::reasoning_message_end("think-1"),
      Event::text_message_start("answer-1", TextMessageRole::Assistant),
      Event::text_message_content("answer-1", "the answer"),
      Event::text_message_end("answer-1"),
  ] {
      applier.apply(&event).expect("applies");
  }
  // Alpha.3 listed one message here, the answer.
  assert_eq!(applier.messages().len(), 2);

  let without_reasoning: Vec<_> = applier
      .messages()
      .iter()
      .filter(|message| message.role() != Role::Reasoning)
      .collect();
  assert_eq!(without_reasoning.len(), 1);
  ```

  A view that draws from updates is unchanged. `Update::Reasoning` still announces each
  change to a reasoning message, and a reasoning delta produces no `Update::Message`.
  `ReasoningUpdate` and `ReasoningChange` gain `index`, the message's place in
  `messages()`; code that builds a `ReasoningChange`, or destructures one without `..`,
  names the new field.
- A run sends the conversation's reasoning messages back to the agent, each as it
  stands. A `REASONING_ENCRYPTED_VALUE` is stored on the message it names, so it returns
  unmodified in every later request, as 1.0 requires. Only activity messages are left
  out of a request. Alpha.3 never sent reasoning. An agent now finds `reasoning`
  messages in `RunAgentInput.messages`; one that forwards its history to a model
  decides what its provider takes.
- `Thread::reasoning()` and `Applier::reasoning()` are a view over `messages()` and
  return an iterator, where they returned `&[ReasoningMessage]`. Write `.count()` for
  `.len()`, `.next()` for `[0]`, and `.cloned().collect::<Vec<_>>()` for `.to_vec()`.
- A reasoning span opens no message. `REASONING_START` and `REASONING_END` change
  nothing: `Applier::apply` returns `Changed::Nothing` and a `Thread` yields no update.
  Alpha.3 made an entry in `reasoning()` under the span's id, which stayed empty when
  the message inside the span had an id of its own, and reported `Started`, `Ended` and
  `Aborted` for it. Those three now come from the reasoning message alone, at
  `REASONING_MESSAGE_START`, at `REASONING_MESSAGE_END` and when the run stops with the
  message open. An application that needs to know a span is open reads
  `Event::ReasoningStart` and `Event::ReasoningEnd` in `Thread::on_event`.
- An id names one message. A `REASONING_MESSAGE_START` for an id the conversation
  already holds continues that message, and a `TEXT_MESSAGE_START` for the id of a
  reasoning message continues the reasoning message, as the reference client does.
  Alpha.3 held a reply and a reasoning message with one id apart, in its two lists.
  1.0 requires a producer to give each message of a thread an id of its own.
- `MESSAGES_SNAPSHOT` follows the 1.0 rule for reasoning. A snapshot that carries no
  reasoning message leaves the client's where they are. One that carries any declares
  the whole set, so a reasoning message it leaves out is dropped. In alpha.3 no snapshot
  removed streamed reasoning, and a reasoning message a snapshot carried was added to
  `messages()` beside the streamed copy in `reasoning()`.
- A message in a `MESSAGES_SNAPSHOT` replaces the client's message with the same id,
  whatever the role of either. Alpha.3 kept an activity message whose id a snapshot
  without activity gave to another message, and discarded the snapshot's.
- A `REASONING_ENCRYPTED_VALUE` that names an activity message changes nothing, and
  `Applier::apply` returns `Changed::Nothing`: an activity has no `encryptedValue`.
  Alpha.3 reported `MessageChangeKind::EncryptedValue` for it.
- `Thread::snapshot()` writes format version 2: reasoning is in `messages`, and
  `reasoning` is not written. `Thread::restore` still reads version 1. Nothing in that
  format says where a reasoning message stood among the others, so its `reasoning`
  entries are put before every message, in the order the list held them: reasoning
  comes before the reply it led to, and there each entry still does. From there they
  are sent with the next run like any other. None is next to the reply it preceded,
  so an agent that pairs a reasoning message with the message after it finds the
  first message of the conversation there. Two kinds of entry are not carried over:
  one whose id a message already has, and one with no content, encrypted value or
  metadata, which is what alpha.3 kept for a `REASONING_START` whose id no reasoning
  message shared. `ThreadSnapshot::reasoning` stays for reading version 1 and is empty
  in a new snapshot, so code that read it reads `messages` and filters by role. A
  version 2 snapshot with entries in `reasoning` is refused, and alpha.3 cannot read a
  version 2 snapshot.
- A `RunStream` yields one more update per run: the new `Update::RunEnded(RunSummary)`,
  once for every run the stream carried, the only run of a stream included. It arrives
  right before `Update::Done`, after a fatal `Update::Error` and whatever that run left
  aborted, so a stream that ended `[.., Error, Done]` in alpha.3 now ends
  `[.., Error, RunEnded, Done]`, and one that yielded only `Done` yields `RunEnded` and
  then `Done`. Code that matches on the variant is unaffected. Code that counts updates,
  or takes the one before `Done` by position, skips `Update::RunEnded`. `RunSummary` has
  the run's `run_id`, its `end` and whether it is the one the request asked for
  (`requested`); `RunReport::runs` lists them in stream order.
- A `Thread`, `client::Verifier`, `client::verify_all` and `server::EventVerifier` read
  several runs in sequence on one stream, as AG-UI 1.0 allows: a `RUN_STARTED` after
  `RUN_FINISHED` or after `RUN_ERROR` begins the next run. Messages accumulate, reasoning
  messages among them, and state persists across runs; what a run had open does not
  carry over, so a new run may open a step, or announce a subagent, of the same name,
  and a message a failed run left half-written stays as it is. In alpha.3 the verifiers
  rejected that `RUN_STARTED` ("arrived after the run had already finished",
  `Rule::RunEnded`), and a `Thread` stopped reading at the first run's terminal event. A
  hosted run still sends one run: its sink refuses every emit after a terminal event, as
  before.
- A `Thread` reads to the end of the stream after `RUN_ERROR` too, since another run may
  follow. `Update::Error` and `Update::RunEnded` arrive at the `RUN_ERROR`, and
  `Update::Done(RunEnd::Failed { .. })` when the transport ends the stream; alpha.3
  yielded `Done` at the `RUN_ERROR`. Against a server that leaves the connection open
  after `RUN_ERROR`, act on `Update::RunEnded`, or abort: the request still ends as
  `Failed`. `RunStream::collect_report` returns when the stream ends.
- A `Thread` no longer requires every `RUN_STARTED` to carry the request's run id. The
  run that does is the requested run; one before it is a run the producer replays, read
  into the conversation and listed in `RunReport::runs` with `requested: false`. Alpha.3
  failed such a stream at its first event with "RUN_STARTED does not match the requested
  thread and run IDs" and applied nothing. A stream that ends without the requested run
  now fails at its end, with "the stream ended without the requested run: no RUN_STARTED
  carried run ID …", after what it carried was applied. A `RUN_STARTED` for another
  thread fails with "RUN_STARTED does not match the requested thread ID". Each
  `RUN_FINISHED` must still carry the ids of the `RUN_STARTED` it closes.
- On a stream of several runs, `RunReport::end`, `Update::Done` and
  `Thread::last_run_end` say how the last run from the requested one on ended, or how
  reading stopped. What each run's end does to the thread depends on where it stands:

  | Run | Ends | The thread |
  | --- | --- | --- |
  | replayed, before the requested run | any way | keeps what it delivered; nothing pending changes, no `Update::Interrupt` |
  | requested | success, interrupt | confirms a resume submission; pending interrupts cleared, or set (as in alpha.3) |
  | requested | cancelled, `RUN_ERROR`, cut short | submission `Unconfirmed`, interrupts as they were (as in alpha.3) |
  | after the requested run | success, cancelled | pending interrupts cleared |
  | after the requested run | interrupt | pending interrupts set, with `Update::Interrupt` |
  | after the requested run | `RUN_ERROR`, cut short | pending interrupts left |
  | after the requested run, a submission `Unconfirmed` | any `RUN_FINISHED` | pending interrupts left, with one `Update::Warning` |

  A stream with only the requested run behaves as in alpha.3. `unanswered_tool_calls()`
  reads the conversation, so a call any run on the stream left without a result is
  listed; the calls a success outcome names are those of the run `end` reports.
- A run after the requested one does not replace the pending interrupts while a resume
  submission is `Unconfirmed`: its entries answer exactly those interrupts, and it stays
  until the application reconciles it with the server's state and restores a reconciled
  snapshot, as for any unconfirmed submission. The run's outcome is still what
  `RunReport::end` reports, and one warning says the interrupts were kept: "run "r-2"
  finished with outcome `interrupt` while a resume submission is unconfirmed: the
  pending interrupts stay the ones that submission answers until the application
  reconciles it". No `Update::Interrupt` is yielded for them.
- `A2uiAuthor` keeps its constructors and methods and is now a use of the agent layer:
  `AuthorRequest::prompt` is the request's target followed by
  `A2uiRequestProcessor::prompt_snippet`, an answer is read by `DirectJson` (the
  messages as a JSON array, inside `<a2ui-json>` tags or on their own), and the attempts
  are a `RetryDriver`'s. `A2uiAuthor::processor()` is new and gives that processor. What
  changes for a caller:
  - A batch that is valid A2UI and not what was asked for (another surface, another
    version, a `deleteSurface`, a `createSurface` in an edit) is `Error::Validation`,
    located in the message (`messages[1].version`). alpha.3 returned `Error::Parse` or
    `Error::Catalog` with no location, and fed it back to the model at `response`.
  - A component without an `id` is located where it is,
    `messages[1].updateComponents.components[1].id`. alpha.3 reported a decode error at
    `response`.
  - A composition violation (`allowedParents`, `allowedChildren`) is `Error::Validation`
    with one `invalid_value` per violation, each located. alpha.3 returned
    `Error::Catalog` with the list in its message, and fed that back at `response`.
  - A create that draws nothing (`createSurface` and no `updateComponents`) is
    `empty_components` at `response`.
  - The prompt shows the envelope and common-types schemas this crate pins, cut down to
    what the catalog refers to, and the bundle's catalog. A bundle with its own `s2c` or
    `common_types` is still what every message is validated against.
- `MessageProcessor` is what the agent layer's `RendererState` runs on, and gained what
  that needed:
  - `MessageProcessor::register_schema(catalog_id, SchemaValidator)` (feature
    `schema-validation`) also holds every message of that catalog's surfaces to the JSON
    Schemas. A refusal is located like any other,
    `messages[0].createSurface.theme.primaryColor`.
  - `begin_batch()` and `process_at(index, message)` take a batch one message at a time.
  - `accepting_version(version)` refuses a message of another protocol family.
  - `register` takes a `Catalog` or an `Arc<Catalog>`.
  - A component that carries `@metadata`, `@catalogId`, `@accessibility` or `@checks` is
    refused, with `invalid_value` at the key, unless its catalog declares a property by
    that name. Those are component keys of A2UI v1.0, and the reference renderers refuse
    them on an earlier surface. alpha.3 had no processor; `Validator` ignores the keys,
    as before.
- `Validator` resolves bindings only in the components the root reaches. A component
  the root cannot reach is still reported in `ValidationReport::unreachable` and still
  checked for its type, properties and path syntax, but a relative path in it is no
  longer "not inside a list template" and its data is not looked up: where it would sit
  is not known, and nothing draws it. alpha.3 resolved the bindings of every component
  nothing pointed at, so a row left behind when its list was replaced failed the
  surface. A payload with no root (a fragment of an update) is checked as before.
- `RetryDriver::judge_block` judges an answer that is one block with no tags, such as a
  tool call argument, and `RetryDriver::reject` turns down an answer that stood for a
  reason of the caller's own.
- A block of direct JSON with an invalid escape (`"\approx"`) is rejected with
  "Unescaped backslash found" and how to write one, so that the model reading the
  retry prompt can fix it.
- Removed from `ag-ui-a2ui`, with what to write instead. `processor` is an
  `A2uiRequestProcessor`, `state` its `RendererState`, `retry` a `RetryDriver`:

| Removed | Write instead |
|---|---|
| `toolkit::recovery::generate_with_recovery`, `generate_with_recovery_async` | `loop { match retry.judge(&call_model(&prompt).await?, &mut state) { Verdict::Done(parts) => break parts, Verdict::Retry(feedback) => prompt = feedback.prompt(&request), Verdict::GiveUp(error) => return Err(error) } }`, or `author.create(id, request).generate(call).await` for one surface |
| `RecoveryOptions::max_attempts` | `RetryDriver::new(&processor, max_attempts)` |
| `RecoveryOptions::prior` | The `RendererState` given to `judge`, kept for the conversation |
| `RecoveryOptions::validate`, `RecoveryOptions::for_update()` | Nothing: an answer is checked against the state it is read into, so an update needs no other options |
| `RecoveryOptions::target_surface_id`, `RecoveredSurface::surface_id` | `state.surface(id)` |
| `RecoveredSurface::operations`, `text` | The `ResponsePart::A2ui` and `ResponsePart::Text` parts of `Verdict::Done` |
| `RecoveredSurface::components`, `data_model` | `state.surface(id)` and its `components`, `data_model` |
| `RecoveredSurface::attempts` | `retry.attempts()` |
| `RecoveryActivity`, `RecoveryStatus`, `constants::A2UI_RECOVERY_ACTIVITY_TYPE` | Report from the verdict: `Feedback::attempt()` and `Feedback::errors()`; write `"a2ui_recovery"` if a frontend routes on it |
| `toolkit::parser::parse_response` | `processor.parse_response(answer)` for validated, typed messages; `DirectJson::new().unwrap(answer)` then `.compile(&raw, closed)` for the raw JSON of each block |
| `toolkit::parser::unwrap_response` | `DirectJson::new().unwrap(answer)` |
| `toolkit::parser::parse_and_fix` | `DirectJson::new().compile(payload, true)` |
| `toolkit::parser::has_a2ui_parts` | `DirectJson::new().has_content(answer, true)` |
| `toolkit::parser::ResponsePart` | `agent::ResponsePart`, or `agent::RawPart` before compiling |
| `toolkit::streaming::StreamParser`, `ag_ui_a2ui::StreamParser`, `process_chunk` | `let mut stream = processor.open_stream(&state, StreamOptions::default())?;` then `stream.feed(chunk, &mut state)?` and `stream.close(&mut state)?`, or `StreamedAnswer` |
| `StreamParser::with_cuttable_keys` | `A2uiGenerator::with_format(Arc::new(DirectJson::new().with_progressive_keys(keys)))` |
| `StreamParser::without_validation` | Nothing: a stream emits what a renderer would accept. `OutputFormat::reader` yields the unvalidated candidates |
| `StreamParser::surface_id`, `root_id` | `state.surfaces()` |
| `loading_…` placeholder components in a stream | Nothing: a component is emitted when it arrives, and the renderer buffers it until its parent does |
| `toolkit::prompt::build_subagent_prompt`, `PromptSpec` | `format!("{role}\n\n{}", processor.prompt_snippet())`; the role, the task and the conversation are the framework's own prompt |
| `PromptSpec::with_examples`, `toolkit::schema::load_examples` | `A2uiGenerator::with_examples(vec![serde_json::from_str(&std::fs::read_to_string(path)?)?])` |
| `PromptSpec::with_schemas`, `SchemaBundle::render_llm_instructions`, `Catalog::render_summary` | `processor.prompt_snippet()` |
| `toolkit::prompt::GENERATION_GUIDELINES`, `WORKFLOW_SECTION`, `UI_SECTION`, `EXAMPLES_SECTION` | Nothing: the snippet carries its own rules |
| `toolkit::prompt::augment_prompt_with_errors`, `format_validation_errors` | `feedback.prompt(original)` from `Verdict::Retry`, or `retry.reject(errors)?.prompt(original)` for errors of the caller's own |
| `toolkit::prompt::describe_surface`, `render_data_model` | Nothing; `serde_json::to_string_pretty(&value)` for the second |
| `toolkit::negotiate::ClientCapabilities`, `ClientCapabilitiesWire::from_json` | `RendererCapabilities::from_json(&capabilities)?`, or `RendererCapabilities::from_metadata(Some(&metadata), version)?` for a transport's metadata; `schema_validation::validate_capabilities(&capabilities)?` for the schema check `from_json` used to run |
| `ClientCapabilities::supporting(ids)` | `RendererCapabilities::supporting(version, ids)` |
| `toolkit::negotiate::select_catalog`, `select_catalog_schema` | `resolve_catalogs(&configs, Some(&capabilities), accepts_inline)?`, which gives every active catalog, the agent's own first, or `generator.create_processor(Some(&capabilities))?`. An agent registers at least one catalog: `select_catalog_schema(&[], …, true)` chose a renderer's inline catalog alone, and `resolve_catalogs` refuses an empty registration |
| `CatalogRegistry`, `RegistryEntry` | `A2uiGenerator::new(vec![CatalogConfig::new(CatalogDocument::read(path, None, Some(version))?)])` |
| `CatalogRegistry::insert_alias`, `relax_strict_validation` | Nothing: a catalog has one id, and a schema is used as published |
| `SchemaBundle::prune` | `CatalogConfig::keeping_components(names)` and `keeping_functions(names)`; `DirectJson::allowing_messages(operations)` for the envelope |
| `SchemaBundle::relaxed`, `toolkit::schema::remove_strict_validation` | Nothing |
| `SchemaBundle::from_catalog(catalog)` | `SchemaBundle { catalog, ..Default::default() }` |
| `SchemaBundle::custom_cuttable_keys`, `cuttable_keys()` | `DirectJson::with_progressive_keys(keys)` |
| `ValidateOptions::with_root_id(id)` | `ValidateOptions { root_id: id.into(), ..options }` |
| `catalog::BASIC_FUNCTIONS` | `Catalog::basic().functions.keys()` |
| `catalog::COMMON_PROPS` | Nothing |
| `constants::MIME_TYPE` | `adapters::a2a::MIME_TYPE` (feature `a2a`) |
| `toolkit::ops::{Intent, SurfaceSpec, assemble_ops, create_surface, update_components, update_data_model}` | The same names in `adapters::agui` |
| `toolkit::envelope::{wrap_as_operations_envelope, operations_envelope, is_operations_envelope, unwrap_operations_envelope}`, `ag_ui_a2ui::wrap_as_operations_envelope` | The same names in `adapters::agui` |
| `toolkit::envelope::wrap_error_envelope(surface_id, message, &errors)`, `ag_ui_a2ui::wrap_error_envelope` | `adapters::agui::error_envelope(message)`, or `recovery_exhausted_envelope(&[Attempt { attempt, ok: false, errors }])?` when the attempts are used up |
| `toolkit::tools::render_a2ui_tool(Some(&catalog))`, `generate_a2ui_tool()`, `tool_definitions(catalog)` | `adapters::agui::render_a2ui_tool()` and `generate_a2ui_tool()`, each an `ag_ui::Tool` |
| `ToolDefinition`, `Tool::from(definition)` | Nothing: the functions return the `ag_ui::Tool` a run carries |
| `ToolDefinition::to_anthropic_value()` | `json!({"name": tool.name, "description": tool.description, "input_schema": tool.parameters})` |
| `agui::find_prior_surface_in(messages)`, `ag_ui_a2ui::find_prior_surface_in`, `toolkit::history::find_prior_surface(&history)` | `adapters::agui::latest_surface(messages).ok().flatten()` |
| `toolkit::history::try_find_prior_surface(&history, None)` | `adapters::agui::latest_surface(messages)?` |
| `find_prior_surface_by_id(&history, Some(id))`, `try_find_prior_surface_by_id(&history, id)` | `adapters::agui::surfaces_in(messages)?.get(id)` |
| `toolkit::history::PriorSurface` | `SurfaceState`, with the same `surface_id`, `components`, `data_model` and `deleted`; `catalog_id` is a `String` |
| `toolkit::history::HistoryMessage`, `HistoryMessage::from(&message)` | Nothing: the readers take `&[ag_ui::Message]` |
| `toolkit::history::next_surface_id(&history, base)` | `let surfaces = surfaces_in(messages)?;`, then the first of `base`, `base-2`, … for which `surfaces.get(id)` is `None` |
| `agui::A2uiRunContextExt`, `ctx.send_a2ui(&surface)?` | `let mut painter = SurfacePainter::from_history(ctx.messages()).map_err(Error::agent)?;` once in the run, then `painter.paint(ctx, surface.operations())?` |
| A tool result unwrapped by hand: `serde_json::from_str`, `is_operations_envelope`, `unwrap_operations_envelope` | `adapters::agui::operations_in(&message)` |

- The `toolkit` feature and the `toolkit` module of `ag-ui-a2ui` are gone, so a
  manifest that names `features = ["toolkit"]` stops resolving. What alpha.3's
  `toolkit` held maps to:

  | Used from `toolkit` | Feature to name | Where it is |
  |---|---|---|
  | `ops`, `envelope`, `tools`, `history` | `ag-ui` | `adapters::agui`; they are AG-UI's conventions and take `ag_ui::Message` and `ag_ui::Tool` |
  | Sending a surface in a run (`ctx.send_a2ui`) | `ag-ui-server` | `adapters::agui::SurfacePainter::paint` |
  | A2UI over A2A (`constants::MIME_TYPE`) | `a2a` | `adapters::a2a` |
  | `parser`, `prompt`, `recovery`, `streaming`, `negotiate`, `schema` | None: `agent` is the default | `agent` and `schema`, as the table above gives them |

  `ag-ui` and `a2a` turn `agent` on. `ag-ui-server` is `ag-ui` plus `ag-ui/server`
  and no longer turns `author` on, so sending operations does not bring in the JSON
  Schema engine. A crate that uses `A2uiAuthor` and named only `ag-ui-server` adds
  `author`: `features = ["ag-ui-server", "author"]`.
- The module `agui` is `adapters::agui` (feature `ag-ui`). It follows AG-UI's own A2UI
  middleware, read at `ag-ui-protocol/ag-ui` commit `2330368a`, so a frontend that
  paints behind that middleware paints from a Rust agent with nothing in front.
- A surface reaches a frontend as an `ACTIVITY_SNAPSHOT` with `activityType`
  `"a2ui-surface"`, `replace: true` and content `{"a2ui_operations": […]}`, after the
  tool result. alpha.3 sent the `render_a2ui` call and its result and no activity, so a
  frontend painted only with the TypeScript middleware in front. `SurfacePainter::paint`
  sends all three; `SurfacePainter::settle` gives the tool result and the events
  without a run. A client receives one more event for a paint and holds one more
  message, an `ActivityMessage` whose id is `a2ui-surface-<tool call id>`.
- The activity carries the surface whole (its creation, every component, its data),
  whatever the batch sent, because a frontend replays an activity on its own. The tool
  result still carries the batch as sent. Within one `SurfacePainter`, which is one
  run, a later batch replaces the surface's activity in place. A painter made in a
  later run (`SurfacePainter::from_history(ctx.messages())`) paints under that run's
  tool call, so an update in a later run is a second activity that carries the whole
  surface.
- `render_a2ui_tool()` and `generate_a2ui_tool()` take the arguments AG-UI's toolkit
  gives them: `{surfaceId, components, data}` with the first two required, and
  `{intent?, target_surface_id?, changes?}`. alpha.3 had `{components, data_model,
  message}` and `{intent, surface_id, request, design_notes}`. `RenderArgs` and
  `GenerateArgs` decode a call's arguments; `RenderArgs::into_spec(catalog_id)` gives
  the `SurfaceSpec`. The `render_a2ui` call that `paint` makes carries `surfaceId`, and
  `components` and `data` when the batch has them; `send_a2ui` wrote `surfaceId` alone.
- A surface that could not be made is reported in AG-UI's shapes. The tool result is
  `error_envelope(message)`, `{"error": …}`, or `recovery_exhausted_envelope(&attempts)`,
  `{"error": "Failed to generate valid A2UI after N attempt(s)", "code":
  "a2ui_recovery_exhausted", "attempts": […]}`. `surface_status(tool_call_id, &status)`
  puts the call's surface activity in `SurfaceStatus::Building`, `Retrying` or `Failed`.
  alpha.3's `wrap_error_envelope` wrote `{"error", "code": "VALIDATION_FAILED",
  "surfaceId", "path", "details"}`, which no AG-UI frontend reads.
- Reading a conversation: `operations_in(&message)` gives the operations a message
  carries, `surfaces_in(messages)` every surface as a `SurfaceStore`, and
  `latest_surface(messages)` the one touched last. They read a tool result whose
  content is an operations envelope and an `a2ui-surface` activity, and nothing else.
  alpha.3's `find_prior_surface_in` read the text of every role, so an assistant,
  user or reasoning message that quoted an envelope or an `<a2ui-json>` block was
  replayed as a surface; those are no longer read. A conversation that does not replay
  is an `Err` from `surfaces_in` and `latest_surface`, where `find_prior_surface_in`
  gave `None`.
- A client leaves activity messages out of a request, so a server's
  `ctx.messages()` holds the tool results and a client's `thread.messages()` holds
  the activities too. `surfaces_in` gives the same surfaces for both: where a tool
  call's result carries operations, the activity that call owns is not read.
- `user_action(ctx.input())` reads the action a frontend sends in
  `forwardedProps.a2uiAction.userAction`, and `UserAction::as_user_text()` is the text
  AG-UI's middleware hands a model for it. `catalog_context(ctx.input())` reads the
  catalog a frontend sends as a `context` entry, and
  `CatalogContext::capabilities(version)` gives the `RendererCapabilities` for
  `A2uiGenerator::create_processor`. alpha.3 read neither.
- `SurfaceState::replay()` gives the messages that rebuild a surface on a renderer
  that holds nothing, and `SurfaceState::outline(&catalog, &data)` the components the
  root reaches, in drawing order, each as a `Placed` with its depth and the scope its
  bindings resolve in, a list template once for each item. A client that walked the
  component tree by hand calls `outline`. `Placed` is exported at the crate root.
- `adapters::a2a` (feature `a2a`) builds and reads A2UI over A2A as JSON, with no A2A
  library: `data_part(&message, version)` and `is_a2ui_part(&part)` for the data part
  and its `mimeType`, `agent_extension(version, accepts_inline_catalogs, &catalog_ids)`
  for the agent card, `select_newest` and `activate` to agree on a version, and
  `renderer_capabilities(&message, version)` for what a client's message says it
  draws. alpha.3 had none of these.

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
