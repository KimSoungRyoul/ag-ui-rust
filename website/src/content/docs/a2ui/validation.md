---
title: Validation and replay
description: Full local schema validation, transactional surface lifecycle, forward references, and lossless data-model snapshots.
---

Use the validation level that matches the input. `Validator` checks semantic
relationships and basic property/envelope constraints. `SchemaValidator` and
`SurfaceValidator` add full Draft 2020-12 validation from the pinned official
schemas. `A2uiAuthor` always uses full validation; disabling its schema feature
does not substitute a weaker validator.

## Validate a stream

```rust
use ag_ui_a2ui::schema_validation::SurfaceValidator;
use ag_ui_a2ui::toolkit::schema::SchemaBundle;
use serde_json::json;
use std::collections::BTreeMap;

let bundle = SchemaBundle::basic().unwrap();
let mut stream = SurfaceValidator::new();
stream.register(&bundle, &BTreeMap::new()).unwrap();
stream.push(&json!({"version":"v0.9.1","createSurface":{
    "surfaceId":"cart","catalogId":bundle.catalog_id().unwrap()
}})).unwrap();
assert!(stream.finish().is_err()); // The root has not arrived yet.
stream.push(&json!({"version":"v0.9.1","updateComponents":{
    "surfaceId":"cart",
    "components":[{"id":"root","component":"Text","text":"Your cart"}]
}})).unwrap();
stream.finish().unwrap();

// Retained state is explicit when validating a later partial stream.
let mut update = SurfaceValidator::from_state(stream.state().clone());
update.register(&bundle, &BTreeMap::new()).unwrap();
update.push(&json!({"version":"v0.9.1","updateDataModel":{
    "surfaceId":"cart","path":"/memo","value":null
}})).unwrap();
update.finish().unwrap();
```

Each `push` checks raw JSON before serde can add defaults or ignore fields.
It applies a candidate state only after success. A malformed pointer, duplicate
component ID in one message, or invalid schema leaves accepted state unchanged.
Already transmitted renderer messages are not rolled back.

Forward child references and a missing root are pending while the stream is
open; `finish()` requires complete graphs and resolved bindings. A later message
may replace a component ID, and two surfaces can each have their own `root`.
Updates/deletes require an active surface. Duplicate creates are rejected;
delete followed by create may reuse the ID.

Register each complete catalog separately for a multi-catalog stream. The state
belongs to one renderer/conversation context; do not share it across users.
Unknown IDs or `$ref`s fail locally. Neither catalog URLs nor references are
fetched from the network or filesystem.

## Inspect semantic diagnostics

```rust
use ag_ui_a2ui::{Catalog, Component, ErrorCode, Validator};
use serde_json::json;

let report = Validator::new(&Catalog::basic()).validate(&[
    Component::new("root", "Card").with("child", json!("missing")),
]);
assert!(report.errors.iter().any(|error| error.code == ErrorCode::UnresolvedChild));
```

`ValidationReport` retains machine-readable codes, locations, and correction
messages. Unreachable components are reported separately as warnings.
`Validator::incremental().validate(components)` checks an isolated component
fragment; it does not establish a valid renderer lifecycle. Whole message
streams use `validate_messages`, or `validate_updates` with explicit prior state.

Schema checks cover enums, nested properties, extra fields, and function
arguments; graph/binding checks cover what JSON Schema cannot express. The full
surface validator also checks custom composition constraints. Root, graph depth,
and binding policies are configurable on the lower-level semantic validator.

## Preserve undefined values

```rust
use ag_ui_a2ui::{DataModel, DataModelUpdate, ModelValue};
use serde_json::{json, Value};

let mut model = DataModel::from(json!({"items":[null,2]}));
model.apply("/items/1", &DataModelUpdate::Remove).unwrap();
assert_eq!(model.lookup("/items/0").unwrap(), Some(&ModelValue::Null));
assert_eq!(model.lookup("/items/1").unwrap(), None);
assert!(model.to_json().is_err());

let saved = serde_json::to_vec(&model).unwrap();
let restored: DataModel = serde_json::from_slice(&saved).unwrap();
assert_eq!(restored, model);
model.apply("/", &DataModelUpdate::Set(Value::Null)).unwrap();
assert_eq!(model.to_json().unwrap(), Value::Null);
```

Upserts create missing/null intermediate containers. Numeric paths infer arrays,
and sparse gaps become Undefined. Implicit expansion is limited to 65,536 new
array slots per update; `DataModel::apply_with_array_growth_limit` accepts an
explicit budget. Failed paths or allocation limits leave the old model intact.

The snapshot has a format version and preserves `Undefined`. It is an internal
storage representation, never an A2UI wire value. Unknown snapshot versions are
rejected. Export to ordinary JSON fails if any undefined root/slot would be lost.
The original update operations remain the wire representation.

`binding::ModelScope` distinguishes explicit null from missing/undefined and
supports collection scopes. `Validator::validate_model` checks bindings against
this lossless state, including defined template items. JSON-based `Scope` and
`validate_surface` remain useful when the model is entirely representable as JSON.

## Verification scope

The official v0.9.1 schemas are pinned at upstream commit `1c45c809`. The schema
engine is `jsonschema 0.29.1`, with default retrieval features disabled and a
rejecting external retriever. Native Rust 1.85 and wasm builds are checked.

`tests/author.rs` covers real schema evaluation, local resources, async
correction, target/catalog/version checks, and transactional multi-catalog
streams. `tests/protocol_091.rs` covers lossless updates, snapshots, and replay.
The independent applications under `examples/` exercise public SDK calls.

The older toolkit conformance suite reports **119 passed, 74 skipped, 0 failed**.
Four named vectors require implicit catalog fallback or merging different
catalogs under one ID; these policies are superseded by explicit matching and
conflict-rejection regressions. Other exclusions are v0.8/rendering/example-file
checks documented in the vendored conformance README. Legacy fragment vectors
exercise component validation, not an invented complete renderer history.
