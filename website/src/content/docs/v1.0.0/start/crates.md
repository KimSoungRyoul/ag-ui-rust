---
title: Crates and features
description: Select the crates and features your application needs.
---

`ag-ui` provides AG-UI protocol types and server/client APIs. Enable features for the
role you are building. `ag-ui-a2ui` handles the separate A2UI protocol.

## Choose by application role

| Your application | Dependency and features | Main components |
| --- | --- | --- |
| HTTP agent server | `ag-ui`, `features = ["axum"]` | `Agent`, `RunContext`, `RouterExt` |
| HTTP agent client | `ag-ui`, `features = ["http"]` | `HttpAgent`, `Thread`, `Update` |
| Server without HTTP | `ag-ui`, `features = ["server"]` | `run`, `Runner`, event stream |
| Client with a custom transport | `ag-ui`, `features = ["client"]` | `Thread`, `Transport` |
| Server calling another agent | `ag-ui`, `features = ["axum", "http"]` | Both server and client components |
| A2UI surface authoring or validation | `ag-ui-a2ui` | Choose features in the [A2UI guide](/ag-ui-rust/v1.0.0/a2ui/) |

`axum` includes `server` and `sse`; `http` includes `client` and `sse`.
Protocol types such as `Message`, `Tool`, `Event` and `RunAgentInput` live at the crate root.

## Add dependencies

This project uses `ag-ui` and `ag-ui-a2ui`. The crates.io names `ag-ui-core`,
`ag-ui-server` and `ag-ui-client` belong to a separate community SDK.

Add the registry dependencies to your application’s `Cargo.toml`.

```toml
[dependencies]
ag-ui = { version = "1.0.0", features = ["axum"] }
ag-ui-a2ui = { version = "1.0.0", features = ["author"] }
```

For an HTTP client, use `http` in place of `axum`. Add `ag-ui-a2ui` when
your application uses A2UI.

A2UI defaults to `agent`. Enable `author` for fixed-surface generation with
JSON Schema validation, `ag-ui` for AG-UI history and tools, `ag-ui-server`
for `SurfacePainter`, or `a2a` for A2A data parts. See the [A2UI overview](/ag-ui-rust/v1.0.0/a2ui/)
for how to combine validation and surface painting.

## Runtimes and platforms

The `server` and `client` APIs separate transport from execution. `axum` connects a Tokio HTTP server;
`http` connects a reqwest HTTP client. With `client` alone, you supply the transport.

Cargo unifies features requested for the same dependency. A dependency graph that requests
both server and client builds both sets of APIs.

See [Feature flags](/ag-ui-rust/v1.0.0/reference/features/) for the full list and
[Platforms and MSRV](/ag-ui-rust/v1.0.0/reference/platforms/) for supported environments.
