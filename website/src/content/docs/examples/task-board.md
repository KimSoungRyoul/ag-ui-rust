---
title: task-board (agent)
description: A read through the worked agent example — streamed text, tool calls, shared state, an A2UI surface and a pause for a human.
---

`task-board` is a workshop task board spoken over AG-UI: an agent you can host, and a
terminal that talks to it, in one crate. It exists to be the SDK's first outside consumer.
It uses `ag_ui::server`, `ag_ui::client`, `ag_ui::axum`, `ag-ui-a2ui` and `ag-ui` the way
anyone else would — public items only, nothing reached into — and every major subsystem
appears in it exactly once.

[Read the source on GitHub](https://github.com/KimSoungRyoul/ag-ui-rust/tree/main/examples/task-board).

| Subsystem | Where it shows up |
| --- | --- |
| Streamed text | the reply, one word per `TEXT_MESSAGE_CONTENT` |
| Reasoning | `ctx.think()` — one line about what the agent made of the message |
| Tool calls | `add_task`, `complete_task`, `estimate`, `clear_board`, run by the agent |
| Shared state | the board, published as `STATE_SNAPSHOT` then `STATE_DELTA`s |
| A2UI | the board as a surface, in an `a2ui_operations` tool-result envelope |
| Human in the loop | `clear` pauses the run and waits for a yes |
| Steps | the whole turn is bracketed by `STEP_STARTED` / `STEP_FINISHED` |
| Subagents | `research` delegates to two in turn, and everything each emits is attributed to it |

## Running it

Two terminals. The agent first, on port 8080:

```sh
cargo run -p task-board -- serve
```

Then the client:

```sh
cargo run -p task-board -- chat
```

Type `add draft the agenda, book the room`, then `list`, `complete 1`, `clear`. `quit` or
Ctrl-D ends it, and `--port`, `--url` and `--thread` are the flags. Piping works too, which
is how the tests drive it:

```sh
printf 'add draft the agenda, book the room\nlist\n' | cargo run -p task-board -- chat
```

A turn looks like this:

```text
you> add draft the agenda, book the room
  ~ adding 2 task(s)
  · add_task({"title":"draft the agenda"})
  [state] 1 open · 0 done
    → {"id":1,"title":"draft the agenda"}
  · add_task({"title":"book the room"})
  [state] 2 open · 0 done
    → {"id":2,"title":"book the room"}
  agent> Added #1 draft the agenda, #2 book the room. 2 open · 0 done
  · render_a2ui({"surfaceId":"task-board"})
    ┌ a2ui surface
    │ Workshop board
    │ 2 open · 0 done
    │ [ ] #1 draft the agenda
    │ [ ] #2 book the room
    └
```

`~` is reasoning, `·` is a tool call, `→` is its result, `[state]` is the board after a
state event, and the box is the A2UI surface — drawn by walking the component tree and
resolving each binding, which is as far as a terminal can honestly go towards rendering.

The agent is **deterministic**. The board moves because someone typed `add`, not because a
model decided to call a tool, which is what makes those transcripts assertable to the
character. `tests/flows.rs` drives the same `converse` function the binary runs, with a
scripted `&[u8]` for a keyboard and a `Vec<u8>` for a screen, so the transcripts are
assertions rather than illustrations.

## Why the state lands in the middle of a tool call

Look at the transcript again: `[state]` appears *between* the call and its result. That is
not an artifact of the renderer. It is where the agent does the work.

`ctx.tool_call(…)` returns a handle that reaches the run state as well as the event sink,
so a mutation happens with the call still open — `TOOL_CALL_START`, the arguments, the
`STATE_*` event, then `TOOL_CALL_END` and the result. The protocol allows it because state
events are unordered, and it is what lets a client show a call *in flight* rather than only
after it is done:

```rust
use ag_ui::{Event, EventType, RunAgentInput, RunOutcome};
use ag_ui::server::{Agent, Result, RunContext, run};
use futures_util::StreamExt;
use serde::{Deserialize, Serialize};
use serde_json::json;

#[derive(Default, Serialize, Deserialize)]
struct Board {
    tasks: Vec<String>,
}

struct Adder;

impl Agent for Adder {
    type State = Board;

    async fn run(&self, ctx: &mut RunContext<Board>) -> Result<RunOutcome> {
        let mut call = ctx.tool_call("add_task")?;
        call.args_json(&json!({ "title": "draft the agenda" }))?;

        // The board moves while the call is still open.
        call.state_mut().tasks.push("draft the agenda".into());
        call.publish_state()?;

        call.result_json(&json!({ "id": 1 }))?;

        Ok(RunOutcome::Success)
    }
}

#[tokio::main]
async fn main() {
    let types: Vec<EventType> = run(Adder, RunAgentInput::new("thread-1", "run-1"))
        .map(|event| event.expect("the stream should not break").event_type())
        .collect()
        .await;

    assert_eq!(
        types,
        [
            EventType::RunStarted,
            EventType::ToolCallStart,
            EventType::ToolCallArgs,
            EventType::StateSnapshot,
            EventType::ToolCallEnd,
            EventType::ToolCallResult,
            EventType::RunFinished,
        ]
    );
}
```

An earlier draft of the emitter API held only the event sink, which left the state
unreachable for as long as anything was open and forced every agent to mutate *before*
announcing the call it was mutating for. Same events, different order — and the order is
what decides whether a client can watch a call land.

The cost lands on the consumer: an `Update::State` carries no association with the call it
arrived during, because the wire carries none either. `board-watch` is the example that
takes that seriously — see [board-watch](/ag-ui-rust/examples/board-watch/).

## Snapshot or delta, decided per publish

Two adds means two publishes, and nothing in the transcript tells you what either was on
the wire, because the client applied both into the same `Board`. The server decides each
time: the first publish is always a `STATE_SNAPSHOT`, and later ones are a `STATE_DELTA`
*unless the patch would be no smaller than the state it describes*.

On a board this small it would be — resending two short tasks costs less than the RFC 6902
patch that adds one — so both go out as snapshots. Give the tasks realistic titles and the
second becomes a delta:

```text
STATE_SNAPSHOT {"tasks":[{"id":1,"title":"write the workshop agenda and circulate it",…}],"nextId":1}
STATE_DELTA    [{"op":"add","path":"/tasks/1","value":{"id":2,…}},{"op":"replace","path":"/nextId",…}]
```

Both encodings are pinned by a test, because "it works" here means the client lands in the
same place either way. [Shared state](/ag-ui-rust/server/state/) is the reference for it.

## The human in the loop, over two requests

`clear` is the one destructive command, so the agent stops and asks. The answer travels on
a **second** request:

```text
you> clear
  ~ clearing cannot be undone, so a human decides
  agent> Clearing drops 1 task(s) and cannot be undone.
  ?? Clear the board? 1 task(s) will be removed.
  [y/N] y
  ~ a human approved clearing the board
  · clear_board({})
  [state] nothing on the board
    → {"removed":1}
  agent> Cleared 1 task(s). The board is empty.
```

The first run ends `RUN_FINISHED` with an interrupt outcome; the client collects the answer
and sends it back in `resume`; the agent reads it with `ctx.resume_for(…)` and carries on.
The declined path reaches the same code with `ResumeStatus::Cancelled` and answers no tool
call at all, which is what the test asserts —
`a_paused_run_ends_as_interrupted_and_resumes_as_its_own_run` in `tests/flows.rs`.

[Human in the loop](/ag-ui-rust/server/interrupts/) has the mechanics.

## Delegating to subagents

`research` is the one command that delegates. Inside the same `board` step the supervisor
opens two subagents in turn — `scope`, then `risks` — and each streams a sentence and adds
a task through the same `add_task` tool the other commands use:

```text
you> research onboarding
  ~ delegating "onboarding" to two subagents
  ⟂ scope started
  scope> Scoping "onboarding": one deliverable, one owner.
  · [scope] add_task({"title":"scope onboarding"})
  [state] 1 open · 0 done
    → {"id":1,"title":"scope onboarding"}
  ⟂ scope done
  ⟂ risks started
  risks> One risk for "onboarding": nobody owns the follow-up.
  · [risks] add_task({"title":"name a follow-up owner for onboarding"})
  [state] 2 open · 0 done
    → {"id":2,"title":"name a follow-up owner for onboarding"}
  ⟂ risks done
  agent> Research on "onboarding" added #1 scope onboarding, #2 name a follow-up owner for onboarding. 2 open · 0 done
```

`⟂` is a subagent starting or finishing, and `scope>` and `· [scope]` are its sentence and
its tool call. The agent tags none of this itself: `ctx.subagent("scope")` returns a handle
that dereferences to the run context, and everything emitted through it — the sentence, the
call, the board it publishes — goes out with that invocation's `subagentRunId`, bracketed
by `SUBAGENT_STARTED` and `SUBAGENT_FINISHED`. On the wire, the first delegate is:

```text
SUBAGENT_STARTED    {"subagentRunId":"r1-sub-1","name":"scope"}
TEXT_MESSAGE_START  {"messageId":"r1-msg-2","role":"assistant","subagentRunId":"r1-sub-1"}
TOOL_CALL_START     {"toolCallId":"r1-call-1","toolCallName":"add_task","subagentRunId":"r1-sub-1"}
STATE_SNAPSHOT      {"snapshot":{…},"subagentRunId":"r1-sub-1"}
TOOL_CALL_RESULT    {"messageId":"r1-msg-3","toolCallId":"r1-call-1","content":"…","role":"tool","subagentRunId":"r1-sub-1"}
SUBAGENT_FINISHED   {"subagentRunId":"r1-sub-1","result":{"added":1},"outcome":{"type":"success"}}
```

The client reads it back as `Update::Subagent` for the `⟂` lines and, for everything else,
`Message::subagent_run_id()` resolved to a name through `session.subagent(id)` — mid-run,
through `RunStream::session()`. The supervisor's own reply comes after both delegates,
untagged, which is why it prints as `agent>`.

A client written before subagents existed rejects an event type it does not know. An
endpoint can flatten or drop the subagent surface for such a client —
`SubagentVisibility::inline()` and `SubagentVisibility::hidden()` are transformers — but
this example ships the full one. [Subagents](/ag-ui-rust/server/subagents/) has the
mechanics.

## Where the board lives

**On the client.** The agent stores nothing between runs: it reads the board out of
`RunAgentInput.state`, publishes what it changed, and forgets. `Session` is what carries it
from one run to the next, along with the conversation.

Two consequences are worth absorbing before you build on this shape:

- A second `chat` process joining the same thread id starts from an **empty board**. The
  thread id names the conversation; it does not fetch one. Persisting a thread is the
  application's job, and this example does not do it.
- The agent knows whether a surface is already on screen only because the client sends the
  history back. `find_prior_surface` replays the A2UI operations already in the
  conversation, which is what makes the second render an `updateComponents` rather than a
  second `createSurface`. [Authoring surfaces](/ag-ui-rust/a2ui/authoring/) covers that.

## Letting a model do the talking

```sh
export AG_UI_LLM_API_KEY=…        # or GEMINI_API_KEY
cargo run -p task-board -- serve --llm
```

Or against a model on your own machine, with no key at all:

```sh
export AG_UI_LLM_BASE_URL=http://localhost:11434/v1
export AG_UI_LLM_MODEL=qwen3:4b
cargo run -p task-board -- serve --llm
```

The model rewrites the reply sentence and nothing else — ids, counts and state transitions
stay deterministic, and a model that fails does not fail the run: the scripted sentence
goes out and the failure is reported as reasoning.

There is no LLM crate in the dependency tree. `src/llm.rs` is `reqwest` and two `serde`
structs, which is the demonstration of the design decision rather than a shortcut: this SDK
depends on no model client, so an example that needed one would be arguing against it.

## The code

| File | What is in it |
| --- | --- |
| `src/board.rs` | `Board` and `Task`, the four tool schemas, the A2UI surface. Knows nothing about AG-UI events. |
| `src/agent.rs` | the `impl Agent`, the command parser, and the `research` delegation |
| `src/chat.rs` | the terminal client, generic over its input and output |
| `src/llm.rs` | the optional `--llm` phrasing |
| `src/main.rs` | the CLI, argument parsing by hand |
| `tests/flows.rs` | every flow above, against a server on a real port |

`src/board.rs` is the one to read first. Keeping the domain free of the protocol — the
state is a plain `serde` struct, the tools are `Tool` definitions, the surface is a
component tree — is what makes `src/agent.rs` short enough to read in one sitting.

The wire-level tests in `tests/flows.rs` drop below `Session` to `HttpAgent` and pin the
exact event sequence a run puts on the wire, attribution included:

```sh
cargo test -p task-board
```

## Next

- [board-watch](/ag-ui-rust/examples/board-watch/) — the same protocol from the other side,
  with a client written against no particular agent.
- [The Agent trait](/ag-ui-rust/server/agent/) — the reference for what this example does.
- [Tool calls](/ag-ui-rust/server/tools/) and [Shared state](/ag-ui-rust/server/state/) —
  the two subsystems that interact most here.
- [A2UI](/ag-ui-rust/a2ui/) — the surface, and what it takes to draw one.
