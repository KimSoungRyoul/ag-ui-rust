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
| A2UI surface authoring or validation | `ag-ui-a2ui` | Choose features in the [A2UI guide](/ag-ui-rust/v0.5.0-alpha.3/a2ui/) |

`axum` includes `server` and `sse`; `http` includes `client` and `sse`.
Protocol types such as `Message`, `Tool`, `Event` and `RunAgentInput` live at the crate root.

## Add dependencies

This project uses `ag-ui` and `ag-ui-a2ui`. The crates.io names `ag-ui-core`,
`ag-ui-server` and `ag-ui-client` belong to a separate community SDK.

Pin the exact prerelease version so Cargo does not select a later alpha or
stable release without a deliberate dependency update:

```toml
[dependencies]
ag-ui = { version = "=0.5.0-alpha.3", features = ["axum"] }
# Add this only if your application uses A2UI:
ag-ui-a2ui = { version = "=0.5.0-alpha.3", features = ["author"] }
```

For an HTTP client, replace `axum` with the `http` feature. Add
`ag-ui-a2ui` only when your application uses A2UI.

For source checkout QA, replace `version` with a local `path` pointing to the
same tagged checkout for both crates. A registry requirement resolves only
after crates.io has indexed the version. The [0.4.5 guide](/ag-ui-rust/v0.4.5/start/crates/)
retains that release's API and installation instructions.

[Build a server](/ag-ui-rust/v0.5.0-alpha.3/server/) and [Build a client](/ag-ui-rust/v0.5.0-alpha.3/client/) include the relevant
`Cargo.toml` and runnable code. Separate programs only need their own dependencies.

A2UI defaults to `toolkit` alone. Enable `ag-ui` for history/tool adapters,
`author` for generation, or `ag-ui-server` for `ctx.send_a2ui` (which includes
both the author and adapter). Upgrading from alpha.2 may require an explicit
`ag-ui` feature.

## Runtimes and platforms

The `server` and `client` APIs separate transport from execution. `axum` connects a Tokio HTTP server;
`http` connects a reqwest HTTP client. With `client` alone, you supply the transport.

Cargo unifies features requested for the same dependency. A dependency graph that requests
both server and client builds both sets of APIs.

See [Feature flags](/ag-ui-rust/v0.5.0-alpha.3/reference/features/) for the full list and
[Platforms and MSRV](/ag-ui-rust/v0.5.0-alpha.3/reference/platforms/) for supported environments.
