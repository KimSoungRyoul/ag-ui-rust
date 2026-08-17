# board-watch

A terminal client for any AG-UI agent — and the deliberately awkward agent it is
aimed at.

Round one (`../task-board`) was an agent with a client attached. This is the
other way round: the application is the **client**, and the server in this crate
exists only to give it streams worth surviving. It uses `ag-ui-client`,
`ag-ui-core` and `ag-ui-a2ui` the way an outside consumer does — public items
only — and needs no key and no network beyond loopback.

| Command | What it is |
| --- | --- |
| `watch` | The application: send a line, render the run, answer what it pauses on, draw the board. |
| `trace` | The same conversation one level down — events exactly as they arrived. |
| `replay` | The whole client with the network taken out, over a recorded fixture. |
| `serve-fake` | The awkward backend the transcripts below were recorded against. |

## Running it

Two terminals. The backend:

```sh
cargo run -p board-watch -- serve-fake
```

Then the client, pointed at it:

```sh
cargo run -p board-watch -- watch --url http://127.0.0.1:8090/agent --fragments
```

Scenarios are chosen by the first word you type, so a transcript names what it
exercised: `chunks`, `call`, `parallel`, `mixed`, `approve`, `slow`, `fail`, and
anything else for a well-behaved turn.

`--fragments` is the flag worth knowing: it brackets every delta as it arrives,
so what the client had to reassemble is visible in the output.

## Chunked streams

A provider adapter cannot bracket its output — the upstream API does not say a
message ended until the next one begins — so it sends `*_CHUNK` events that
carry their id **only on the first one**. Five events, one message:

```text
> chunks
  text   [Chunked text arrives in frag][ments, and the client rejoins ][them — emoji included: 👩][‍][💻.]
  done   success
```

The last three fragments split a ZWJ emoji between its parts. Every fragment is
valid UTF-8 on its own — a `String` cannot be otherwise — but the *grapheme*
only exists once they are rejoined.

Tool arguments are worse, because they are JSON split at arbitrary offsets:

```text
> call
  call   add_task [{"no][te":"line\][nbreak","ti][tle":"ship ][the SDK","depth":3}]
  result {"id":1,"title":"ship the SDK"}
```

The seam after `line\` is the case every adapter gets wrong once: the backslash
and the `n` it escapes arrive in different events, so anything that parses a
fragment on its own sees invalid JSON. What the client hands over is
`{"note":"line\nbreak","title":"ship the SDK","depth":3}`, which parses.

## Two calls at once

A model that asks for two things at once produces interleaved events —
`args(a) args(b) args(a) end(a) end(b)`. The obvious renderer, which prints a
prefix on `ToolCallStarted` and a newline on `ToolCallEnded`, produces one
garbled line; this one buffers by call id:

```text
> parallel
  call   add_task [{"title":]["write it down"}]
  call   add_task [{"title":]["read it back"}]
  result {"id":1,"title":"write it down"}
  result {"id":2,"title":"read it back"}
  state  2 open · 0 done
```

That buffering has a price, and it is worth knowing before you copy the
renderer. A call is printed when it *closes*, so anything the agent emitted
while the call was open prints **before** it. `task-board` publishes state from
inside its call, so `watch` shows:

```text
  state  1 open · 0 done
  call   add_task {"title":"draft the agenda"}
  result {"id":1,"title":"draft the agenda"}
```

…while the wire actually carried:

```text
  TOOL_CALL_START            {"toolCallName":"add_task","toolCallId":"wire-run-1-call-1"}
  TOOL_CALL_ARGS             {"delta":"{\"title\":\"one thing\"}","toolCallId":"wire-run-1-call-1"}
  STATE_SNAPSHOT             {"snapshot":{"tasks":[{"id":1,...}],"nextId":1}}
  TOOL_CALL_END              {"toolCallId":"wire-run-1-call-1"}
  TOOL_CALL_RESULT           {"messageId":"wire-run-1-msg-2",...}
