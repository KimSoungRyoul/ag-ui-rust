---
title: How AG-UI works
description: The shape of an AG-UI exchange — a POST carrying a run input, answered by a stream of typed events.
---

AG-UI is small. One request starts a run; the answer is a stream of typed events that
describes everything the agent does until the run ends. There is no second endpoint, no
polling channel and no negotiation step.

This page is about the wire, not about this SDK's API. The types named here live in
`ag-ui`, which is deliberately just the vocabulary: no runtime, no I/O, no async.

## One request, one run

The request body is a `RunAgentInput`. Everything the agent is allowed to know about the
conversation arrives in it, because the agent is not assumed to remember anything:

| Field | What it carries |
| --- | --- |
| `threadId` | The conversation this run belongs to. |
| `runId` | This run's id, echoed on every lifecycle event. |
| `parentRunId` | The run that spawned this one, for nested or delegated agents. |
| `state` | Shared application state, as free-form JSON. Opaque to the protocol. |
| `messages` | Conversation history, oldest first. |
| `tools` | The tools the *client* is offering for this run. |
| `context` | Ambient context entries. |
| `forwardedProps` | Arbitrary passthrough, also opaque to the protocol. |
| `resume` | Answers to what a previous run paused on. Present only when resuming. |

```rust
use ag_ui::RunAgentInput;

let body = r#"{
    "threadId": "thread-1",
    "runId": "run-1",
    "state": { "tasks": [] },
    "messages": [{ "id": "m1", "role": "user", "content": "add a task" }],
    "tools": [],
    "context": []
}"#;

let input: RunAgentInput = serde_json::from_str(body).unwrap();

assert_eq!(input.thread_id.as_str(), "thread-1");
assert_eq!(input.messages.len(), 1);
assert_eq!(input.state["tasks"], serde_json::json!([]));
assert!(!input.is_resume());
```

Two consequences are worth stating plainly, because they surprise people.

**The thread lives on the client.** `threadId` names a conversation; it does not fetch
one. Nothing in the protocol says a server stores threads, and an agent that keeps no
history is a conforming agent. Persisting a conversation is the application's decision.

**The tool list is the client's offer, not the agent's menu.** AG-UI has no tool
discovery: an agent cannot ask for a tool it was not sent. It also is not an allow-list —
emitting a call for a name absent from `tools` is a well-formed stream, and it is how an
agent reports work it did itself. `docs/DESIGN.md` argues that case at length, and
[Tool calls](/ag-ui-rust/server/tools/) covers what it means when you are writing an
agent.

## The answer is a stream of events

The response is `text/event-stream`, one JSON object per SSE `data:` frame, in the order
the agent produced them. Each object carries a `type` discriminator holding a
SCREAMING_SNAKE_CASE name, and the payload's fields sit beside it rather than nested under
a key:

```rust
use ag_ui::{Event, EventStreamFormatter, SseFormatter, TextMessageRole};

let formatter = SseFormatter::new();
let run = [
    Event::run_started("thread-1", "run-1"),
    Event::text_message_start("msg-1", TextMessageRole::Assistant),
    Event::text_message_content("msg-1", "It is "),
    Event::text_message_content("msg-1", "sunny."),
    Event::text_message_end("msg-1"),
    Event::run_finished_success("thread-1", "run-1"),
];

let body: String = run
    .iter()
    .map(|event| formatter.encode_to_string(event).unwrap())
    .collect();

assert_eq!(
    body.lines().next(),
    Some(r#"data: {"type":"RUN_STARTED","threadId":"thread-1","runId":"run-1"}"#),
);
// One frame is one `data:` line and a blank line. Serialized JSON never
// contains a raw newline, so an event is never split across frames.
assert_eq!(body.matches("\n\n").count(), 6);
```

SSE is the interoperable default and the only transport this SDK fully implements. The
protocol also defines a binary media type, `application/vnd.ag-ui.event+proto`, and
`ag-ui` will negotiate it — but upstream's `events.proto` covers 18 of the protocol's
33 event types, so encoding to it would silently drop events, and there is no encoder
here. [Feature flags](/ag-ui-rust/reference/features/) has the details.

Content negotiation is by `Accept`. A missing or empty header is read as `*/*` and answers
SSE; a header that excludes everything the build can emit is the case that deserves a
`406`.

