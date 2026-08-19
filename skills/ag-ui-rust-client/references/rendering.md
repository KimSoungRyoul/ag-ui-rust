# Rendering a run

Read this when building a view over `Update`s — a TUI, a web frontend, a transcript.

## Consecutive updates need not belong together

Two calls open before either closes, and their fragments alternate on the wire. The client
does not reorder them, so they alternate in the update stream too:

```rust
use ag_ui::client::{MessageChangeKind, Session, Update, transport::ReplayTransport};
use ag_ui::Event;
use futures_util::StreamExt;

#[tokio::main]
async fn main() {
    let transport = ReplayTransport::new([
        Event::run_started("thread-1", "run-1"),
        Event::tool_call_start("call-1", "add_task"),
        Event::tool_call_start("call-2", "add_task"),
        Event::tool_call_args("call-1", r#"{"title":"#),
        Event::tool_call_args("call-2", r#"{"title":"#),
        Event::tool_call_args("call-1", r#""write it down"}"#),
        Event::tool_call_args("call-2", r#""read it back"}"#),
        Event::tool_call_end("call-1"),
        Event::tool_call_end("call-2"),
        Event::run_finished_success("thread-1", "run-1"),
    ]);

    let mut session = Session::<_>::new(transport, "thread-1");
    let updates: Vec<_> = session.send("add two things").collect().await;

    let fragments: Vec<String> = updates
        .iter()
        .filter_map(|update| match update {
            Update::Message(message) => match &message.change {
                MessageChangeKind::ToolCallArgs { tool_call_id, .. } => {
                    Some(tool_call_id.to_string())
                }
                _ => None,
            },
            _ => None,
        })
        .collect();

    // Four fragments, two calls, strictly alternating. A renderer that appends
    // each delta to "the call it is in the middle of" writes one garbled line.
    assert_eq!(fragments, ["call-1", "call-2", "call-1", "call-2"]);
}
```

## Arrival order is the only nesting

The protocol has no containment — it has a sequence, and where an event lands is all the wire
says about what was open. An agent doing a tool's work *while the call is open* publishes
state between `TOOL_CALL_ARGS` and `TOOL_CALL_END`, and the resulting `Update::State` carries
no mention of the call. That is not a missing field: under parallel calls the wire itself does
not say which call the state belongs to, so any attribution would be invented.

Drawing in arrival order shows what happened. Buffering by entity is choosing to reorder:

```rust
use ag_ui::client::{MessageChangeKind, Session, Update, transport::ReplayTransport};
use ag_ui::{Event, ToolCallId};
use futures_util::StreamExt;
use serde_json::json;

/// The tail of a call id — enough to tell two apart in one transcript.
fn short(id: &ToolCallId) -> &str {
    let id = id.as_str();
    id.rsplit('-').next().unwrap_or(id)
}

/// One line per update, in arrival order, each tool line naming its call.
fn in_order(update: &Update, out: &mut Vec<String>) {
    match update {
        Update::Message(message) => match &message.change {
            MessageChangeKind::ToolCallStarted { tool_call_id, name } => {
                out.push(format!("call {name} ({})", short(tool_call_id)));
            }
            MessageChangeKind::ToolCallArgs { tool_call_id, delta } => {
                out.push(format!("args ({}) {delta}", short(tool_call_id)));
            }
            // `ToolCallEnded` carries only the id — the name arrived on
            // `ToolCallStarted`, so a renderer that wants it here keeps a map.
            MessageChangeKind::ToolCallEnded { tool_call_id } => {
                out.push(format!("end  ({})", short(tool_call_id)));
            }
            _ => {}
        },
        Update::State(state) => out.push(format!("state {state}")),
        _ => {}
    }
}

/// The whole call on one line — which means the line is written when the call
/// *closes*, so anything that arrived during it is already on screen.
#[derive(Default)]
struct Grouped {
    open: Vec<(ToolCallId, String, String)>,
}

impl Grouped {
    fn draw(&mut self, update: &Update, out: &mut Vec<String>) {
        match update {
            Update::Message(message) => match &message.change {
                MessageChangeKind::ToolCallStarted { tool_call_id, name } => {
                    self.open
                        .push((tool_call_id.clone(), name.clone(), String::new()));
                }
                MessageChangeKind::ToolCallArgs { tool_call_id, delta } => {
                    if let Some(call) = self.open.iter_mut().find(|call| &call.0 == tool_call_id) {
                        call.2.push_str(delta);
                    }
                }
                MessageChangeKind::ToolCallEnded { tool_call_id } => {
                    if let Some(at) = self.open.iter().position(|call| &call.0 == tool_call_id) {
                        let (_, name, args) = self.open.remove(at);
                        out.push(format!("call {name} {args}"));
                    }
                }
                _ => {}
            },
            Update::State(state) => out.push(format!("state {state}")),
            _ => {}
        }
    }
}

#[tokio::main]
async fn main() {
    let transport = ReplayTransport::new([
        Event::run_started("thread-1", "run-1"),
        Event::tool_call_start("call-1", "add_task"),
        Event::tool_call_args("call-1", r#"{"title":"draft "#),
        Event::state_snapshot(json!({ "open": 1 })),
        Event::tool_call_args("call-1", r#"the agenda"}"#),
        Event::tool_call_end("call-1"),
        Event::run_finished_success("thread-1", "run-1"),
    ]);

    let mut session = Session::<_>::new(transport, "thread-1");
    let updates: Vec<_> = session.send("add one thing").collect().await;

    let mut ordered = Vec::new();
    let mut grouped = Vec::new();
    let mut state = Grouped::default();
    for update in &updates {
        in_order(update, &mut ordered);
        state.draw(update, &mut grouped);
    }

    assert_eq!(
        ordered,
        [
            r#"call add_task (1)"#,
            r#"args (1) {"title":"draft "#,
            r#"state {"open":1}"#,
            r#"args (1) the agenda"}"#,
            r#"end  (1)"#,
        ]
    );

    // The state that happened *during* the call is drawn before it.
    assert_eq!(
        grouped,
        [
            r#"state {"open":1}"#,
            r#"call add_task {"title":"draft the agenda"}"#,
        ]
    );
}
```

Neither is more correct. Pick grouped to read a conversation and arrival order to debug one.
What cannot be had is a call drawn as one line **and** kept in order.

## What the client already handles

- **Chunk events.** `*_CHUNK` carries its id only on the first event; the normalizer expands
  them into explicit `Started` / `Content` / `Ended`, so a renderer never sees the chunk form.
- **Unterminated messages.** If the producer never closes its last message, the end of the
  stream closes it — a view hiding its typing indicator on `Ended` will not spin forever.
- **Malformed streams.** An event breaking an ordering rule is reported as `Update::Error`
  and **not applied**, so the conversation never contains state assembled from a broken
  stream. (Except the event that ends the run, which is applied anyway so the caller is not
  left waiting.)
- **Reasoning lifecycle.** The protocol brackets a thought twice under the same id;
  `ReasoningChangeKind::Started` / `Ended` arrive **once**, so no dedupe is needed.

## Redraw hints

- `MessageUpdate::index` is the row that changed. Redraw one row.
- `Update::Messages` means messages may have *disappeared*. Redraw all of it.
- `Update::State` carries the state by value, so a view can hold it after the run moved on.
- `Update::Error` is not terminal.
- `Update::Done` is where the input goes live again.

## Sources

- <https://kimsoungryoul.github.io/ag-ui-rust/client/rendering/>
- <https://kimsoungryoul.github.io/ag-ui-rust/client/updates/>
