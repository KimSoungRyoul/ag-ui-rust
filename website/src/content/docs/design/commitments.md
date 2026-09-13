---
title: Design and responsibility boundaries
description: Responsibilities, guarantees and limits of the protocol SDK.
---

This SDK connects agents to user interfaces through a protocol.
Connect existing execution code through `Agent::run`, emit output with `RunContext`,
and apply incoming events to conversations and state with `Thread`.

## Application boundary

The application or framework owns model loops, executable tool registration, graph routing,
child-agent execution and durable storage. `Tool` is descriptive data; `subagent_events()` scopes output provenance.
Optional A2UI authoring helpers live in the separate `ag-ui-a2ui` crate.

## Handles and protocol verification

Message and tool handles help generate start/end events. A single context cannot lend overlapping message/tool handles.
This is a Rust convenience API constraint. The protocol permits interleaved messages and calls with different IDs;
use `ctx.emit` to represent those sequences directly.

Message, tool and step handles attempt closure on drop. Call `end()` to observe errors.
Subagent handles do not infer success on drop: explicitly `finish`, `fail` or `suspend` them.

The optional server verifier and the client verifier check ID and ordering rules.
They do not validate business authorization or tool argument JSON Schemas.
See [Verification](/ag-ui-rust/design/verification/) for the limits.

## Public types and compatibility

`Event` and `EventType` are exhaustive. An upgrade adding variants requires changes to exhaustive matches.
Such changes require a compatibility-breaking version increment, which is the next minor version during `0.x`.
`Update` and the main error enums are non-exhaustive; handle variants you do not recognize.

IDs are string newtypes. The protocol does not require UUID syntax.
Default server output IDs derive from the run ID and a counter; automatic client ID generation is separate.

## Execution and transport

`server` and `client` work without HTTP. Enabling `axum` or `http` adds Tokio-based transports.
See [Feature flags](/ag-ui-rust/reference/features/) and [Platforms](/ag-ui-rust/reference/platforms/) for dependencies.

Emission is synchronous and queues events in an unbounded channel. It does not provide backpressure for slow consumers;
the application must bound output and control production rate. Cancellation does not prove rollback or remote termination.

## Specification tracking and verification scope

The offline drift check compares Rust event names and fields with a vendored TypeScript schema snapshot.
It does not prove parity with current upstream. A separate upstream freshness check reports when the snapshot needs refreshing.

Protocol types, server/client runtimes and A2UI are covered by [tests](/ag-ui-rust/design/testing/).
Protobuf event encoding is unsupported. The `render_a2ui` envelope is this SDK's integration convention;
the receiving renderer must understand it too.
