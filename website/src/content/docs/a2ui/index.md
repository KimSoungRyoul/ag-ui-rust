---
title: Overview
description: A2UI v0.9/v0.9.1 protocol, optional authoring and schema validation, and the AG-UI transport integration.
---

[A2UI](https://a2ui.org/specification/v0.9.1-a2ui/) describes a UI as components
and a data model. `ag-ui-a2ui` produces, validates, and replays those descriptions.
The application supplies the generating model and the renderer.

A2UI has its own protocol version and remains separate from AG-UI. The optional
AG-UI integration sends validated operations as a `render_a2ui` tool result with
an `a2ui_operations` envelope. Other transports can use the protocol types
directly. The MIME type is `application/a2ui+json`.

## Understand A2UI through a weather card

For “What is the weather in Seoul?”, a text answer might be “Clear, 21°C”. A2UI can
instead send a JSON **UI description**: show a card, give it the title “Seoul weather”,
and display “Clear, 21°C” inside. The application's renderer turns that description
into visible components. These messages declare supported widgets, not executable HTML or JavaScript.

| Participant | Responsibility |
| --- | --- |
| Model or application code | Choose the content and UI composition |
| A2UI | Represent that composition in a common JSON format |
| Renderer | Display the described UI |

There are two ways to choose the composition.

| Approach | Model output | SDK usage |
| --- | --- | --- |
| Model authors the UI | A2UI JSON containing Card, Text and other components | `A2uiAuthor.generate()` validates and requests corrections |
| Developer authors a template | Ordinary domain data or template-tool arguments | Code constructs the components and data, then validates them |

`generate()` is an SDK convenience method, not an A2UI wire message. It does not
infer a layout from arbitrary domain data. A UI constructed without a model is
also valid A2UI. See the [template tool example](/ag-ui-rust/a2ui/authoring/#expose-a-template-as-a-tool).

## Start with the task you need

| Task | API |
| --- | --- |
| Describe a known UI manually | `AgentMessage`, `Component`, `SurfaceSpec` |
| Check a component graph and its bindings | `Validator` |
| Check full schemas and a stream of surfaces | `schema_validation::SurfaceValidator` |
| Ask an async model to create or edit a surface | `A2uiAuthor` |
| Send a validated surface in AG-UI | `A2uiRunContextExt::send_a2ui` |
| Restore lossless local data | `DataModel`, `SurfaceStore` |

```rust
use ag_ui_a2ui::{Catalog, Component, Validator};
use serde_json::json;

let catalog = Catalog::basic();
let components = [
    Component::new("root", "Column").with("children", json!(["title"])),
    Component::new("title", "Text").with("text", json!({"path":"/title"})),
];
let report = Validator::new(&catalog)
    .validate_surface(&components, Some(&json!({"title":"Your cart"})));
assert!(report.is_valid());
```

Components form a flat adjacency list: parents refer to child IDs. Each surface
has its own `root` and its own component IDs. A later message replaces the
previous definition with the same ID; duplicates inside one message are invalid.

## Versions and features

The crate accepts `v0.9` and `v0.9.1`. Low-level constructors retain `v0.9` as the
default; `.with_version(A2uiVersion::V0_9_1)` chooses the other discriminator.
`A2uiAuthor::basic(version)` uses the pinned official v0.9.1 schemas, which
accept both versions.

The server messages are `createSurface`, `updateComponents`, `updateDataModel`,
and `deleteSurface`. Renderer replies use `action` or `error`. Candidate RPC
shapes cannot be encoded or decoded as v0.9-family messages; retaining those
Rust types does not advertise v1.0 support.

| Feature | Default | Added capability |
| --- | --- | --- |
| `toolkit` | on | Manual operations, prompt/parser, history, sync and async recovery |
| `ag-ui` | on | AG-UI history and tool definitions |
| `schema-validation` | off | Draft 2020-12, pinned schemas, local resource registry |
| `author` | off | Shared author configuration and immutable validated output |
| `ag-ui-server` | off | Author plus AG-UI server event emission |

The new features do not implicitly add HTTP, axum, or Tokio. With defaults
disabled, protocol/model users do not depend on AG-UI or a schema engine.
Native Rust 1.85 and wasm builds are checked. The optional schema engine enables
browser entropy only on the relevant wasm target.

## Catalog agreement

`A2uiAuthor::basic` uses
`https://a2ui.org/specification/v0_9/catalogs/basic/catalog.json`, the exact ID
inside the official catalog. The historical `BASIC_CATALOG_ID` remains available
for existing manual integrations. The SDK does not assume they identify the
same renderer implementation; register an alias explicitly when that is true.

Capabilities use a `v0.9` namespace even for v0.9.1 messages. Inline catalogs
remain complete documents under their own advertised IDs. Missing matches and
conflicting definitions are errors. See [Authoring surfaces](/ag-ui-rust/a2ui/authoring/).

## Data deletion is distinct from null

```rust
use ag_ui_a2ui::AgentMessage;
use serde_json::Value;

let set_null = AgentMessage::update_data_model("cart", "/memo", Value::Null);
let remove = AgentMessage::remove_data_model_value("cart", "/memo");
assert!(serde_json::to_value(&set_null).unwrap()["updateDataModel"].get("value").is_some());
assert!(serde_json::to_value(&remove).unwrap()["updateDataModel"].get("value").is_none());
```

Removal deletes object keys, leaves undefined array slots without shortening the
array, and makes the root undefined after root deletion. `DataModel` preserves
that distinction in local snapshots. `to_json()` rejects undefined values rather
than silently replacing them with null. See [Validation](/ag-ui-rust/a2ui/validation/).
