---
title: Choose a version and catalog
description: Select the A2UI wire version and its catalog for an agent session, AG-UI, or A2A.
---

A2UI has its own version, independent of the AG-UI protocol and SDK release.
The default is `A2uiVersion::V0_9`. `A2uiVersion::LATEST` names `V1_0`;
select v1.0 explicitly when the renderer supports it.

| Where | v0.9 family | v1.0 family |
| --- | --- | --- |
| Message `version` | `"v0.9"` or `"v0.9.1"` | `"1.0"` |
| Dynamic binding and function reference | `path`, `call` | `@path`, `@call` |
| Data model removal | Omit `value` | Set `value` to `null` |
| Surface creation | Separate updates for components and data. Optional `theme`. | Can include `components` and `dataModel`. Optional default `catalogId`, with per-component overrides. |
| Capabilities in metadata | `a2uiClientCapabilities`, namespace `v0.9` | `a2uiRendererCapabilities`, namespace `1.0` |

A surface stays in its original wire family. A v0.9.1 message can update a
v0.9 surface, but v1.0 messages require a v1.0 surface. Use `version.path()`,
`version.call()` and `version.component_keys()` to work with each version's
spelling. In v1.0, a leading `@` is reserved in dynamic values. Escape a literal
leading `@` there as `@@`. A component can select its catalog with
`@catalogId`. Multi-catalog surfaces use semantic checks, with a
[known gap in JSON Schema validation](/ag-ui-rust/v1.0.0/a2ui/validation/#verification-scope).

## Use the version at each layer

1. For a standalone agent session, register `CatalogConfig::basic(version)` with
   the generator. One session uses one family. `Catalog::basic_for(version)` and
   `SchemaBundle::for_version(version)` give the published basic catalog and its
   schema for hand-authored protocol messages.
2. For AG-UI, read `CatalogContext::protocol_version()` from the frontend
   catalog. Pass that version to `CatalogContext::capabilities`,
   `render_a2ui_tool_for`, and `SurfaceSpec::with_version`. The v0.9 frontend
   convention uses `BASIC_CATALOG_ID`, which differs from the published
   `OFFICIAL_BASIC_CATALOG_ID`. `adapters::agui::basic_catalog(version)` and
   `basic_catalog_id(version)` use the right ID for the adapter.
3. For A2A, enable `a2a`, advertise an extension for each supported version, and
   answer with A2UI messages written in the version selected by `activate`.

```rust
use ag_ui_a2ui::{A2uiVersion, adapters::a2a};

let old = a2a::extension_uri("0.9.1");
let new = a2a::extension_uri("1.0");
let extensions = [old, new.clone()];
let (uri, version) = a2a::activate(&extensions, &extensions).unwrap();
assert_eq!(uri, new);
assert_eq!(version, A2uiVersion::V1_0);
```

The A2A `data_part` helper carries the message as JSON, including
[function calls](/ag-ui-rust/v1.0.0/a2ui/functions/) in v1.0. The AG-UI adapter
carries surface operations.

A caller staying on the v0.9 family still picks up SDK changes after alpha.3.
For example, removing an absent path now leaves the model untouched. Prompt
fixtures that pin the exact `DirectJson` text need updating. Follow the
[migration guide](https://github.com/KimSoungRyoul/ag-ui-rust/blob/v1.0.0/docs/migration-0.5.md#changes-in-100)
when updating a checkout.
