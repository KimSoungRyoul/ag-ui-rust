---
title: Validation and replay
description: Full local schema validation, transactional surface lifecycle, forward references, and lossless data-model snapshots.
---

Use the validation level that matches the input. `Validator` checks semantic
relationships and basic property/envelope constraints. `SchemaValidator` and
`SurfaceValidator` add full Draft 2020-12 validation from the pinned official
schemas. `A2uiAuthor` always uses full validation; disabling its schema feature
does not substitute a weaker validator.

## Why the SDK includes JSON files

The bundled schema directories contain the official rules for each version.
`server_to_client.json` defines message structure, `common_types.json` defines
bindings and other shared types, and `catalog.json` defines component properties.

The SDK vendors an unmodified upstream revision and embeds these resources through
`include_str!`. Runtime execution does not download them. `A2uiAuthor` uses the same
rules in model prompts and output validation; code-authored templates can use them too.

| Stage | Responsibility | Example |
| --- | --- | --- |
| JSON parsing | Check JSON syntax | Reject broken commas or brackets |
| JSON Schema validation | Check declared fields, types and structure | Reject missing catalogId or numeric surfaceId |
| Semantic/state validation | Check component links and ordered updates | Reject unresolved children or duplicate creates |

The JSON file is the rule definition, not executable validation logic. The `jsonschema`
library interprets it, while Rust code checks state and relationships. With `generate()`,
the model authors UI JSON and the SDK checks it. These rules do not automatically turn
ordinary weather data into a layout.

## Validate a stream

```rust
use ag_ui_a2ui::schema_validation::SurfaceValidator;
use ag_ui_a2ui::schema::SchemaBundle;
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
open; `finish()` requires complete graphs and valid binding structure.
Missing data values are allowed under the default binding policy. A later message
may replace a component ID, and two surfaces can each have their own `root`.
The example's `SurfaceValidator` expects an existing surface for updates and
deletes. Duplicate creates are rejected;
delete followed by create may reuse the ID.

Register each complete catalog separately if the stream carries surfaces from
different catalogs. For a v1.0 surface with per-component catalog IDs, use the
protocol's `processor::MessageProcessor` and see the JSON Schema limitation below. The state
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

## Validate bindings before values arrive

Missing or undefined bound values are accepted by default: a renderer or later
update may populate a form field. Malformed pointers and existing non-array
collections still fail. Set `require_bound_values` when linting a complete data
model, while leaving `check_bindings` enabled:

```rust
use ag_ui_a2ui::{Catalog, Component, ValidateOptions, Validator};
use serde_json::json;

let catalog = Catalog::basic();
let components = [Component::new("root", "Text")
    .with("text", json!({"path":"/later"}))];
assert!(Validator::new(&catalog)
    .validate_surface(&components, Some(&json!({}))).is_valid());
let strict = Validator::with_options(&catalog, ValidateOptions {
    require_bound_values: true,
    ..Default::default()
});
assert!(!strict.validate_surface(&components, Some(&json!({}))).is_valid());
```

`validate_surface` and `validate_model` inspect every defined collection item,
including nested templates. The latter keeps undefined array slots intact.

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

## Data model limits

Data model paths allow 256 segments from the root and reject reserved segments.
An array write can address an index up to 10,000. The defaults allow up to
65,536 new slots per update and 65,536 undefined slots per model.
`DataModel::undefined_slots()` reports the current count. Removing an absent
path leaves the model unchanged and allocates no containers or array slots.

## Validate renderer messages

With `schema-validation`, `RendererMessage::from_json` validates original JSON
before serde applies compatibility defaults. Reuse `ClientSchemaValidator` for
multiple messages:

```rust
use ag_ui_a2ui::client_schema::ClientSchemaValidator;
use serde_json::json;

let validator = ClientSchemaValidator::new()?;
let raw = json!({"version":"v0.9.1","action":{
    "name":"submit","surfaceId":"cart","sourceComponentId":"submit",
    "timestamp":"2026-09-28T00:00:00Z","context":{},
    "userMessage":{"text":"Place the order"}
}});
let message = validator.decode_message(&raw)?;
assert_eq!(serde_json::to_value(message)?, raw);
# Ok::<(), ag_ui_a2ui::Error>(())
```

Missing action `context`, missing error `surfaceId`, and validation errors
without `path` are rejected. `Action.extensions` preserves additional action
fields; `RendererError.extensions` preserves generic-error fields including null
or structured values. Read a validation error's string pointer with `path()`.
Direct serde decoding remains available as a tolerant low-level API.

The pinned `client_to_server.json` and `client_data_model.json` schemas also
cover renderer replies and form snapshots. `ClientDataModel::from_json` validates
the snapshot metadata without applying it; see
[renderer form snapshots](/ag-ui-rust/v1.0.0/a2ui/authoring/#apply-renderer-form-snapshots).

## Verification scope

The pinned A2UI corpus reports 948 passed, 34 expected divergences,
1,053 skipped, and 3 known failures. The known failures concern JSON Schema
validation of v1.0 messages drawing on multiple catalogs. The
[conformance README](https://github.com/KimSoungRyoul/ag-ui-rust/blob/v1.0.0/crates/ag-ui-a2ui/tests/conformance/README.md)
records the per-suite counts and reasons.

[Review Desk](/ag-ui-rust/v1.0.0/examples/review-desk/) separately compares state
against `@a2ui/web_core` 0.15.0 using `v0.9.1` and `1.0` messages.