## The run lifecycle

Every run opens with `RUN_STARTED` and closes with exactly one of `RUN_FINISHED` or
`RUN_ERROR`. Nothing follows either.

```text
RUN_STARTED
  …everything the agent did…
RUN_FINISHED   or   RUN_ERROR
```

`RUN_FINISHED` carries an `outcome`, and "finished" includes *paused*:

- `{"type":"success"}` — the run completed.
- `{"type":"interrupt","interrupts":[…]}` — the agent is waiting on a human. The client
  collects the answers and sends them back on the *next* request, in `resume`, and the run
  continues there. See [Human in the loop](/ag-ui-rust/server/interrupts/).

The field is optional: producers that predate the interrupt protocol omit it, and a
consumer must read that as success.

`RUN_ERROR` carries a message and an optional machine-readable `code`. It arrives inside a
well-formed `200` response, because by the time an agent can fail the status line has long
since been sent — which is exactly what lets a client tell an agent failure from a dead
socket.

Inside a run, `STEP_STARTED` / `STEP_FINISHED` bracket named phases. They are optional and
purely descriptive; nothing else depends on them.

## Deltas, and the triples that frame them

Almost everything an agent produces arrives in pieces, so the stream is mostly *deltas*.
One logical thing — a message, a tool call, a reasoning block — is a `START`, some number
of content events, and an `END`, all carrying the same id:

```text
TEXT_MESSAGE_START     messageId=msg-1  role=assistant
TEXT_MESSAGE_CONTENT   messageId=msg-1  delta="It is "
TEXT_MESSAGE_CONTENT   messageId=msg-1  delta="sunny."
TEXT_MESSAGE_END       messageId=msg-1
```

The id is what makes the triple a triple. It is also what makes *interleaving* legible: a
model that asks for two tools at once produces two open calls whose events alternate, and
only the id says which fragment belongs to which call.

```text
TOOL_CALL_START   toolCallId=call-1  toolCallName=add_task
TOOL_CALL_START   toolCallId=call-2  toolCallName=add_task
TOOL_CALL_ARGS    toolCallId=call-1  delta="{\"title\":"
TOOL_CALL_ARGS    toolCallId=call-2  delta="{\"title\":"
TOOL_CALL_ARGS    toolCallId=call-1  delta="\"write it down\"}"
TOOL_CALL_END     toolCallId=call-1
TOOL_CALL_RESULT  toolCallId=call-1  content="{\"id\":1}"
```

Two details bite renderers, and both are demonstrated by the
[board-watch example](/ag-ui-rust/examples/board-watch/). Argument fragments are JSON split
at arbitrary byte offsets — a `\` and the `n` it escapes can arrive in different events —
so a fragment does not parse on its own. And text fragments are each valid UTF-8, because
a Rust `String` cannot be otherwise, but a *grapheme* can still be split across them: an
emoji built from a zero-width joiner arrives as several pieces.

### Chunk events

Some producers cannot bracket their output. A provider adapter often does not learn that a
message ended until the next one begins, so the protocol also defines
`TEXT_MESSAGE_CHUNK`, `TOOL_CALL_CHUNK` and `REASONING_MESSAGE_CHUNK`. A chunk carries its
id **only on the first one** of a sequence, and everything after it inherits that id.
Five chunk events can therefore be one message.

A consumer normalizes chunks back into explicit start/content/end triples before anything
else looks at them — `ag_ui::client` does that in its `chunks` stage, and
[The update stream](/ag-ui-rust/client/updates/) shows what a view sees as a result.

## The event families

There are **33 event types**, and `ag-ui` models them as one exhaustive `Event` enum
plus an `EventType` discriminator:

```rust
use ag_ui::{Event, EventType};

let event = Event::text_message_content("msg-1", "Hello");