```

An `Update::State` carries no association with the call it arrived during, so
the nesting the wire had is not recoverable from the update stream. Use `trace`
when the order is what you are debugging.

## Pausing

A run can pause on several decisions at once, and they are answered in **one**
request — answering one per request never terminates, because the agent only
sees what the resuming request carries.

```text
> approve
  pause  approve-budget · Approve the budget?
  pause  confirm-date · Confirm the date?
  done   interrupted on 2
  approve approve-budget [y/N] y
  answer approve-budget · approved
  approve confirm-date [y/N] n
  answer confirm-date · declined
  text   Declined: confirm-date. Nothing booked.
  done   success
```

`--approve` and `--decline` answer everything unattended, for scripts.

## Stopping

Polling the stream is what pulls bytes, so letting go of it is the whole of
client-side cancellation. `--stop-after N` drops the `RunStream` after N
updates:

```text
> slow
  text   working on it, this will take a while
  stop   dropped the stream after 3 updates
```

The agent is thirty seconds into a call it will never finish, and the drop
reaches it: the integration test asserts the run's cancellation token had been
tripped by the time the agent's future exited. The session stays usable — the
next run is a run like any other.

## Streams the protocol forbids

`ag-ui-server` will not emit a malformed stream, which is what it is for. So the
`/raw` endpoint frames the bytes by hand with `SseFormatter`, the way a producer
in another language does, and the client's own verifier is what has to catch it:

```text
$ board-watch watch --url http://127.0.0.1:8090/raw/unbracketed
> go
  error  protocol violation: TEXT_MESSAGE_CONTENT for message "ghost", which was never opened
  done   success
```

The offending event is *not* applied — the conversation holds one message, the
user's. Turning verification off applies it anyway:

```text
$ board-watch watch --url http://127.0.0.1:8090/raw/unbracketed --no-verify
> go
  text   text nobody opened
  done   success
```

The applier is tolerant either way; what `--no-verify` costs is the diagnosis,
not the conversation. Note also that the run still ends `success`: an
`Update::Error` is not necessarily fatal, so a client that reads only
`Update::Done` misses this entirely.

## Against a real agent

`task-board` from round one, unmodified, on port 8080:

```sh
cargo run -p task-board -- serve
cargo run -p board-watch -- watch --url http://127.0.0.1:8080/agent \
    --tools examples/board-watch/fixtures/task-board-tools.json
```

```text
> add draft the agenda, book the room
  think  adding 2 task(s)
  state  1 open · 0 done
  call   add_task {"title":"draft the agenda"}
  result {"id":1,"title":"draft the agenda"}
  state  2 open · 0 done
  call   add_task {"title":"book the room"}
  result {"id":2,"title":"book the room"}
  text   Added #1 draft the agenda, #2 book the room. 2 open · 0 done
  call   render_a2ui {"surfaceId":"task-board"}
  surface
    Workshop board
    2 open · 0 done
    [ ] #1 draft the agenda
    [ ] #2 book the room
  done   success
┌ board
│ 2 open · 0 done
│ [ ] #1 draft the agenda
│ [ ] #2 book the room
└ run board-run-1 · 8 messages · surface task-board (6)
```

Three things in that panel are worth naming. The board is this client's *own*
view model, declared in `src/board.rs` independently of the agent's — a
front-end team is handed a JSON shape, not a crate. The surface is drawn by
walking the A2UI component tree and resolving each binding through
`ag_ui_a2ui::binding::Scope`. And `surface task-board (6)` is recovered from the
**conversation** with `find_prior_surface_in`, not from the tool result that
happened to carry it.

### Why `--tools` exists

In AG-UI the *client* offers the tools and the agent picks from them. There is
no discovery: nothing lets an agent ask for a tool it was not sent, and an agent
handed none simply fails.

```text
$ board-watch watch --url http://127.0.0.1:8080/agent      # no --tools
> add anything
  think  adding 1 task(s)
  error  run failed: agent error: the client offered no add_task tool
  done   failed [AGENT_ERROR] agent error: the client offered no add_task tool
