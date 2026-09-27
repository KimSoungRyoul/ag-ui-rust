---
title: Documentation versions
description: Choose the Rust SDK guide that matches your dependency version.
---

# Choose your SDK version

The Rust API and AG-UI wire behavior changed between these versions. Use the
guide that matches the version in your `Cargo.lock`.

| Version | Status | Guide | Main difference |
| --- | --- | --- | --- |
| `0.4.5` | Stable release | [Open the 0.4.5 guide](/ag-ui-rust/v0.4.5/start/) | Existing conversation and A2UI APIs |
| `0.5.0-alpha.1` | Prerelease | [Open the 0.5.0-alpha.1 guide](/ag-ui-rust/v0.5.0-alpha.1/start/) | AG-UI 1.0 events, structured tool results and new run outcomes |

Both guides cover `ag-ui` and the separate `ag-ui-a2ui` crate. A versioned
guide describes that exact source version; check [crates.io](https://crates.io/crates/ag-ui)
for registry availability before using its install command.

The [0.5 migration notes](https://github.com/KimSoungRyoul/ag-ui-rust/blob/main/docs/migration-0.5.md)
summarize source and wire changes. The 0.4.5 pages are frozen from the
[`v0.4.5` tag](https://github.com/KimSoungRyoul/ag-ui-rust/tree/v0.4.5/website/src/content/docs).
