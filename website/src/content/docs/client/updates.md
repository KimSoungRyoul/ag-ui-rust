---
title: The update stream
description: The values a run yields as it proceeds, the three ways a run can end, and why one of those two enums is exhaustive and the other is not.
---

A run is a stream of `Update`s. Each one is something a view should react to —
"this message grew", "the state changed, here it is typed", "the agent is
waiting on you" — with chunk normalization, protocol verification and delta
application already done. One `Update` is one redraw.

`Update<S>` is generic over the state type, which is where a `Session`'s second
parameter shows up. It is `serde_json::Value` unless the session was asked for
something better.

## Every variant

```rust
// src/render.rs
use ag_ui::client::{RunEnd, Update};
use serde_json::Value;

fn render(update: Update<Value>) {
    match update {
        // A message was created, appended to, or completed. `index` is into
        // `Session::messages`, and `message` is the whole thing as it now
        // stands — so a view can redraw one row or re-read the lot.
        Update::Message(message) => {
            println!("message {} at {}: {:?}", message.id, message.index, message.change);
        }

        // MESSAGES_SNAPSHOT replaced the conversation. Messages may have
        // disappeared, so redraw all of it.
        Update::Messages(messages) => println!("{} messages replaced", messages.len()),

        // The application state, in the caller's type. A snapshot and a patch
        // arrive the same way; nothing here can tell which.
        Update::State(state) => println!("state is now {state}"),

        // Reasoning text, kept separate from the reply.
        Update::Reasoning(reasoning) => println!("thinking: {}", reasoning.text),

        // The run paused and needs a human — one update per pending interrupt.
        Update::Interrupt(interrupt) => println!("waiting on {}", interrupt.id),

        // A subagent was announced, resumed, finished, suspended or failed.
        // Only the lifecycle: what it *says* arrives as ordinary messages.
        Update::Subagent(subagent) => {
            println!("{} {:?}", subagent.subagent.name, subagent.change);
        }

        // A malformed stream, a patch that would not apply, a transport
        // failure, a RUN_ERROR. Not necessarily fatal.
        Update::Error(error) => eprintln!("{error}"),

        // Always the last update of a run, on every path out.
        Update::Done(end) => println!("{}", ended(&end)),

        // `Update` is `#[non_exhaustive]`: it is a view model rather than a
        // wire type, and a new kind of thing worth redrawing is not a protocol
        // change.
        _ => {}
    }
}

/// How a run ended, in one phrase. Three arms and no `_`; see below.
fn ended(end: &RunEnd) -> String {
    match end {
        RunEnd::Success { .. } => "success".to_owned(),
        RunEnd::Interrupted { interrupts } => format!("interrupted on {}", interrupts.len()),
        RunEnd::Failed { message, .. } => format!("failed: {message}"),
    }
}

fn main() {
    render(Update::State(Value::Bool(true)));
    assert_eq!(ended(&RunEnd::Success { result: None }), "success");
}
```

A `MessageUpdate` carries `index`, `id`, `change` and the assembled `message`.
The `change` is a `MessageChangeKind`, and it is the part a renderer spends most
of its time on: `Started`, `Content { delta }`, `Ended`, `ToolCallStarted`,
`ToolCallArgs`, `ToolCallEnded`, `ToolResult`, `Activity`, `EncryptedValue`.
What to do with those is [Rendering a run](/ag-ui-rust/client/rendering/).

:::note
`Update` is per *event*, not per entity. Forty text deltas are forty
`Update::Message`s under one id, and two tool calls in flight interleave their
events, so consecutive updates need not belong to the same call. Arrival order
is the only nesting signal there is, and what it costs a renderer to give that
up is the whole of [Rendering a run](/ag-ui-rust/client/rendering/).
:::

## Subagents

An agent that delegates announces each child with `SUBAGENT_STARTED`, and everything the
child produces arrives tagged with that invocation's id. On the consuming side that is
two things. `Update::Subagent` is the **lifecycle only** — one update when a subagent is
announced, and one when it finishes, suspends or fails — carrying a `SubagentChangeKind`
(`Started`, `Resumed`, `Finished`, `Suspended`, `Failed`) and the whole `Subagent` entry
as it now stands: its name, description, parent links and `SubagentStatus`. What the
subagent *says* arrives as ordinary `Update::Message`s and `Update::Reasoning`s, whose
messages carry the producer on `Message::subagent_run_id()`. A view groups by that and
uses the lifecycle for the group's header.

The registry is on the session: `session.subagents()` lists every invocation announced so
far, across runs, and `session.subagent(id)` looks one up — which is what a renderer does
with the id on a message it is about to draw. A subagent that paused on an interrupt stays
`Suspended` until the run that resumes it announces the same id again, and that arrives as
`Resumed` rather than as a second subagent.

```rust
// src/main.rs
use ag_ui::client::{Session, SubagentChangeKind, SubagentStatus, Update, transport::ReplayTransport};
use ag_ui::{Event, SubagentFinishedEvent, SubagentOutcome, SubagentRunId, TextMessageRole};
use futures_util::StreamExt;
use serde_json::json;

