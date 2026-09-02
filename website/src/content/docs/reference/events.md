---
title: Event reference
description: All 36 event types in the protocol, the Rust variant that carries each one, and the families they fall into.
---

An AG-UI run is a sequence of events. On the wire each is a JSON object with a
`type` field holding a SCREAMING_SNAKE_CASE name; in Rust each is a variant of
[`Event`](/ag-ui-rust/api/ag_ui/event/enum.Event.html), and
[`EventType`](/ag-ui-rust/api/ag_ui/event/enum.EventType.html) is that
discriminator on its own.

There are **36** of them. That number is
`EventType::ALL.len()`, and it is also what `cargo run -p xtask -- drift-check`
compares against the vendored snapshot of the upstream TypeScript schemas on
every pull request — see [Verification](/ag-ui-rust/design/verification/).

Both enums are exhaustive on purpose, so a protocol addition is a compile error
where you match rather than something a `_` arm swallows.
[Design commitments](/ag-ui-rust/design/commitments/) explains why, and what it
costs.

## The events

Every variant wraps a payload struct named after it — `Event::TextMessageStart`
carries a `TextMessageStartEvent`, and so on the whole way down. The payload's
fields are serialized beside `type`, not nested under a key, and every payload
also carries the optional `timestamp`, `rawEvent` and `metadata` fields of
`BaseEvent`, flattened into the same object. `metadata` is an object open by
key — token usage, a trace id, whatever an application needs to carry — and it
is absent or an object, never `null`. A consumer merges each event's metadata
into the message that event builds, key by key with the last write winning;
[`ag_ui::metadata`](/ag-ui-rust/api/ag_ui/metadata/index.html) has the rules
and the one reserved key.

The order below is `EventType::ALL`'s order, which is upstream's.

| Wire name | Rust variant | Family | What it means |
| --- | --- | --- | --- |
| `TEXT_MESSAGE_START` | `TextMessageStart` | Text | Opens a text message under a `messageId`. `role` defaults to `assistant`, and a JSON `null` reads as omitted. |
| `TEXT_MESSAGE_CONTENT` | `TextMessageContent` | Text | Appends `delta` to the open message. |
| `TEXT_MESSAGE_END` | `TextMessageEnd` | Text | Closes the message. |
| `TEXT_MESSAGE_CHUNK` | `TextMessageChunk` | Text | Start, content and end folded into one self-contained event. |
| `TOOL_CALL_START` | `ToolCallStart` | Tool | Opens a call, naming the tool and the `toolCallId` that correlates everything after it. |
| `TOOL_CALL_ARGS` | `ToolCallArgs` | Tool | Appends a fragment of the argument JSON. Fragments concatenate; one on its own is usually not valid JSON. |
| `TOOL_CALL_END` | `ToolCallEnd` | Tool | Closes the call. The arguments are complete. |
| `TOOL_CALL_CHUNK` | `ToolCallChunk` | Tool | Start, args and end folded into one self-contained event. |
| `TOOL_CALL_RESULT` | `ToolCallResult` | Tool | The call's result, as a `tool` message appended to the thread. |
| `THINKING_START` | `ThinkingStart` | Thinking (deprecated) | Opens a thinking block, with an optional title. Use `REASONING_START`. |
| `THINKING_END` | `ThinkingEnd` | Thinking (deprecated) | Closes the thinking block. Use `REASONING_END`. |
| `THINKING_TEXT_MESSAGE_START` | `ThinkingTextMessageStart` | Thinking (deprecated) | Opens a thinking message. Use `REASONING_MESSAGE_START`. |
| `THINKING_TEXT_MESSAGE_CONTENT` | `ThinkingTextMessageContent` | Thinking (deprecated) | Appends thinking text. Carries no message id — a block could only ever have one message in flight, which is why it was replaced. |
| `THINKING_TEXT_MESSAGE_END` | `ThinkingTextMessageEnd` | Thinking (deprecated) | Closes the thinking message. Use `REASONING_MESSAGE_END`. |
| `STATE_SNAPSHOT` | `StateSnapshot` | State | Replaces the shared state wholesale. Free-form JSON, opaque to the protocol. |
| `STATE_DELTA` | `StateDelta` | State | Patches the shared state with RFC 6902 operations, applied in order. |
| `MESSAGES_SNAPSHOT` | `MessagesSnapshot` | State | Replaces the message history — after a reconnect, or when an agent rewrites history. |
| `ACTIVITY_SNAPSHOT` | `ActivitySnapshot` | Activity | Publishes an activity's content under a client-defined `activityType`. `replace` defaults to `true`. |
| `ACTIVITY_DELTA` | `ActivityDelta` | Activity | Patches an activity's content with RFC 6902 operations. |
| `RAW` | `Raw` | Escape hatch | Forwards a provider event verbatim, with an optional `source`. |
| `CUSTOM` | `Custom` | Escape hatch | A named application-defined event. The protocol guarantees the envelope and nothing else. |
| `RUN_STARTED` | `RunStarted` | Lifecycle | The first event of every run: `threadId`, `runId`, optionally a parent run and the input that started it. |
| `RUN_FINISHED` | `RunFinished` | Lifecycle | The run ended without failing. `outcome` distinguishes success from an interrupt — a run paused for human input. |
| `RUN_ERROR` | `RunError` | Lifecycle | The run failed. Nothing follows. |
| `STEP_STARTED` | `StepStarted` | Lifecycle | Opens a named step within the run. |
| `STEP_FINISHED` | `StepFinished` | Lifecycle | Closes the named step. |
| `REASONING_START` | `ReasoningStart` | Reasoning | Opens a reasoning block for a message id. |
| `REASONING_MESSAGE_START` | `ReasoningMessageStart` | Reasoning | Opens a reasoning message. `role` is required here, unlike on `TEXT_MESSAGE_START`, and is always `reasoning`. |
| `REASONING_MESSAGE_CONTENT` | `ReasoningMessageContent` | Reasoning | Appends reasoning text. |
| `REASONING_MESSAGE_END` | `ReasoningMessageEnd` | Reasoning | Closes the reasoning message. |
| `REASONING_MESSAGE_CHUNK` | `ReasoningMessageChunk` | Reasoning | Start, content and end folded into one self-contained event. |
| `REASONING_END` | `ReasoningEnd` | Reasoning | Closes the reasoning block. |
| `REASONING_ENCRYPTED_VALUE` | `ReasoningEncryptedValue` | Reasoning | A provider's opaque reasoning blob, for zero-data-retention modes. `subtype` says whether `entityId` names a `tool-call` or a `message`. |
| `SUBAGENT_STARTED` | `SubagentStarted` | Subagent | Announces a subagent invocation under a `subagentRunId`, with a display `name`. Optionally a `description`, the enclosing `parentSubagentRunId`, and the `parentToolCallId` / `parentMessageId` that spawned it. |
| `SUBAGENT_FINISHED` | `SubagentFinished` | Subagent | Closes the invocation. `outcome` is `success` or `suspended` — the latter naming the `interruptIds` the subagent owns — and absent reads as success. `result` mirrors `RUN_FINISHED.result`. |
| `SUBAGENT_ERROR` | `SubagentError` | Subagent | The invocation failed: a `message` for a human and an optional machine-readable `code`. |

