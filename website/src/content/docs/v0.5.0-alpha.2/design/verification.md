---
title: Verification
description: Three layers that keep an event stream well formed — typestate handles, a runtime ordering verifier on both ends, and a drift check against upstream in CI.
---

"The stream is well formed" is three different claims, and they need three
different mechanisms:

1. **The code cannot open two overlapping blocks.** A type-system guarantee,
   proved by a `compile_fail` doctest.
2. **What does go out obeys the ordering rules.** A runtime state machine, on
   the server *and* on the client, on by default even in release.
3. **The event set still matches the protocol.** An offline drift check against
   a vendored snapshot of upstream, run on every pull request.

Each catches something the others cannot. This page is all three.

## Layer 1: the borrow checker

The emitters in `ag_ui::server` are typestate handles. `ctx.assistant_message()`
returns a handle that borrows the run context mutably, so a second overlapping
block does not compile:

```rust,compile_fail,E0499
use ag_ui::server::RunContext;

fn interleave(ctx: &mut RunContext<()>) {
    let mut first = ctx.assistant_message().unwrap();
    // error[E0499]: cannot borrow `*ctx` as mutable more than once at a time
    let mut second = ctx.assistant_message().unwrap();
    first.delta("a").unwrap();
    second.delta("b").unwrap();
}
```

That block is `compile_fail`, so this page goes red if it ever starts compiling.
The same example lives in `crates/ag-ui/src/server/emit/mod.rs`, and it is the
only executable proof of the guarantee that
[Design commitments](/ag-ui-rust/v0.5.0-alpha.2/design/commitments/) sells as a headline
feature. Weaken the emitter API and that doctest goes green, which is a failure.

A handle also emits its terminating event on `Drop`, so the other half of the
guarantee — that what was opened gets closed — holds even when a `?` unwinds out
of the middle of a message.

### Why stable rustdoc is not enough

The annotation on that block names the error it expects: `compile_fail,E0499`.
**Stable rustdoc parses that error code and then ignores it.** The example need
only fail to compile, for any reason at all — a typo would do, and the guarantee
would quietly stop being tested while the test kept passing.

This is not a guess. It was verified by putting `E0308` — mismatched types,
nothing to do with the borrow-check example it labels — on the `emit/mod.rs`
doctest. Stable passed. Nightly failed with "Some expected error codes were not
found".

So CI runs the doctests on nightly as well, in a job that exists for no other
reason. It is the only use of nightly in the build.

## Layer 2: the runtime ordering verifier

Not everything the protocol forbids is expressible in a borrow. `ctx.emit` is
the documented escape hatch for chunk events and interleaved parallel tool
calls, and an agent using it can still emit `TEXT_MESSAGE_CONTENT` for a message
nothing opened. That is a bug which otherwise surfaces as a confused frontend,
three network hops from where it was caused.

### On the server

`ag_ui::server` runs an ordering state machine over every event on its way out,
before the transport sees it. An emit that breaks a rule returns `Err`, so the
agent's next `?` unwinds the run and the failure is reported as a `RUN_ERROR`
naming the rule:

```rust
use ag_ui::{Event, EventType, RunAgentInput};
use ag_ui::server::{Error, Rule, RunContext};

fn main() {
    let (mut ctx, _events) =
        RunContext::<()>::new(RunAgentInput::new("thread-1", "run-1")).unwrap();

    // Content for a message that was never opened.
    let error = ctx
        .emit(Event::text_message_content("msg-1", "Hello"))
        .expect_err("the verifier should reject this");

    let Error::Verification(violation) = error else {
        panic!("expected a verification error");
    };
    assert_eq!(violation.event, EventType::TextMessageContent);
    assert_eq!(violation.rule, Rule::NotOpen);
}
```

