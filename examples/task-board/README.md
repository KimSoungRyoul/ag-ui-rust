# task-board

A workshop task board, spoken over AG-UI. Both halves of the protocol in one
crate: an agent you can host, and a terminal that talks to it.

It exists to be a first outside consumer of this SDK. It uses `ag-ui-server`,
`ag-ui-client`, `ag-ui-axum`, `ag-ui-a2ui` and `ag-ui-core` exactly as a crates.io
user would — public items only, nothing reached into — and every major subsystem
appears once:

| Subsystem | Where it shows up |
| --- | --- |
| Streamed text | the reply, one word per `TEXT_MESSAGE_CONTENT` |
| Reasoning | `ctx.think()` — one line about what the agent made of the message |
| Tool calls | `add_task`, `complete_task`, `estimate`, `clear_board`, run by the agent |
| Shared state | the board, published as `STATE_SNAPSHOT` then `STATE_DELTA`s |
| A2UI | the board as a surface, in an `a2ui_operations` tool-result envelope |
| Human in the loop | `clear` pauses the run and waits for a yes |
| Steps | the whole turn is bracketed by `STEP_STARTED` / `STEP_FINISHED` |

## Running it

Two terminals. First the agent:

```sh
cargo run -p task-board -- serve
```

```text
task board on AG-UI
  POST http://127.0.0.1:8080/agent    run endpoint (text/event-stream)
  GET  http://127.0.0.1:8080/health
```

Then the client:

```sh
cargo run -p task-board -- chat
```

Type `add draft the agenda, book the room`, then `list`, `complete 1`, `clear`.
`quit` or Ctrl-D ends it. `--port`, `--url` and `--thread` are the flags.

The transcripts below are piped rather than typed, which is also how the tests
run them:

```sh
printf 'add draft the agenda, book the room\nlist\n' | cargo run -p task-board -- chat
```

## What a turn looks like

```text
you> add draft the agenda, book the room
  ~ adding 2 task(s)
  · add_task({"title":"draft the agenda"})
    → {"id":1,"title":"draft the agenda"}
  [state] 1 open · 0 done
  · add_task({"title":"book the room"})
    → {"id":2,"title":"book the room"}
  [state] 2 open · 0 done
  agent> Added #1 draft the agenda, #2 book the room. 2 open · 0 done
  · render_a2ui({"surfaceId":"task-board"})
    ┌ a2ui surface
    │ Workshop board
    │ 2 open · 0 done
    │ [ ] #1 draft the agenda
    │ [ ] #2 book the room
    └
you> complete 1
  ~ looking for the task matching "1"
  · complete_task({"task":"1"})
    → {"id":1,"title":"draft the agenda","done":true}
  [state] 1 open · 1 done
  agent> Done: #1 draft the agenda. 1 open · 1 done
  · render_a2ui({"surfaceId":"task-board"})
    ┌ a2ui surface
    │ Workshop board
    │ 1 open · 1 done
    │ [x] #1 draft the agenda
    │ [ ] #2 book the room
    └
```

`~` is reasoning, `·` is a tool call, `→` is its result, `[state]` is the board
after a state event, and the box is the A2UI surface — drawn by walking the
component tree and resolving each binding through `ag_ui_a2ui::binding::Scope`,
which is as far as a terminal can honestly go towards rendering.

Two adds means two publishes, and nothing above can tell what either was on the
wire, because the client applied both into the same `Board`. The server decides
per publish: the first is always a `STATE_SNAPSHOT`, and later ones are a
`STATE_DELTA` *unless the patch would be no smaller than the state it
describes*. On a board this small it would be — resending two short tasks costs
less than the RFC 6902 patch adding one — so both of those go out as snapshots.
Give the tasks realistic titles and the second becomes a delta:

```text
STATE_SNAPSHOT {"tasks":[{"id":1,"title":"write the workshop agenda and circulate it",…}],"nextId":1}
STATE_DELTA    [{"op":"add","path":"/tasks/1","value":{"id":2,…}},{"op":"replace","path":"/nextId",…}]
```

Both encodings are pinned by a test, because "it works" here means the client
lands in the same place either way.

## The human in the loop

`clear` is the one destructive command, so the agent stops and asks. The answer
travels on a *second* request, in `ctx.resume()`:

```text
you> clear
  ~ clearing cannot be undone, so a human decides
  agent> Clearing drops 1 task(s) and cannot be undone.
  ?? Clear the board? 1 task(s) will be removed.
  [y/N] n
  ~ a human declined, so the board stays
  agent> Left the board alone. 1 open · 0 done
you> clear
  ~ clearing cannot be undone, so a human decides
  agent> Clearing drops 1 task(s) and cannot be undone.
  ?? Clear the board? 1 task(s) will be removed.
  [y/N] y
  ~ a human approved clearing the board
  · clear_board({})
    → {"removed":1}
  [state] nothing on the board
  agent> Cleared 1 task(s). The board is empty.
```

The declined run reaches the same code with `ResumeStatus::Cancelled`, so it
answers no tool call at all — which is the assertion the test makes.

## Where the board lives

**On the client.** The agent stores nothing between runs: it reads the board out
of `RunAgentInput.state`, publishes what it changed, and forgets. `Session` is
what carries it from one run to the next, along with the conversation.

Two consequences worth knowing before you build on this:

- A second `chat` process joining the same thread id starts from an empty board.
  The thread id names the conversation; it does not fetch one. Persisting a
  thread is the application's job, and this example does not do it.
- The agent knows whether a surface is already on screen only because the
  client sends the history back. `find_prior_surface` replays the A2UI
  operations already in the conversation, which is what makes the second render
  an `updateComponents` rather than a second `createSurface`.

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

The model rewrites the reply sentence and nothing else — ids, counts and state
transitions stay deterministic:

```text
you> add ship the rust sdk
  ~ adding 1 task(s)
  · add_task({"title":"ship the rust sdk"})
    → {"id":1,"title":"ship the rust sdk"}
  [state] 1 open · 0 done
  agent> Okay, I've added task #1 ship the rust sdk, which is currently open.
```

A model that fails does not fail the run: the scripted sentence goes out and the
failure is reported as reasoning. There is no LLM crate in the dependency
tree — `src/llm.rs` is `reqwest` and two `serde` structs, the same way
`e2e/src/llm.rs` is.

## The code

| File | What is in it |
| --- | --- |
| `src/board.rs` | `Board` and `Task`, the four tool schemas, the A2UI surface |
| `src/agent.rs` | the `impl Agent` and the command parser |
| `src/chat.rs` | the terminal client, generic over its input and output |
| `src/llm.rs` | the optional `--llm` phrasing |
| `src/main.rs` | the CLI |
| `tests/flows.rs` | all three flows, against a server on a real port |

`tests/flows.rs` drives `chat::converse` itself, with a scripted `&[u8]` for a
keyboard and a `Vec<u8>` for a screen, so the transcripts above are assertions
rather than illustrations. The last test drops to `HttpAgent` and pins the exact
event sequence a run puts on the wire.

```sh
cargo test -p task-board
```