That is 4 text, 5 tool, 5 deprecated thinking, 3 state, 2 activity, 2 escape
hatches, 5 lifecycle, 7 reasoning and 3 subagent.

### Attribution

Beyond the three lifecycle events, **24** of the 36 types carry an optional
`subagentRunId` naming the subagent that produced them: the text, tool, state,
activity, reasoning and step families, plus `RAW` and `CUSTOM`. An event
without one belongs to the parent agent, so a stream that never sets the field
is exactly the stream there was before subagents existed. The nine that cannot
carry it are the run lifecycle (`RUN_STARTED`, `RUN_FINISHED`, `RUN_ERROR`),
`MESSAGES_SNAPSHOT` — whose messages carry their own — and the five deprecated
`THINKING_*` events. `EventType::is_attributable` is that list as a method,
and `Event::subagent_run_id` reads the tag off any event.
[Subagents](/ag-ui-rust/server/subagents/) is what to do with it.

## On the wire

`type` is the tag and the payload is flat beside it:

```rust
use ag_ui::{Event, EventType};

fn main() {
    // Every event type the protocol defines, in upstream order.
    assert_eq!(EventType::ALL.len(), 36);

    // The discriminator is the wire name, both ways.
    assert_eq!(EventType::TextMessageContent.as_str(), "TEXT_MESSAGE_CONTENT");
    assert_eq!(
        "TEXT_MESSAGE_CONTENT".parse::<EventType>().unwrap(),
        EventType::TextMessageContent,
    );

    let event = Event::text_message_content("msg-1", "Hello");
    assert_eq!(event.event_type(), EventType::TextMessageContent);
    assert_eq!(
        serde_json::to_string(&event).unwrap(),
        r#"{"type":"TEXT_MESSAGE_CONTENT","messageId":"msg-1","delta":"Hello"}"#,
    );
}
```

An event type this build does not know fails to deserialize. That is deliberate:
a frontend talking to a newer agent stops with an error naming the unknown type
rather than quietly rendering three quarters of a conversation.

## The `THINKING_*` family is deprecated

All five are still in the protocol, still parsed, and still emitted by producers
that predate the change — so they are here, and the SDK carries them. The
`REASONING_*` events replace them, and the replacements fix the reason the
originals were retired: `THINKING_TEXT_MESSAGE_CONTENT` carries no message id,
so a thinking block could only ever have one message in flight.