[`Rule`](https://docs.rs/ag-ui/0.5.0-alpha.2/ag_ui/server/error/enum.Rule.html) is the closed list of
what the machine checks:

| Rule | Rejected |
| --- | --- |
| `RunEnded` | anything after `RUN_FINISHED` / `RUN_ERROR` |
| `DuplicateRunStarted` | a second `RUN_STARTED` |
| `DuplicateStart` | opening a message, reasoning block, tool call or step whose id is already open |
| `NotOpen` | content or a terminator for something that was never opened |
| `UnknownId` | `TOOL_CALL_RESULT` for a call id that was never introduced |
| `OutOfOrder` | `TOOL_CALL_RESULT` before that call's `TOOL_CALL_END` |
| `OpenAtFinish` | `RUN_FINISHED` while a message, reasoning block, tool call, step or subagent is open |
| `OwnerMismatch` | a tagged continuation, terminator or re-open whose `subagentRunId` is not the one that opened the entity; a tool call tagged with one subagent whose parent message belongs to another |

`RUN_ERROR` is exempt from `OpenAtFinish`: a run that blew up mid-message could
not have closed it.

`OwnerMismatch` is the one rule subagents added. Every entity is opened *by
someone* — a subagent, or the parent agent when the opener carries no
`subagentRunId` — and the machine remembers who. A later event that *names* a
different owner is rejected; one that names none is accepted, because
attribution is optional on every event and a bare continuation is what a
pre-subagent producer sends. Steps are keyed by owner as well as name, so a
subagent cannot close the parent's step, and two agents may run a step of the
same name at once.

Each rejection is a
[`VerificationError`](https://docs.rs/ag-ui/0.5.0-alpha.2/ag_ui/server/error/struct.VerificationError.html)
carrying the offending event type, the rule, and a detail string. Emit content
for `msg-2` while `msg-1` is the message actually open, and its `Display` reads:

```text
TEXT_MESSAGE_CONTENT breaks rule `not-open` (content and terminators require a
matching start): message MessageId("msg-2") is not open [open: messages=["msg-1"]]
```

The bracketed dump of everything still open is **debug-only**. It is the
expensive part, and it is usually enough on its own to spot the missing
terminator.

What the machine deliberately lets through is as much of the design as what it
rejects. The `*_CHUNK` events are self-contained, so a chunk carrying a new id
registers that id rather than being rejected for having no start. The deprecated
`THINKING_*` family is not tracked at all. State, activity, raw and custom events
are unordered. And two *different* ids may overlap freely: a tool call opening
inside the message that narrates it is what every provider doing parallel calls
actually sends. The rule is that one id may not overlap itself.

#### What it costs, and how to switch it off

A handful of `HashSet`s and one lookup per event. It is on by default in release
builds too, because that price is not worth thinking about next to a protocol
bug that reaches a user.

If you have measured it and want the lookups back, turn off the `verify` feature.
The whole state machine is then replaced by a zero-sized type whose `observe` is
an inlined `Ok(())`. `verify` is in the crate's default set rather than implied
by `server`, which is what makes it droppable — a feature cannot be subtracted
from the set another feature pulls in:

```toml
[dependencies]
ag-ui = { version = "=0.5.0-alpha.2", default-features = false, features = ["server", "sse"] }
```

One thing survives that switch. Whether a terminal event has already gone out is
tracked in the event sink as well as in the verifier, so compiling verification
out cannot make the run driver emit a second `RUN_FINISHED`.

### On the client

`ag_ui::client` verifies too, and for a different reason: the events arrive from
someone else's process, and a stream that breaks the rules should produce one
clear error rather than a confused UI. This is where the TypeScript SDK puts its
verifier, and for a consumer that is the right instinct.

The rules are the same shape — subagent ownership included, spelled out as
rules 9 to 13 of `ag_ui::client::verify` — with three additions that only make
sense on the receiving end:

- `RUN_STARTED` opens the stream and does so exactly once. Only `RAW` and
  `CUSTOM` may precede it — they are outside the protocol's vocabulary by
  definition, so they are outside its ordering too.
- An `interrupt` outcome must carry at least one interrupt. It is the one rule
  the type system cannot express.
- The stream must *reach* a terminal event. A transport that stops early
  otherwise looks exactly like a short answer.

That last one is what `Verifier::finish` is for, and `verify_all` is the
convenience that runs a whole recorded stream and then calls it:

```rust
use ag_ui::client::verify_all;
use ag_ui::{Event, TextMessageRole};

fn main() {
    // A response the transport cut short: no RUN_FINISHED, no RUN_ERROR.
    let truncated = [
        Event::run_started("thread-1", "run-1"),
        Event::text_message_start("msg-1", TextMessageRole::Assistant),
        Event::text_message_content("msg-1", "Hel"),
    ];

    let error = verify_all(&truncated).expect_err("the stream was truncated");
    assert_eq!(
        error.to_string(),
        "protocol violation: the stream ended before RUN_FINISHED or RUN_ERROR",
    );
}
```

A `Thread` runs the streaming form by default. There is no cargo feature here —
it is a runtime switch, `ThreadBuilder::verify(false)`, for producers whose
quirks you have decided to live with. What you lose by turning it off is the
diagnosis, not the conversation: the applier stays tolerant either way.

### Why both ends

They answer different questions. The server's verifier says *you* emitted this
wrongly, at the moment you did it, with the agent's stack still on hand. The
client's says *someone else* sent this, and names what arrived rather than
letting a half-applied stream turn into a UI bug. Neither is redundant, because
the two ends of a run are usually not the same program — and quite often not the
same SDK.

## Layer 3: drift against upstream

The Rust event types are hand-written against the frozen AG-UI 1.0 JSON Schema.
Nothing in the compiler links them to that schema, so the SDK could otherwise
keep building after the protocol adds an event. An earlier community SDK had
24 variants against a 32-event spec. AG-UI 1.0 now defines **31** event types;
Rust also retains five `THINKING_*` variants for reading older streams.

The offline drift checks are:

```sh
cargo run --locked -p xtask -- drift-check
cargo run --locked -p xtask -- drift-check --local
```

The first command currently reports:

```text
drift-check
  baseline  xtask/baseline/events.json  (ag-ui-protocol/ag-ui@fdbca490dc, captured 2026-09-23)
  upstream  31 event types
  rust      crates/ag-ui/src/event  (10 files, 36 event types, tagged enum `Event`)

OK  31 event types match the baseline.
```

The baseline is a reviewed snapshot of upstream `spec/1.0/schema.json`. It
records the current event names, fields and requiredness, plus signatures for
the schema root and definitions. The scanner reads `crates/ag-ui/src/event/` as
text, so it can detect Rust drift even when that module does not compile. The
five legacy `THINKING_*` variants are excluded from the normative comparison.

`--local` also compares the schema vendored at
`crates/ag-ui/src/protocol/schema-1.0.json` with the reviewed baseline. A
missing schema, changed shape or unreadable field fails that check. Both
commands are offline and run in pull-request CI; exit 1 means drift and exit 2
means the check could not run.

### Is the baseline itself current?

The offline checks cannot tell whether the upstream repository changed after
the snapshot was reviewed. That needs a network request:

```sh
cargo run --locked -p xtask -- drift-check --upstream
```

This is a scheduled, non-required job. Upstream movement reports drift; a fetch
failure reports that the check could not run and fails that job.

When upstream changes, a human reviews the new schema and refreshes the
baseline:

```sh
cargo run --locked -p xtask -- drift-check --refresh
```

The refreshed `events.json` records the upstream commit and fetch date. Review
its diff alongside the vendored schema and update `crates/ag-ui/src/event/` in
the same pull request. The baseline is generated, never hand-edited to make a
failing implementation pass.

## What each layer cannot do

- The borrow checker cannot see events emitted through `ctx.emit`, which is why
  layer 2 exists.
- The runtime verifier cannot know that the schema added another event, which
  is why layer 3 exists.
- The drift check cannot detect a change in interpretation when the schema
  itself stays the same. Review upstream changes beyond the schema as well.

Everything above runs in CI on every pull request except the upstream freshness
job. [Testing](/ag-ui-rust/v0.5.0-alpha.2/design/testing/) is the full list and how to run it
locally.
