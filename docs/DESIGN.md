# Design decisions

Why this SDK looks the way it does. Written for someone about to change something here.

## Why another Rust AG-UI SDK

`sdks/community/rust` in the upstream repo landed in October 2025 and then stalled: there is
no CODEOWNERS entry for it, so nobody has merge authority. As of August 2026 four Rust PRs sit
with zero reviews, including a server crate (#972, +5,916 lines) open since January. The core
team acknowledged the situation on issue #2256.

The concrete result: `ag-ui-core 0.1.0` declares 24 event variants against a spec with 33 at the time of writing — 36 today —
missing nine, the whole `REASONING_*` family and both `ACTIVITY_*` events — and has no
`RunFinished.outcome` field, which means human-in-the-loop is not expressible at all. An
unknown `type` does not degrade to "ignore it": the enum is `#[serde(tag = "type")]` with no
fallback, so the event fails to deserialize and the run stops there.

That vacuum has since drawn other answers. As of August 2026 crates.io carries several
independent Rust takes on AG-UI, and more than one of them can host an agent. An earlier draft
of this document said there was no way to host an AG-UI agent in Rust; that was true when it
was written and is not true now, and nothing here should be argued from scarcity again.

So the case for this SDK is not that it is the only option. It is what it holds itself to,
each of which is a decision the rest of this document explains and a test enforces:

- **Emitters you cannot misuse.** Two overlapping messages are a borrow-check error, not a
  runtime protocol violation, and a handle emits its terminator on `Drop` so it cannot be
  forgotten.
- **Ordering verified on the server**, on by default, so `TEXT_MESSAGE_CONTENT` without a
  `START` is reported where it was caused rather than three network hops downstream.
- **An exhaustive `Event`.** A protocol addition is a compile error for consumers instead of
  something a `_` arm swallows.
- **A drift check in CI.** `xtask drift-check` fails the build when upstream's event set moves,
  so the port cannot quietly fall behind the way the community crate did.

The goal is for this to become the official Rust SDK. That is the AG-UI organisation's
decision to make, not this project's, so the only thing worth doing about it is the list
above — and keeping every item on it enforced by something that fails the build.

## The source of truth is the TypeScript Zod schemas

Not the protobuf definitions. `sdks/typescript/packages/proto/src/proto/events.proto` has an
`Event` oneof with 21 arms — no reasoning, no activity, no thinking, no `tool_call_result`.
The binary transport is a lossy subset of the protocol, so it cannot serve as the port target.
There is also no JSON Schema export upstream (`zod-to-json-schema` and `toJSONSchema` appear
nowhere in the repo).

So the port is hand-written against `core/src/events.ts`, and `xtask drift-check` in CI is what
keeps it honest. Detection, not generation: it parses the upstream `EventType` enum and Zod
object keys and fails the build when they diverge from the Rust side. Full code generation would
mean writing and maintaining a Zod-to-Rust compiler, which is not worth it yet.

The check reads the fields of upstream's `BaseEvent` as well as each event's own. It did not
until 0.3.0: it walked per-event payloads only, so when upstream added `metadata` to the base
schema every event gained a field and the check stayed green. The gap was found by reading
the upstream diff by hand, which is precisely the reading the check exists to make
unnecessary, so it was closed rather than noted — `timestamp`, `rawEvent` and `metadata`
are now compared like any other field, and `--refresh` records them in the baseline.

## `Event` is exhaustive on purpose; the errors are not

Every error enum in the workspace is `#[non_exhaustive]`. `Event` and `EventType` are not,
and that asymmetry is deliberate rather than an oversight — the protocol *has* grown twice
in the last year (`REASONING_*`, `ACTIVITY_*`), so this will be tested.

The failure this SDK exists to correct is silent under-coverage. `ag-ui-core 0.1.0` declares
24 variants against the 32 the spec had then — 36 today — and nobody noticed, because nothing
anywhere forced the question. `#[non_exhaustive]` institutionalises that: it obliges every
consumer to write a `_` arm, and a `_` arm is precisely the construct that turns "event 37
arrived" into no diagnostic at all. It does not remove the work of handling a new event; it
removes the notification that there is work.

So a new protocol event *should* be a compile error for a Rust consumer. That is the whole
value proposition of a typed SDK over `serde_json::Value`, and it is the story the drift
checker completes: `xtask drift-check` fails this repo's build when upstream adds an event,
this crate adds the variant, and every downstream match then fails to compile until someone
decides what the new event means to them. Three links, each loud.

The price is honest and accepted: adding an event is a major version of this SDK. It should
be — the wire contract changed.

The same reasoning inverts for errors, which is why they carry the attribute. Nobody wants
an exhaustive match over failure modes, callers route on a handful of variants and fall
through on the rest, and a new failure mode is not a protocol change.

`ag_ui::client::RunEnd` sits with `Event` rather than with the errors, for the same reason
scaled down: a run ends in exactly the three ways the protocol defines, that match is the one
a front-end most wants checked — it decides whether the input goes live again — and a fourth
way to end a run *would* be a wire-contract change. `Update` keeps the attribute: it is a view
model rather than a wire type, and a new kind of thing worth redrawing is not a protocol change.

The runtime side agrees with the type side. An event type this build does not know fails to
deserialize, `Session` reports it and ends the run as `RunEnd::Failed`. A frontend talking
to a newer agent stops with an error naming the unknown type rather than quietly rendering
three quarters of a conversation.

## Crate boundaries

Two crates. The first draft mirrored the .NET assembly split one-for-one, which is the wrong
instinct: in .NET an assembly is the deployment and versioning unit, so splitting is cheap and
natural. In Rust, **features are the primary tool** and a crate split should be justified by
dependency isolation or independent versioning.

That rule folded seven crates into five, and then — applied to its own conclusion — five into
two. The five-crate arrangement failed its own test. Feature gates give exactly the dependency
isolation a split does: `--no-default-features` compiles no axum, no tokio, no reqwest, and CI
asserts it per feature rather than per crate. So isolation was never the thing the split was
buying. Independent versioning would have been, and this workspace does not do it: one
`workspace.version`, every crate released in lockstep. A split that delivers neither of the two
things that justify a split is five publishing steps and four extra names to defend, in a
registry where three of those names were taken while the question was open.

What is left is one crate per *protocol*:

- **`ag-ui`** — the protocol types at the root, always compiled; `server`, `client` and `axum`
  behind features. Each runtime keeps its own `Error` under its own module, so `ag_ui::Error`
  is a protocol error and `ag_ui::server::Error` is a hosting error.
- **`ag-ui-a2ui`** — A2UI is a different protocol, usable over A2A or MCP with no AG-UI
  anywhere. Its users should not have to depend on a crate named `ag-ui` to reach it, and that
  is a dependency-isolation argument the feature gate cannot make.

The price is the one thing a split does buy and features cannot: cargo unifies features across
a dependency graph, so if one crate in a build wants `server` and another wants `client`, both
compile. That is a compile-time cost for a mixed graph, not a runtime or correctness one, and
it is the trade this arrangement accepts.

Two crates were folded in earlier under the same rule, and stay folded:

- `ag-ui-encoder` → `ag_ui::encode`. SSE framing is a few hundred lines with zero extra
  dependencies. Only protobuf is heavy, and an optional dependency already handles that.
- `ag-ui-a2ui-toolkit` → `ag-ui-a2ui`, `toolkit` feature. It is prompt strings and
  orchestration; nothing to isolate.

## No LLM abstraction

`AGUI.Server` in .NET is built on `Microsoft.Extensions.AI`'s `IChatClient`. That works because
.NET has one blessed chat abstraction. Rust does not: recent 90-day downloads run roughly
`async-openai` 2.3M, `rig-core` 1.3M, `genai` 113k, while `agent-framework-core` — the closest
MEAI analogue — sits near 1k.

So `trait Agent` *is* the boundary and this SDK depends on no LLM crate. A framework
integration is then just an `impl Agent for …` in its own crate.

## Executor-agnostic below the web binding

`core`, `server`, and `client` use `futures` primitives — notably `futures::channel::mpsc` for
the emit path rather than `tokio::sync::mpsc`. tokio appears only in `ag_ui::axum`. This keeps
wasm targets and non-tokio executors viable, and the CI wasm job enforces it.

## Synchronous emit, because Rust has no async Drop

The emitter API is typestate: `ctx.assistant_message()` returns a handle that borrows the run
context mutably, so starting a second overlapping message is a borrow-check error rather than a
runtime protocol violation. The handle emits its terminating event on `Drop`, so forgetting
`end()` is harmless.

What the borrow forbids is a second open block — the protocol's rule — and nothing else. A
handle therefore holds two *fields* of the context, the event sink and the state, rather than
the context itself: `call.state_mut()` and `call.publish_state()` work while the call is open,
which is where a tool's own work belongs. The verifier agrees, because `STATE_*` is unordered on
the wire.

The first draft held only the sink, which left the state unreachable for as long as anything was
open and forced every agent to mutate *before* announcing the call it was mutating for. Same
events, different order — and the order is what decides whether a client can watch a call land or
only see it already done. Holding the state beside the sink widens what a handle can reach
without widening what it can open: there is still no run context behind it to open a second block
with.

That last guarantee is what forces the design. `Drop` cannot be async, so a handle cannot
`await` while emitting its terminator. The emit path is therefore synchronous end to end —
handles push into an unbounded channel and the transport layer drains it. The first draft of
this API had `msg.delta(t).await?`, copied from the TypeScript and .NET SDKs, and it simply
cannot coexist with the RAII guarantee.

## One extension point, not two

An early draft carried both a `StreamOptions` builder with `map_content` / `map_call` /
`map_result` / `map_interrupt` closures (a direct port of .NET's `AGUIStreamOptions`) *and* a
middleware chain. Two ways to do the same thing, and the closure version degrades into a pile of
`Box<dyn Fn>` in Rust. Everything composes through `StreamTransformer`; the former hooks are
provided as built-in transformers.

## IDs are strings

The spec types `threadId`/`runId`/`messageId` as strings. The existing community crate parses
them as UUIDs, which breaks immediately against LangGraph and anything else using its own id
scheme (upstream #2195, #2196). Newtypes over `String` preserve type distinction without
inventing a constraint the protocol does not have.

## Server-side protocol verification

Neither the TypeScript SDK (which verifies on the client) nor .NET (which does not verify at
all) checks event ordering on the server. Emitting `TEXT_MESSAGE_CONTENT` without a preceding
`START` is a bug that currently surfaces as a confused frontend. `ag_ui::server` runs an ordering
state machine, on by default, so it surfaces where it was caused.

## Subagent attribution is a sink scope

The protocol attributes events to subagents with an optional `subagentRunId` on 24 of the 36
types. The obvious port is a tag on every handle: a `MessageHandle` that knows which subagent
it belongs to and writes the field on each event it emits. That doubles every emitter — a
subagent-aware and a plain variant of each — and still misses `ctx.emit`, which is the path
every hand-built event takes.

So the attribution lives one layer down, in the event sink. `ctx.subagent(name)` announces
the subagent and sets a scope on the sink; while it is open, every attributable event that
arrives untagged is tagged, whichever emitter produced it, and an event the agent tagged
explicitly is left alone. The handle that represents the scope dereferences to the run
context, the way a step guard does, so the same messages, tool calls and nested subagents
open through it with no new API, and `Drop` emits `SUBAGENT_FINISHED` on the early return
a `?` produces. Two scopes cannot be open at once — a borrow-check error, as everything
overlapping is here — and the genuinely concurrent case is emitted by hand with explicit
tags, exactly as interleaved parallel tool calls already are.

The verifier learned the same fact from the other side: every entity is opened by someone,
and a later event that names a different owner is `Rule::OwnerMismatch`, the eighth rule.
One that names nobody is accepted, because attribution is optional per event and a bare
continuation is what a pre-subagent producer sends — and it does not hand the entity to the
parent either: the first writer stays the owner, which is what upstream records and what
keeps the verifier and the applier telling the same story about who owns a message. The
same owner tracking covers activities, the entity a `REASONING_ENCRYPTED_VALUE` names, the
history the `RUN_STARTED` echo replays, and the tool message a `TOOL_CALL_RESULT` mints.

## Visibility defaults to attributed

Upstream's integrations default to the *inline* shape — no lifecycle events, no
`subagentRunId` anywhere — and make the full surface opt-in, because a client older than
subagent support fails while decoding the three new event types. That is a real constraint,
and `SubagentVisibility::inline()` and `hidden()` exist for it.

This crate defaults the other way. A transformer that rewrites the stream is opt-in here like
every other transformer: an agent that wrote `ctx.subagent(..)` meant it, and silently
flattening what it said would make the emitted stream and the wire disagree by default,
which is the kind of surprise the rest of this document argues against. The producer is the
one who knows how old its consumers are, so the producer flips it, per endpoint.

## The offered tool list is a capability list, not an allow-list

`RunAgentInput.tools` says what the *client* can execute. It does not say what the agent may
call, and nothing here treats it as an allow-list: emitting `TOOL_CALL_START` for a name absent
from that list is a well-formed stream, and the ordering verifier says nothing about it.

The case that settles it is a tool the agent answers itself. An A2UI agent emits `render_a2ui`
to carry a surface to the frontend — the frontend draws it, and no client ever "offered" it
because there is nothing for a client to execute. The same shape covers a server-side tool
whose result the agent computes and reports within the run, and a call emitted purely so the
transcript shows what the agent did.

What a client does with a call it does not recognise is the client's decision: ignore it,
render it as an activity, or report it. What the protocol constrains is the *ordering* —
`TOOL_CALL_ARGS` with no `START`, a result before the end — and that is what the verifier
checks.

An agent that wants the stricter rule can have it in one line, because
[`RunContext::tool`](../crates/ag-ui/src/server/context.rs) returns `None` for anything
unoffered. `examples/task-board` does exactly that, but only for the tools it genuinely expects
the client to run.

## A2UI pins to v0.9

The A2UI spec is at v1.0, but every shipping toolkit — TypeScript, .NET, Python — still stamps
`v0.9`, and .NET's constants file marks these values a "cross-language wire contract" that "must
not diverge". Implementing v1.0 wire values today would mean not interoperating with any of them.
v1.0 goes behind a feature when the toolkits move.
