---
title: Design commitments
description: What this SDK holds itself to, the reasoning behind each commitment, and what each one costs.
---

This page is the case for the SDK, from the outside. If you are deciding whether
to build on it, this is what it promises and what those promises cost you.

The case is not that there is nothing else. As of August 2026 crates.io carries
several independent Rust takes on AG-UI, and more than one of them can host an
agent. An earlier draft of `docs/DESIGN.md` argued from scarcity; that argument
was true when it was written, is not true now, and has been retracted. What is
left is a quality claim, which is the kind that has to keep being earned.

## The four commitments

**Emitters you cannot misuse.** Every streaming construct in AG-UI is bracketed:
`TEXT_MESSAGE_START` … `TEXT_MESSAGE_END`, `TOOL_CALL_START` … `TOOL_CALL_END`,
`STEP_STARTED` … `STEP_FINISHED`. Handing an agent three raw emit calls per
construct means trusting it to close what it opened, in order, on every path
including the early return. This SDK hands out RAII handles instead. Creating
one emits the opening event; the handle borrows the run context mutably, so a
second overlapping handle is a **borrow-check error**; and `Drop` emits the
terminator, so forgetting `end()` — or returning `Err` through a `?` halfway
through a message — still produces a well-formed stream.

**Ordering verified on the server.** Not everything the protocol forbids is
expressible in the type system, so what the borrow checker cannot catch, a
runtime state machine does. `TEXT_MESSAGE_CONTENT` without a preceding `START`
is reported where it was caused rather than three network hops downstream, as a
confused frontend. Neither the TypeScript SDK (which verifies on the client) nor
the .NET one (which does not verify at all) checks ordering server-side.

**An exhaustive `Event`.** A protocol addition is a compile error for consumers
instead of something a `_` arm swallows.

**A drift check in CI.** The port is hand-written against upstream's TypeScript
schemas, so nothing in the compiler links the two.
`cargo run -p xtask -- drift-check` is that link, and it fails the build when
upstream's event set moves, so the port cannot quietly fall behind.

[Verification](/ag-ui-rust/design/verification/) is the whole of the second and
fourth in detail; [Testing](/ag-ui-rust/design/testing/) is how all four are
kept honest. The rest of this page is the reasoning behind the decisions those
commitments rest on.

## The source of truth is the TypeScript Zod schemas

Not the protobuf definitions. The `Event` message in upstream's `events.proto`
is a `oneof` over 18 of the protocol's 33 event types — no reasoning, no
activity, no thinking, no `tool_call_result`. The binary transport is a lossy
subset of the protocol, so it cannot serve as the port target. There is also no
JSON Schema export upstream to generate from.

So the port is hand-written against `core/src/events.ts`, and the drift check is
what keeps it honest. Detection, not generation: it parses the upstream
`EventType` enum and Zod object keys and fails the build when they diverge from
the Rust side. Full code generation would mean writing and maintaining a
Zod-to-Rust compiler, which is not worth it yet.

The [Event reference](/ag-ui-rust/reference/events/) names the 18 the binary
transport does carry and the 15 it does not.

## `Event` is exhaustive on purpose; the errors are not

Every error enum in this workspace is `#[non_exhaustive]`.
[`Event`](/ag-ui-rust/api/ag_ui_core/event/enum.Event.html) and
[`EventType`](/ag-ui-rust/api/ag_ui_core/event/enum.EventType.html) are not, and
the asymmetry is deliberate. The protocol *has* grown twice in the last year —
`REASONING_*`, `ACTIVITY_*` — so this gets tested rather than remaining
hypothetical.

The failure the SDK exists to correct is silent under-coverage. `#[non_exhaustive]`
institutionalises exactly that: it obliges every consumer to write a `_` arm, and
a `_` arm is precisely the construct that turns "event 34 arrived" into no
diagnostic at all. It does not remove the work of handling a new event; it
removes the notification that there is work.

So a new protocol event *should* be a compile error for a Rust consumer. That is
the whole value proposition of a typed SDK over `serde_json::Value`, and the
drift checker completes the story: it fails this repo's build when upstream adds
an event, this crate adds the variant, and every downstream match then fails to
compile until someone decides what the new event means to them. Three links,
each loud.

**The price is honest and accepted: adding an event is a major version of this
SDK.** It should be — the wire contract changed. If you match on `Event`
directly, budget for that. If you would rather not, match on the higher-level
[`Update`](/ag-ui-rust/api/ag_ui_client/session/enum.Update.html) stream instead,
which does carry the attribute.

