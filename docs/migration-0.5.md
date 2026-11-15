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
- A `Thread` reports a second `RUN_STARTED` inside an active run as "RUN_STARTED arrived
  twice in one stream" whatever ids it carries. Alpha.3 reported a different run id as
  "RUN_STARTED does not match the requested thread and run IDs". With `verify(false)` the
  id check is still what fails it.
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
  path, whether the value there is different afterwards. `DataModel::apply` is
  unchanged.
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