assert_eq!(event.event_type(), EventType::TextMessageContent);
assert_eq!(EventType::TextMessageContent.as_str(), "TEXT_MESSAGE_CONTENT");
assert_eq!(EventType::ALL.len(), 33);
```

They group into eight families:

| Family | Events | What it is for |
| --- | --- | --- |
| Text message | `TEXT_MESSAGE_START` / `_CONTENT` / `_END` / `_CHUNK` | The reply the user reads. |
| Tool call | `TOOL_CALL_START` / `_ARGS` / `_END` / `_CHUNK` / `_RESULT` | A call, its argument JSON, and its result. |
| Reasoning | `REASONING_START` / `_END`, `REASONING_MESSAGE_START` / `_CONTENT` / `_END` / `_CHUNK`, `REASONING_ENCRYPTED_VALUE` | Thinking, kept separate from the reply. A block wraps one or more messages. |
| Thinking | `THINKING_START` / `_END`, `THINKING_TEXT_MESSAGE_START` / `_CONTENT` / `_END` | The reasoning family's deprecated predecessor. Still on the wire, still modelled. |
| State | `STATE_SNAPSHOT`, `STATE_DELTA`, `MESSAGES_SNAPSHOT` | The shared state, and a wholesale replacement of the history. |
| Activity | `ACTIVITY_SNAPSHOT`, `ACTIVITY_DELTA` | What the agent is *doing* — searching, reading, waiting — in a shape the client renders. |
| Run and step | `RUN_STARTED`, `RUN_FINISHED`, `RUN_ERROR`, `STEP_STARTED`, `STEP_FINISHED` | The lifecycle above. |
| Escape hatches | `RAW`, `CUSTOM` | A provider event forwarded verbatim, and an application-defined one. |

[Event reference](/ag-ui-rust/reference/events/) has the field-by-field version.

That `Event` is exhaustive rather than `#[non_exhaustive]` is a deliberate decision with a
price: a new protocol event becomes a compile error for everyone who matches on events,
and a major version of this SDK. The reasoning — under-coverage should be loud, and a `_`
arm is exactly what makes it quiet — is in
[Design commitments](/ag-ui-rust/design/commitments/).

## State moves by snapshot or by patch

Application state is free-form JSON that both sides mirror. An agent republishes it two
ways:

- `STATE_SNAPSHOT` carries the whole value and replaces what the client holds.
- `STATE_DELTA` carries an RFC 6902 JSON Patch and is applied to it.

Which one to send is a size judgement, not a protocol rule, and a client must handle both.
`ag_ui::server` makes the choice per publish: the first is always a snapshot, and a later
one is a delta unless the patch would be no smaller than the state it describes. On a small
state that happens often — [Shared state](/ag-ui-rust/server/state/) works through it.

`STATE_*` events are **unordered** with respect to everything else. They may arrive while a
message or a tool call is open, and that is not a violation; it is how an agent shows a
call landing rather than only reporting it afterwards. The flip side is that a state event
carries no association with whatever was open when it arrived, because the wire carries
none either.

## What the ordering rules actually are

The protocol's rules are about brackets and ids, and they are few:

| Rule | What it forbids |
| --- | --- |
| `run-ended` | Any event after `RUN_FINISHED` or `RUN_ERROR`. |
| `duplicate-run-started` | A second `RUN_STARTED`. |
| `duplicate-start` | Opening the same id twice. |
| `not-open` | Content or a terminator with no matching start. |
| `unknown-id` | Referencing an id the stream never introduced. |
| `open-at-finish` | `RUN_FINISHED` while a message, call or step is still open. |
| `out-of-order` | A legal event in an illegal place — a tool result before its `TOOL_CALL_END`. |

Those names are the ones this SDK reports, and it checks them on the **server** as well as
on the client, on by default in release builds. Emitting `TEXT_MESSAGE_CONTENT` without a
`START` is then a diagnostic where the bug is, rather than a confused frontend three
network hops downstream. [Verification](/ag-ui-rust/design/verification/) covers the state
machine and what it costs.

Note what is *not* on that list: nothing about which tools an agent may call, nothing about
where state events may appear, and nothing about how many messages a run must produce.

## Where the types are

The port is hand-written against the upstream TypeScript Zod schemas — not against the
protobuf definitions, which are a lossy subset. Since nothing in the compiler links the
two, `cargo run -p xtask -- drift-check` does: it compares a vendored snapshot of the
upstream event surface against the Rust types and fails the build when they diverge.

- [Event reference](/ag-ui-rust/reference/events/) — every event and its fields.
- [The crates](/ag-ui-rust/start/crates/) — where these types live and what builds on them.
- [ag_ui](/ag-ui-rust/api/ag_ui/index.html) — the rustdoc.