```

A client not written against one specific agent therefore has to be *configured*
with the tool set. The bundled fixture is exactly what `task-board` offers, and
a test asserts it has not drifted.

## One level down

`trace` prints the events unassembled — what a proxy, a recorder or a person
debugging a stream wants. It also does the human-in-the-loop round trip with no
`Session` at all: `interrupts_of` reads what the run paused on, `resume_run`
builds the request that answers it.

```text
$ board-watch trace --url http://127.0.0.1:8090/agent --approve approve
--- run 1 · low-run-1
  RUN_STARTED                {"runId":"low-run-1","threadId":"low"}
  RUN_FINISHED               {"outcome":{"type":"interrupt","interrupts":[{"id":"approve-budget",…}]},…}
--- run 2 · low-run-2
  RUN_STARTED                {"runId":"low-run-2","threadId":"low"}
  TEXT_MESSAGE_START         {"role":"assistant","messageId":"low-run-2-msg-1"}
  TEXT_MESSAGE_CONTENT       {"delta":"Both approved. Booked.","messageId":"low-run-2-msg-1"}
  TEXT_MESSAGE_END           {"messageId":"low-run-2-msg-1"}
  RUN_FINISHED               {"outcome":{"type":"success"},"threadId":"low","runId":"low-run-2"}
--- 7 events
```

## Offline

`Transport` is a trait so that a fixture on disk substitutes for a server and
nothing above it changes:

```sh
cargo run -p board-watch -- replay examples/board-watch/fixtures/chunked-run.json --fragments
```

```text
> anything
  call   add_task [{"no][te":"line\][nbreak","ti][tle":"ship ][the SDK","depth":3}]
  result {"id":1,"title":"ship the SDK"}
  done   success
```

Refresh the recording with `fixtures/capture.py` against a running `serve-fake`.

## Against a real model

Optional, and nothing in the default path touches it. `ag-ui-e2e`'s LLM agent is
an AG-UI endpoint like any other, so the client needs no changes:

```sh
export GEMINI_API_KEY=…                    # or AG_UI_LLM_API_KEY
cargo run -p ag-ui-e2e --example llm_agent
cargo run -p board-watch -- watch --url http://127.0.0.1:8080/agent --fragments
```

`tests/live.rs` does the same thing as a test. It is `#[ignore]`, so `cargo test`
and CI never touch the network, and it skips rather than fails when there is no
key:

```sh
cargo test -p board-watch --test live -- --ignored --nocapture
```

```text
asking gemini-2.5-flash-lite via https://generativelanguage.googleapis.com/v1beta/openai
> Reply with exactly this sentence and nothing else: the board is ready.
  text   [the][ board is ready.]
  done   success
```

Two deltas, fragmented by an actual provider on its own schedule rather than by
this crate's idea of one — which is the only part of chunk handling a fixture
cannot fake.

## The code

| File | What is in it |
| --- | --- |
| `src/watch.rs` | The driver and the renderer, generic over input and output |
| `src/view.rs` | The panel, the A2UI walk, and helpers that name a `Session` without bounding its transport |
| `src/board.rs` | The client's own view model of the agent's state |
| `src/trace.rs` | The unassembled view, and resume without a session |
| `src/fake.rs` | The awkward agent and the hand-framed illegal streams |
| `src/main.rs` | The CLI |
| `tests/client.rs` | Every flow above, against both backends on real sockets |
| `tests/live.rs` | The same client against a real model, `#[ignore]`d |

The tests drive `watch::watch` itself — the same function the binary runs — with
a scripted `&[u8]` for a keyboard and a `Vec<u8>` for a screen, so the
transcripts above are assertions rather than illustrations.

```sh
cargo test -p board-watch
```