#[tokio::main]
async fn main() {
    let tagged = |event: Event| event.with_subagent_run_id("sub-1");
    let transport = ReplayTransport::new([
        Event::run_started("thread-1", "run-1"),
        Event::subagent_started("sub-1", "researcher"),
        tagged(Event::text_message_start("msg-1", TextMessageRole::Assistant)),
        tagged(Event::text_message_content("msg-1", "Three sources.")),
        tagged(Event::text_message_end("msg-1")),
        Event::SubagentFinished(
            SubagentFinishedEvent::new("sub-1")
                .with_result(json!({ "sources": 3 }))
                .with_outcome(SubagentOutcome::Success),
        ),
        Event::text_message_start("msg-2", TextMessageRole::Assistant),
        Event::text_message_content("msg-2", "Thanks."),
        Event::text_message_end("msg-2"),
        Event::run_finished_success("thread-1", "run-1"),
    ]);

    let mut session = Session::<_>::new(transport, "thread-1");
    let mut lifecycle = Vec::new();

    let mut run = session.send("research this");
    while let Some(update) = run.next().await {
        if let Update::Subagent(subagent) = update {
            lifecycle.push((subagent.change, subagent.subagent.status));
        }
    }
    drop(run);

    // Two lifecycle updates: the announcement, then the close with its payload.
    assert_eq!(
        lifecycle,
        [
            (SubagentChangeKind::Started, SubagentStatus::Running),
            (
                SubagentChangeKind::Finished,
                SubagentStatus::Finished { result: Some(json!({ "sources": 3 })) },
            ),
        ]
    );

    // The registry outlives the run, and a message names its producer.
    let researcher = session.subagent(&SubagentRunId::new("sub-1")).expect("announced");
    assert_eq!(researcher.name, "researcher");
    assert_eq!(session.subagents().len(), 1);

    let owner = |i: usize| session.messages()[i].subagent_run_id().map(|id| id.as_str());
    assert_eq!(owner(1), Some("sub-1"));   // the subagent's reply
    assert_eq!(owner(2), None);            // the parent's own
}
```

The rest of what a subagent changes on this side is already done before an update reaches
you. The verifier checks that a tagged continuation names the subagent that opened the
entity, the chunk normalizer keeps one open stream *per subagent* so concurrent children
do not splice into each other, and a message's `metadata()` is the merge of every event
that built it — key by key, last write wins, which is how a token count that is only known
at the end lands on the message. [Subagents](/ag-ui-rust/server/subagents/) is the
producing half.

## The three ways a run ends

Every run ends with exactly one `Update::Done`, and the stream ends there. On
every path out: the agent finishing, the agent failing, and the transport dying
mid-sentence.

```rust
// src/render.rs
use ag_ui::client::RunEnd;

