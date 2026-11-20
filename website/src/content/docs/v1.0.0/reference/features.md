---
title: Feature flags
description: Features, defaults and dependencies for both crates.
---

The table lists manifest features and their direct implications. `dep:` denotes an optional dependency.
Dependencies of included features are enabled transitively.

| Crate | Feature | Default | Direct implications | Capability |
| --- | --- | --- | --- | --- |
| `ag-ui` | `sse` | on | — | SSE framing |
| `ag-ui` | `protobuf` | off | — | Media type only; encoding is unsupported |
| `ag-ui` | `schemars` | off | `dep:schemars` | JSON Schema derives |
| `ag-ui` | `utoipa` | off | `dep:utoipa` | OpenAPI schema derives |
| `ag-ui` | `server` | off | `dep:futures-core`, `dep:futures-channel`, `dep:futures-util`, `dep:json-patch` | Agent adapter and event emitters |
| `ag-ui` | `verify` | on | — | Server event ordering checks |
| `ag-ui` | `client` | off | `dep:futures-core`, `dep:futures-util`, `dep:json-patch`, `dep:getrandom`, `dep:jiff`, `dep:js-sys` | Thread, Update and custom transports |
| `ag-ui` | `http` | off | `client`, `sse`, `dep:reqwest`, `dep:tokio` | HttpAgent and reqwest transport |
| `ag-ui` | `axum` | off | `server`, `sse`, `dep:axum` | Axum HTTP endpoint |
| `ag-ui-a2ui` | `agent` | on | — | Catalog negotiation, prompt, stream parsing, and retry decisions |
| `ag-ui-a2ui` | `schema-validation` | off | `dep:jsonschema`, `dep:schema-getrandom` | Full schema validation with local resources |
| `ag-ui-a2ui` | `author` | off | `agent`, `schema-validation` | Fixed-target authoring and editing |
| `ag-ui-a2ui` | `ag-ui` | off | `dep:ag-ui`, `agent` | AG-UI adapter |
| `ag-ui-a2ui` | `ag-ui-server` | off | `ag-ui`, `ag-ui/server` | Paint from a `RunContext` |
| `ag-ui-a2ui` | `a2a` | off | `agent` | A2A extension and data parts |

## Choose by role

- Server: `features = ["axum"]` also enables `server` and `sse`.
- HTTP client: enable `features = ["http"]`; HTTP is not a default feature.
- Client with your own transport: use `features = ["client"]`.
- To disable optional server ordering checks, combine `default-features = false` with the features you need.

Terminal-event guards and subagent closure tracking remain when `verify` is off.
Client verification is controlled separately through `ThreadBuilder::verify(false)` or `set_verify(false)`.

## Dependencies and limitations

`http` uses Tokio through reqwest; `axum` also depends on Tokio.
The `client` feature adds ID-generation and time dependencies without adding an executor.
A2UI's `ag-ui-server` does not enable HTTP or axum.

`protobuf` supplies no binary event encoder. `ProtobufFormatter::encode` returns
`UnsupportedTransport`. Use SSE for actual communication.

Cargo unifies features for the same dependency. Setting `default-features = false`
in one place does not disable features enabled elsewhere in the dependency graph.

See [Crates and features](/ag-ui-rust/v1.0.0/start/crates/), [Platforms](/ag-ui-rust/v1.0.0/reference/platforms/), and [Verification](/ag-ui-rust/v1.0.0/design/verification/).
