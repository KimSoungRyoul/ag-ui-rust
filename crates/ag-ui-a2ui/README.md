# ag-ui-a2ui

Rust types and validation for A2UI `v0.9`, `v0.9.1`, and `1.0`, with an
agent layer for catalog negotiation, prompts, model output and retries.
The application supplies the model, transport and renderer.

Install the crate and enable the features used by your application:

```toml
[dependencies]
ag-ui-a2ui = "1.0.0"
```

See the [1.0.0 A2UI guide](https://kimsoungryoul.github.io/ag-ui-rust/v1.0.0/a2ui/)
and the [migration notes](https://github.com/KimSoungRyoul/ag-ui-rust/blob/v1.0.0/docs/migration-0.5.md#changes-in-100)
for changes since 0.5.0-alpha.3.

## Minimal component validation

```rust
use ag_ui_a2ui::{Catalog, Component, Validator};

let catalog = Catalog::basic();
let components = [Component::new("root", "Text").with("text", "Hello!".into())];
assert!(Validator::new(&catalog).validate(&components).is_valid());
```

`Validator` checks graph, binding, envelope, and basic property constraints.
It does not claim full JSON Schema validation. `SchemaValidator` and
`SurfaceValidator` provide Draft 2020-12 validation behind `schema-validation`.

Missing or undefined bound values are accepted by default, including fields
initialized by user input or a later update. Malformed JSON Pointers and existing
non-array template collections still fail. To lint missing values as well, use
`Validator::with_options` with
`ValidateOptions { require_bound_values: true, ..Default::default() }`.
Both `validate_surface` and `validate_model` check every defined collection item,
including nested templates; the latter also preserves undefined array slots.

## Async authoring

Enable `author` to generate or edit a single surface under a fixed version and
catalog with schema validation. Examples using the `json!` macro also require
`serde_json = "1"` in your application. To also send it from an AG-UI `RunContext`,
enable `ag-ui-server` alongside `author` and use `adapters::agui::SurfacePainter`.

```rust
# #[cfg(feature = "author")]
# async fn example() -> ag_ui_a2ui::Result<()> {
use ag_ui_a2ui::{A2uiAuthor, A2uiVersion};
use serde_json::json;

let author = A2uiAuthor::basic(A2uiVersion::V0_9_1)?.with_send_data_model(true);
let surface = author.create("cart", "Show the cart title")
    .generate(|_prompt, _attempt| async {
        // Replace this deterministic provider with your async model client.
        Ok(json!([
            {"version":"v0.9.1","createSurface":{
                "surfaceId":"cart",
                "catalogId":"https://a2ui.org/specification/v0_9/catalogs/basic/catalog.json",
                "sendDataModel":true
            }},
            {"version":"v0.9.1","updateComponents":{
                "surfaceId":"cart",
                "components":[{"id":"root","component":"Text","text":"Your cart"}]
            }}
        ]).to_string())
    }).await?;
assert_eq!(surface.surface_id(), "cart");
# Ok(())
# }
```

The callback receives an owned prompt and a one-based attempt number. Invalid
model documents receive correction prompts; provider failures propagate
immediately through `Error::generation`, preserving their source. Send callbacks
produce Send futures; local callbacks are also accepted.

`with_send_data_model(true)` requires `sendDataModel: true` on creation, asking
the renderer to attach current form input to later requests. The author includes
this flag in its prompt and validates generated or restored surfaces against it.
`false` accepts either an explicit false or omission; without the builder, either
setting is accepted.

`author.edit(&surface, request)` allows only updates to that existing surface.
A different surface ID, catalog, version, create, or delete is rejected. The
private `ValidatedSurface` fields prevent mutation after validation.
`operations()` is the current batch; `history()` includes the prior batches and
can be saved and revalidated with `validate_create`. Use `SurfacePainter::from_history(ctx.messages())` and
`painter.paint(ctx, surface.operations())` to enqueue a batch on AG-UI. Observe
the renderer separately if your application needs delivery or application receipts.

For a framework that manages multiple surfaces or reads model output as it
streams, use `agent::A2uiGenerator` to build an `A2uiRequestProcessor`
for each request. It supplies the prompt snippet and the active catalogs.
`RetryDriver` reports whether to accept an answer or try again. Open a
`ResponseStream` to process chunks as they arrive. The framework calls the
model, decides when to send messages, and owns the lifetime of renderer state.

## Renderer messages

Use `RendererMessage::from_json(&raw)` with `schema-validation` to check the
original client message before serde applies compatibility defaults. This rejects
missing action `context`, missing error `surfaceId`, and validation errors without
`path`. For repeated messages, reuse `client_schema::ClientSchemaValidator` and its
`decode_message` method. Direct serde decoding remains available for applications
that need the tolerant low-level types.

Action and generic-error extension fields are retained in their `extensions`
maps, including null or structured `userMessage`, `path`, and `functionCallId`
values. `RendererError::path()` reads a validation error's string pointer.
Typed error codes remain strings, matching the official renderer. Use
`agent::RendererCapabilities` to read supported catalogs for the session's
protocol version.

`ClientDataModel::from_json(&metadata)` reads `a2uiClientDataModel` snapshots
sent by renderers for surfaces with `sendDataModel: true`. Its `surfaces` map
contains the latest form input as JSON objects. The application selects an owned
surface, applies its snapshot through a root `updateDataModel`, and validates the
edit before generating a follow-up. The metadata type itself does not mutate state.

## Null and removal

```rust
use ag_ui_a2ui::{AgentMessage, DataModel, DataModelUpdate, ModelValue};
use serde_json::{json, Value};

let store_null = AgentMessage::update_data_model("cart", "/memo", Value::Null);
let remove = AgentMessage::remove_data_model_value("cart", "/memo");
assert!(serde_json::to_value(remove).unwrap()["updateDataModel"].get("value").is_none());

let mut model = DataModel::from(json!({"items": [null, 2]}));
model.apply("/items/1", &DataModelUpdate::Remove).unwrap();
assert_eq!(model.lookup("/items/0").unwrap(), Some(&ModelValue::Null));
assert_eq!(model.lookup("/items/1").unwrap(), None);
assert!(model.to_json().is_err()); // JSON cannot express the undefined slot.
```

`DataModel::apply_as` uses the rules for the surface's version. The examples
above use v0.9 rules. On a `1.0` surface, `null` removes a value. Removing an
absent path leaves the model unchanged in every version.

Writes can create missing containers and sparse array slots. The model defaults
to a maximum of 65,536 new slots per update and 65,536 undefined slots per model.
Array writes are capped at index 10,000. Data model paths allow at most 256
segments from the root and reject reserved path segments. A failed update leaves
the old model intact.

Deletion removes object keys, preserves array lengths with `Undefined` slots,
and leaves an `Undefined` root on root deletion. The versioned `DataModel`
snapshot preserves these values; plain JSON export fails if it would lose them.
`binding::ModelScope` and `Validator::validate_model` read the lossless model.
Plain JSON `Scope` and `UpdateDataModel::apply` remain available; the latter
rejects a result requiring Undefined instead of silently substituting null.

## Features and protocol

| Feature | Default | Adds |
| --- | --- | --- |
| `agent` | yes | Prompt assembly, catalog negotiation, parsing and streaming, renderer state, retry decisions. |
| `schema-validation` | no | Pinned schemas and a Draft 2020-12 engine with local resources. |
| `author` | no | Fixed-target async generation and immutable validated surfaces. Enables `agent` and `schema-validation`. |
| `ag-ui` | no | AG-UI envelopes, history, catalog context and tool definitions. Enables `agent`. |
| `ag-ui-server` | no | Paint surfaces from a `RunContext`. Enables `ag-ui` and `ag-ui/server`. |
| `a2a` | no | JSON helpers for the A2A extension and data parts. Enables `agent`. |

Set `default-features = false` to use the protocol modules alone. The protocol
modules and agent layer are transport independent. The optional `ag-ui` feature
adds the AG-UI dependency. `author` and `ag-ui-server` do not add an HTTP client
or executor.

Low-level constructors default to `v0.9`. Use `A2uiVersion::V1_0` to select
`1.0` explicitly. An agent session takes its protocol version from its catalogs.
Messages can update a surface of the same wire family. To construct bindings,
use `version.path("/field")` so the key matches that family. The crate does not
convert messages between families.

`Catalog::basic_for(version)` and `A2uiAuthor::basic(version)` use the catalog ID
published for the chosen version. For AG-UI frontends, read
`CatalogContext::protocol_version()` and use `adapters::agui::basic_catalog(version)`
to get the catalog under the ID the adapter uses. In v0.9 it is
`BASIC_CATALOG_ID`, the frontend's catalog ID. In v1.0 it is the published ID.

## A2UI 1.0 function calls

`agent::RendererMessageRouter` routes a renderer message to the application
or returns a protocol reply. `agent::RendererCalls` prepares outbound calls
and tracks responses using timestamps supplied by the application. The
application runs handlers and schedules deadlines. The AG-UI adapter's
`a2ui_operations` envelope carries surface operations. It rejects batches that
include function-call messages. Use a message-preserving transport such as the
`a2a` adapter for function calls.

## Verification

`tests/official_examples.rs` runs the example surfaces upstream publishes
beside each basic catalog, 44 of v0.9, 43 of v0.9.1 and 43 of v1.0, against the
schemas and the catalog of their own version, message by message and as the
surfaces they leave. The files are under `tests/conformance/`, at their
upstream paths, from the commit `tests/conformance/README.md` records.

`tests/author.rs` exercises the official schema engine, local refs, targeted
async generation, catalog negotiation, and multi-surface transitions.
`tests/protocol_091.rs` covers lossless model/history replay and lifecycle.
The official conformance suite is vendored whole, and every suite in it is
accounted for by name: executed, or skipped with the reason. The executed
suites are the seven of the v0.9 core (validator, data context, data model,
expressions, message processor, catalog, common types), the ten of the v1.0
core that are not a renderer's own (validator, message processor, data
deletion, data-model pointers, reserved keys, several catalogs, composition
constraints, the `@index` function, validation results, and function calls as
far as carrying and routing one goes), the fifteen of the agent tier that do
not need another output format than direct JSON, each case as it is written,
and the A2A extension suite, with the transcripts of v0.9.1 and of v1.0; they
report **948 passed, 34 expected divergences, 1,053 skipped, 3 known failures**,
and the per-suite counts are asserted by the test. The divergences
are executed with pinned expectations: an unreachable component is a warning,
an empty path segment is the empty key, a binding that cannot resolve is an
error, and a streamed `updateDataModel` that states no path is emitted when its
text closes. The failures are listed by name with what each case wants: three
about JSON Schema validation of a v1.0 message that draws from several
catalogs.
The three suites of the SDK design upstream had before
its agent layer are not executed, since the code they describe is gone. See
[conformance notes](tests/conformance/README.md) and [schema provenance](schemas/README.md).

```sh
cargo test -p ag-ui-a2ui --all-features
cargo +1.85.0 check -p ag-ui-a2ui --all-features
cargo check -p ag-ui-a2ui --all-features --target wasm32-unknown-unknown
```

License: MIT for SDK code; the vendored official schemas and conformance suite
retain Apache-2.0.