The Rust variants and payload structs are marked `#[deprecated]`. `ag-ui`'s
own event module carries `#![allow(deprecated)]` — it has to name these types in
the union, in `event_type()` and in the factories, and warning at itself for
implementing the spec as written helps nobody. The suppression is local to that
module, so a consumer that names one still gets the warning at its own use site,
which is where the decision to keep using it is actually being made.

`Event::is_deprecated` answers the question at runtime, without a match:

```rust
use ag_ui::Event;

fn main() {
    let event: Event = serde_json::from_str(r#"{"type":"THINKING_END"}"#).unwrap();

    assert_eq!(event.event_type().as_str(), "THINKING_END");
    assert!(event.is_deprecated());

    let current = Event::reasoning_end("msg-1");
    assert!(!current.is_deprecated());
}
```

:::note
There is one exception to the `#[deprecated]` marking. With the `utoipa` feature
on, the attribute is suppressed on the payload structs: utoipa 5.5's derive
emits a `.deprecated()` call on the `AllOf` builder it uses for
`#[serde(flatten)]` structs, and that builder has no such method, so the crate
would not compile. The deprecation stays unconditional on the
`Event::thinking_*` constructors, which utoipa never sees.
:::

## The `*_CHUNK` events

Three events — `TEXT_MESSAGE_CHUNK`, `TOOL_CALL_CHUNK` and
`REASONING_MESSAGE_CHUNK` — fold a start, its content and its end into a single
self-contained event. They exist for producers that cannot bracket their output,
which is most provider adapters: the upstream API does not tell them a message
has ended until the next one begins.

They carry their id and name **only on the first chunk**, so the end of one
stream is knowable only from the start of the next, or from the end of the run:

```text
TEXT_MESSAGE_CHUNK { messageId: "msg-1", delta: "Hel" }
TEXT_MESSAGE_CHUNK { delta: "lo" }
TEXT_MESSAGE_CHUNK { messageId: "msg-2", delta: "Bye" }   <- msg-1 just ended
```

On the consuming side that bookkeeping is `ag_ui::client::chunks`, which expands
a run of chunks back into the equivalent start/content/end triple before
anything else sees them. On the emitting side there is deliberately **no
handle**. The typestate emitters in `ag_ui::server` exist to make sure what you
open gets closed; a chunk has nothing to close, so wrapping one in an RAII
handle would only add a way to get it wrong. Emit them with `ctx.emit` — the
supported path, not a gap waiting for an API.

Interleaved parallel tool calls are the other case that belongs to `ctx.emit`.
Two open `ToolCallHandle`s at once is a borrow-check error *by design*, so a
provider streaming `args(a) args(b) args(a) end(a) end(b)` cannot be mirrored
handle-for-call. Either accumulate each call and emit it whole once its
arguments are complete — the only mapping that cannot splice two calls'
arguments into each other — or emit the interleaving yourself. The ordering
verifier keys everything by id, so it accepts the interleaved stream; what it
will not let you do is close a call you never opened. See
[Verification](/ag-ui-rust/design/verification/).

## What the binary transport cannot carry

The protocol also defines a protobuf encoding, and it is a lossy subset. The
`Event` message in upstream's `events.proto` is a `oneof` over **21** of the 36
types:

`TEXT_MESSAGE_START`, `TEXT_MESSAGE_CONTENT`, `TEXT_MESSAGE_END`,
`TEXT_MESSAGE_CHUNK`, `TOOL_CALL_START`, `TOOL_CALL_ARGS`, `TOOL_CALL_END`,
`TOOL_CALL_CHUNK`, `STATE_SNAPSHOT`, `STATE_DELTA`, `MESSAGES_SNAPSHOT`, `RAW`,
`CUSTOM`, `RUN_STARTED`, `RUN_FINISHED`, `RUN_ERROR`, `STEP_STARTED`,
`STEP_FINISHED`, `SUBAGENT_STARTED`, `SUBAGENT_FINISHED`, `SUBAGENT_ERROR`.

The other 15 have no binary representation at all: all seven `REASONING_*`
events, both `ACTIVITY_*` events, all five deprecated `THINKING_*` events, and
`TOOL_CALL_RESULT`. An agent that reasons, reports activities, or returns a tool
result — which is most of them — cannot express its stream in that format.

So `ag-ui` declines to encode any of it. The `protobuf` feature exists so a
build can negotiate and name the media type; the formatter's `encode` always
fails with `Error::UnsupportedTransport`. Silently dropping close to half the
protocol is worse than refusing. Use SSE, which carries all 36. The
[`encode::protobuf`](/ag-ui-rust/api/ag_ui/encode/protobuf/index.html)
module lists the covered set as `COVERED_EVENT_TYPES` and offers `is_covered`,
so a test can assert that a given stream would have survived the binary
transport.

This is also why the port is written against the TypeScript Zod schemas rather
than the proto definitions: a source of truth that is missing 15 of 36 events
cannot serve as one.