The reasoning inverts for errors, which is why they carry it. Nobody wants an
exhaustive match over failure modes, callers route on a handful of variants and
fall through on the rest, and a new failure mode is not a protocol change.

### Where `RunEnd` and `Update` fall

The two client types sit on opposite sides of that line, and the split shows
what the rule actually is.

[`RunEnd`](/ag-ui-rust/api/ag_ui_client/session/enum.RunEnd.html) sits with
`Event`: exhaustive. A run ends in exactly the three ways the protocol defines,
that match is the one a front-end most wants checked — it decides whether the
input goes live again — and a fourth way to end a run *would* be a wire-contract
change.

```rust
use ag_ui_client::RunEnd;

fn on_end(end: &RunEnd) -> String {
    // No `_` arm. A fourth way to end a run would stop this compiling, which is
    // the point: the protocol changed, and this function has a decision to make.
    match end {
        RunEnd::Success { .. } => "done".to_owned(),
        RunEnd::Interrupted { interrupts } => {
            format!("waiting on {} interrupt(s)", interrupts.len())
        }
        RunEnd::Failed { message, .. } => format!("failed: {message}"),
    }
}

fn main() {
    let end = RunEnd::Failed {
        message: "the weather service is down".to_owned(),
        code: Some("AGENT_ERROR".to_owned()),
    };
    assert_eq!(on_end(&end), "failed: the weather service is down");
}
```

`Update` keeps `#[non_exhaustive]`. It is a view model rather than a wire type,
and a new kind of thing worth redrawing is not a protocol change.

The runtime side agrees with the type side. An event type this build does not
know fails to deserialize, the session reports it and ends the run as
`RunEnd::Failed`. A frontend talking to a newer agent stops with an error naming
the unknown type rather than quietly rendering three quarters of a conversation.

## No LLM abstraction

`AGUI.Server` in .NET is built on `Microsoft.Extensions.AI`'s `IChatClient`.
That works because .NET has one blessed chat abstraction. Rust does not: recent
90-day downloads run roughly `async-openai` 2.3M, `rig-core` 1.3M, `genai` 113k,
while `agent-framework-core` — the closest analogue — sits near 1k. Picking one
would be picking a side, and picking wrong would be a dependency every user of
this SDK carries.

So **`trait Agent` *is* the boundary**, and this SDK depends on no LLM crate at
all. Bring your own client; implement one trait. A framework integration is then
an `impl Agent for …` in its own crate, and it is nobody's problem but its own.

That claim is not left as an assertion. The workspace's live smoke test reaches
a real streaming model through plain `reqwest` and implements nothing but
`Agent`, so the absence of an LLM dependency in `e2e/Cargo.toml` *is* the
evidence — see [Testing](/ag-ui-rust/design/testing/).

## Executor-agnostic below the web binding

`ag-ui-core`, `ag-ui-server` and `ag-ui-client` use `futures` primitives —
notably `futures::channel::mpsc` for the emit path rather than
`tokio::sync::mpsc`. tokio appears only in `ag-ui-axum`. This keeps wasm targets
and non-tokio executors viable.

CI enforces it two ways, and the second exists because the first is not enough:
those crates are built for `wasm32-unknown-unknown`, *and* tokio is asserted
absent from their dependency graphs. tokio's `rt`, `sync`, `macros`, `io-util`
and `time` features all compile for wasm, so adding `tokio` to `ag-ui-server`
passes every wasm check. That was verified by doing exactly that and watching the
wasm build stay green. The dependency graph is what carries the guarantee, so
that is what gets asserted.

## Synchronous emit, because Rust has no async `Drop`

This is the cost of the first commitment, and it is worth naming plainly:
`msg.delta(text)?` does not take `.await`.

`Drop` cannot be async, so a handle cannot `await` while emitting its
terminator. The emit path is therefore synchronous end to end — handles push
into an unbounded channel and the transport layer drains it. The first draft of
this API had `msg.delta(t).await?`, copied from the TypeScript and .NET SDKs,
and it simply cannot coexist with the RAII guarantee. One of the two had to go.

What the borrow forbids is a second open block — the protocol's rule — and
nothing else. A handle therefore holds two *fields* of the run context, the
event sink and the state, rather than the context itself, so `call.state_mut()`
and `call.publish_state()` work while a call is open, which is where a tool's
own work belongs. The verifier agrees, because `STATE_*` is unordered on the
wire.

