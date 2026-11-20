---
title: Overview
description: A2UI protocol types, the agent layer, validation, and transport choices.
---

[A2UI](https://a2ui.org/) describes a UI as components and a data model.
`ag-ui-a2ui` produces, validates, and replays those descriptions. The
application supplies the model, transport, and renderer. The crate supports
`v0.9`, `v0.9.1`, and `1.0`, and defaults to `v0.9`.

The AG-UI adapter carries surface operations. To send A2UI 1.0 function-call
messages, choose a transport that carries A2UI messages directly, such as A2A.

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
| Model authors the UI | A2UI JSON containing Card, Text and other components | `A2uiAuthor` validates and requests corrections |
| Developer authors a template | Ordinary domain data or template-tool arguments | Code constructs the components and data, then validates them |

`generate()` is an SDK convenience method, not an A2UI wire message. It does not
infer a layout from arbitrary domain data. A UI constructed without a model is
also valid A2UI. See the [template tool example](/ag-ui-rust/v1.0.0/a2ui/authoring/#expose-a-template-as-a-tool).

## Start with the task you need

| Task | API |
| --- | --- |
| Describe a known UI manually | `AgentMessage`, `Component`, `SurfaceSpec` |
| Check a component graph and its bindings | `Validator` |
| Check full schemas and a stream of surfaces | `schema_validation::SurfaceValidator` |
| Ask an async model to create or edit a surface | `A2uiAuthor` |
| Send a validated surface in AG-UI | `adapters::agui::SurfacePainter` |
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

## Choose features

| Feature | Default | Capability |
| --- | --- | --- |
| `agent` | on | Catalog negotiation, prompts, answer parsing and streaming, renderer state, and retry decisions |
| `schema-validation` | off | Draft 2020-12 JSON Schema validation with local resources |
| `author` | off | Fixed-target generation and editing. Enables `agent` and `schema-validation`. |
| `ag-ui` | off | AG-UI envelopes, history, catalog context, and tools. Enables `agent`. |
| `ag-ui-server` | off | Paint surfaces from a `RunContext`. Enables `ag-ui` and `ag-ui/server`. |
| `a2a` | off | JSON helpers for the A2A extension and data parts. Enables `agent`. |

Set `default-features = false` for the protocol modules alone. To generate
and paint a model-authored surface from an AG-UI server, enable both
`author` and `ag-ui-server`. See [Choose a version and catalog](/ag-ui-rust/v1.0.0/a2ui/versions/),
[Use the agent layer](/ag-ui-rust/v1.0.0/a2ui/agent/), and [A2UI function calls](/ag-ui-rust/v1.0.0/a2ui/functions/).

## Null and removal

This example uses the default `v0.9` rules. On a `1.0` surface, `null` removes the value.

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
than silently replacing them with null. See [Validation](/ag-ui-rust/v1.0.0/a2ui/validation/).