/// Whether the input goes live again — the decision `RunEnd` exists for.
fn prompt_again(end: &RunEnd) -> bool {
    match end {
        // The agent finished.
        RunEnd::Success { .. } => true,
        // The agent is waiting. Answer the interrupts instead of typing.
        RunEnd::Interrupted { .. } => false,
        // The run failed, or the transport stopped before it could finish.
        RunEnd::Failed { .. } => true,
    }
}

fn main() {
    assert!(prompt_again(&RunEnd::Success { result: None }));
    assert!(!prompt_again(&RunEnd::Interrupted { interrupts: Vec::new() }));
}
```

| Variant | Fields |
| --- | --- |
| `Success` | `result: Option<Value>` — the agent's return value, if it sent one. |
| `Interrupted` | `interrupts: Vec<Interrupt>` — the same ones that arrived individually as `Update::Interrupt`, and are on `Session::interrupts` until the next run. |
| `Failed` | `message: String`, `code: Option<String>` — what went wrong, and the machine-readable code when the agent sent one. |

Three arms and no `_`, because `RunEnd` is **exhaustive**. That is deliberate,
and it is the opposite of what every error type in this workspace does. A run
ends in one of three ways because the protocol says so — `RUN_FINISHED` with a
success outcome, `RUN_FINISHED` with an interrupt outcome, or `RUN_ERROR`, which
is also how a truncated stream is reported. A fourth would be a wire-contract
change, and this is the match a front-end most wants the compiler's help with:
its arms decide whether the prompt comes back, whether an answer is owed, and
whether anything failed. A `_` arm there is exactly the construct that turns "a
new way for a run to end" into no diagnostic at all.

`Update` keeps `#[non_exhaustive]` for the mirror-image reason. It is a view
model, not a wire type. [Design commitments](/ag-ui-rust/design/commitments/)
has the general form of the argument, which the protocol's `Event` enum is the
main case of.

## `Success` means the agent said so

It does not mean nothing went wrong. The two come apart, and the gap is entirely
made of the client's *own* diagnostics — a protocol violation the verifier
caught, or a state patch that would not apply. Those arrive as `Update::Error`
and the run carries on to end successfully, because the agent is neither told
nor asked.

```rust
// src/main.rs
use ag_ui::client::{RunEnd, Session, Update, transport::ReplayTransport};
use ag_ui::{Event, PatchOperation};
use futures_util::StreamExt;
use serde_json::json;

#[tokio::main]
async fn main() {
    let transport = ReplayTransport::new([
        Event::run_started("thread-1", "run-1"),
        Event::state_snapshot(json!({ "count": 1 })),
        // Replacing a path that does not exist. RFC 6902 patches are
        // all-or-nothing, so the state is left exactly as it was.
        Event::state_delta(vec![PatchOperation::replace("/missing/deeply", json!(2))]),
        Event::state_delta(vec![PatchOperation::replace("/count", json!(2))]),
        Event::run_finished_success("thread-1", "run-1"),
    ]);

    let mut session = Session::<_>::new(transport, "thread-1");
    let updates: Vec<_> = session.send("go").collect().await;

    let errors: Vec<String> = updates
        .iter()
        .filter_map(|update| match update {
            Update::Error(error) => Some(error.to_string()),
            _ => None,
        })
        .collect();

    assert_eq!(errors.len(), 1);
    assert!(errors[0].contains("state patch failed"));

    // The run carried on: the later delta applied, and the agent called it a
    // success. A view routing on `Done` alone would call this run clean.
    assert!(matches!(updates.last(), Some(Update::Done(RunEnd::Success { .. }))));
    assert_eq!(session.raw_state(), &json!({ "count": 2 }));
}
```

So `Update::Error` is not a terminal signal. When it *is* fatal the matching
`Update::Done` follows it, and `RunEnd::Failed` always has its `Update::Error`
in front of it — on every path, including the one where the transport simply
stopped. Track the errors as they arrive if the difference matters to you.
`board-watch` prints them as they land, which is how its transcript shows a run
that both complained and succeeded.

## An event this build does not know