An earlier draft held only the sink, which left the state unreachable for as
long as anything was open and forced every agent to mutate *before* announcing
the call it was mutating for. Same events, different order — and the order is
what decides whether a client can watch a call land or only see it already done.
Holding the state beside the sink widens what a handle can reach without
widening what it can open: there is still no run context behind it to open a
second block with.

## IDs are strings

The spec types `threadId`, `runId` and `messageId` as strings. An existing
community crate parses them as UUIDs, which breaks immediately against LangGraph
— it emits thread ids like `"thread-abc"` and run ids that are plain integers —
and against anything else using its own id scheme (upstream issues #2195 and
#2196). Newtypes over `String` preserve the type distinction without inventing a
constraint the protocol does not have:

```rust
use ag_ui_core::{RunId, ThreadId};

fn main() {
    // Whatever the producer sends round-trips byte for byte.
    let thread = ThreadId::new("thread-abc");
    let run = RunId::new("42");

    assert_eq!(thread.as_str(), "thread-abc");
    assert_eq!(run.as_str(), "42");

    // Distinct types, so one cannot be passed where the other is meant.
    assert_eq!(serde_json::to_string(&thread).unwrap(), r#""thread-abc""#);
}
```

Generate a UUID and pass its string form if that is what you want. The SDK takes
no `uuid` dependency and has no opinion.

## Five crates, not seven

The first draft mirrored the .NET assembly split one-for-one, which is the wrong
instinct: in .NET an assembly is the deployment and versioning unit, so
splitting is cheap and natural. In Rust, **features are the primary tool** and a
crate split should be justified by dependency isolation or independent
versioning.

Two crates were folded in as a result. The SSE encoder became
`ag-ui-core::encode` — a few hundred lines with zero extra dependencies, where
only protobuf is heavy and an optional dependency already handles that. The A2UI
toolkit became a feature of `ag-ui-a2ui`, because it is prompt strings and
orchestration with nothing to isolate.

What stayed separate, and why: `ag-ui-axum` drags in axum, tokio and tower;
`ag-ui-client` is useful on its own; `ag-ui-a2ui` is a different protocol that
can be used without AG-UI at all. [The crates](/ag-ui-rust/start/crates/) is the
tour.

## One extension point, not two

An early draft carried both a `StreamOptions` builder of `map_content` /
`map_call` / `map_result` / `map_interrupt` closures — a direct port of .NET's
`AGUIStreamOptions` — *and* a middleware chain. Two ways to do the same thing,
and the closure version degrades into a pile of `Box<dyn Fn>` in Rust.

Everything composes through `StreamTransformer` instead, and the former hooks
are provided as built-in transformers. Transformers run in the order they were
added, each seeing what the previous one produced, before the ordering verifier
sees anything — which is what makes dropping events safe, since the verifier
never sees the half of a tool call that was removed.

## The offered tool list is a capability list, not an allow-list

`RunAgentInput.tools` says what the *client* can execute. It does not say what
the agent may call, and nothing here treats it as an allow-list: emitting
`TOOL_CALL_START` for a name absent from that list is a well-formed stream, and
the ordering verifier says nothing about it.

The case that settles it is a tool the agent answers itself. An A2UI agent emits
`render_a2ui` to carry a surface to the frontend — the frontend draws it, and no
client ever "offered" it, because there is nothing for a client to execute. The
same shape covers a server-side tool whose result the agent computes and reports
within the run, and a call emitted purely so the transcript shows what the agent
did.

What a client does with a call it does not recognise is the client's decision:
ignore it, render it as an activity, or report it. What the protocol constrains
is the *ordering* — `TOOL_CALL_ARGS` with no `START`, a result before the end —
and that is what gets checked.

An agent that wants the stricter rule can have it in one line, because
`RunContext::tool` returns `None` for anything unoffered. The `task-board`
example does exactly that, but only for the tools it genuinely expects the
client to run.

## A2UI pins to v0.9

The A2UI spec is at v1.0, but every shipping toolkit — TypeScript, .NET, Python
— still stamps `v0.9`, and .NET's constants file marks these values a
"cross-language wire contract" that "must not diverge". Implementing v1.0 wire
values today would mean not interoperating with any of them. v1.0 goes behind a
feature when the toolkits move. See [A2UI](/ag-ui-rust/a2ui/).