The protocol's `Event` enum is exhaustive too, and the failure it is exhaustive
to correct is silent under-coverage: the community `ag-ui-core 0.1.0` declares
24 event variants against the 32 the spec had then (36 now), and nobody noticed,
because a `_` arm in every consumer is what silence looks like. The consequence at the type level
is that adding an event is a major version of this SDK. The consequence at
runtime is here: an unrecognised `type` on the wire fails to deserialize, and
the run stops. The transport below is the smallest one that can demonstrate it —
[Transports](/ag-ui-rust/client/transports/) explains the shape:

```rust
// src/main.rs
use ag_ui::client::transport::{Transport, TransportFuture, boxed_stream, decode_events};
use ag_ui::client::{RunEnd, Session, Update};
use ag_ui::encode::sse::frame;
use ag_ui::{Event, RunAgentInput, SseFormatter, TextMessageRole};
use futures_util::StreamExt;

/// A transport that answers every run with the same recorded response body.
struct Recorded(String);

impl Transport for Recorded {
    fn run(&self, _input: RunAgentInput) -> TransportFuture {
        let body = self.0.clone();
        Box::pin(async move {
            let chunks = futures_util::stream::iter([Ok::<_, std::io::Error>(body)]);
            Ok(boxed_stream(decode_events(chunks)))
        })
    }
}

#[tokio::main]
async fn main() {
    let sse = SseFormatter::new();
    let mut body = String::new();
    for event in [
        Event::run_started("thread-1", "run-1"),
        Event::text_message_start("msg-1", TextMessageRole::Assistant),
        Event::text_message_content("msg-1", "Half a conversation."),
        Event::text_message_end("msg-1"),
    ] {
        body.push_str(&sse.encode_to_string(&event).expect("encodes"));
    }
    // An event from a newer agent, framed by hand because this build has no
    // variant to encode it with.
    body.push_str(&frame(r#"{"type":"TELEPATHY_START","messageId":"msg-2"}"#));

    let mut session = Session::<_>::new(Recorded(body), "thread-1");
    let updates: Vec<_> = session.send("go").collect().await;

    let Some(Update::Done(RunEnd::Failed { message, .. })) = updates.last() else {
        panic!("an unknown event must still end the run: {updates:?}");
    };
    // The error names the type it did not recognise.
    assert!(message.contains("TELEPATHY_START"), "unhelpful: {message}");

    // What arrived before it is still there — the failure is loud, not lossy.
    assert_eq!(session.messages().len(), 2);
}
```

A frontend talking to a newer agent stops with an error naming the unknown type,
rather than quietly rendering three quarters of a conversation. Same for a
transport that dies mid-sentence: the truncation is reported as an
`Update::Error` and the run ends `Failed`, because a view that re-enables its
input on `Done` must not be left waiting by a dropped connection. Turning
verification off changes how precisely the truncation is described, not whether
it is reported.

## Errors

`Update::Error` carries `ag_ui::client::Error`, which is `#[non_exhaustive]` —
new transports and validation rules are expected to add variants without a
breaking release. The variants worth routing on:

| Variant | What happened |
| --- | --- |
| `Protocol` | The stream parsed but broke an ordering rule. The offending event was not applied. |
| `Patch` | An RFC 6902 patch could not be applied. The target document is unchanged. |
| `State` | The state did not deserialize into `S`. `raw_state` is still correct. |
| `Run` | The agent sent `RUN_ERROR`. |
| `Json` / `Decode` | The bytes were not a valid event, or not well-formed `text/event-stream`. |
| `Http` / `Transport` / `Config` | The request never became a stream, or stopped being one. |

Everything else falls through, which is the point of the attribute: nobody wants
an exhaustive match over failure modes, and a new failure mode is not a protocol
change.

## Next

- [Rendering a run](/ag-ui-rust/client/rendering/) — what to do with
  `Update::Message` when two tool calls are open at once.
- [Transports](/ag-ui-rust/client/transports/) — where the events came from.
- [`Update`](/ag-ui-rust/api/ag_ui/client/session/enum.Update.html) and
  [`RunEnd`](/ag-ui-rust/api/ag_ui/client/session/enum.RunEnd.html) in the API
  docs.
