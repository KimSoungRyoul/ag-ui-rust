---
title: Authoring surfaces
description: Generate and edit validated A2UI with an async provider, send it in AG-UI, and negotiate complete catalogs.
---

Enable `author` for `A2uiAuthor`, or `ag-ui-server` to also use `ctx.send_a2ui`.
An author shares immutable version, catalog, and schema settings. Each request
has independent generation state.

## Generate with an async provider

```rust
use ag_ui_a2ui::{A2uiAuthor, A2uiVersion, Result};
use serde_json::json;

async fn create_cart() -> Result<ag_ui_a2ui::ValidatedSurface> {
    let author = A2uiAuthor::basic(A2uiVersion::V0_9_1)?;
    author.create("cart", "Show the cart title")
        .generate(|prompt, attempt| async move {
            // Replace this deterministic fixture with model.complete(&prompt).await.
            if attempt == 1 {
                return Ok("invalid generated document".into());
            }
            assert!(prompt.contains("Correction required"));
            Ok(json!([
                {"version":"v0.9.1","createSurface":{
                    "surfaceId":"cart",
                    "catalogId":"https://a2ui.org/specification/v0_9/catalogs/basic/catalog.json"
                }},
                {"version":"v0.9.1","updateComponents":{
                    "surfaceId":"cart",
                    "components":[{"id":"root","component":"Text","text":"Your cart"}]
                }}
            ]).to_string())
        }).await
}
```

The callback receives an owned `String` and a one-based attempt number. Return a
JSON array, a single message, or JSON inside `<a2ui-json>` tags. Invalid generated
documents receive correction prompts up to the configured attempt limit.
Provider errors return immediately; map them with `Error::generation` to retain
their original source. Network retries belong to the caller.

No executor or mandatory `Send` bound is imposed. A Send callback/future works in
an AG-UI server agent, and a local callback can capture `Rc` on a local executor.
If a model handle is shared, clone the handle into each `async move` block.

## Edit an existing surface

```rust
use ag_ui_a2ui::{A2uiAuthor, Result, ValidatedSurface};
use serde_json::json;

async fn edit_cart(author: &A2uiAuthor, prior: &ValidatedSurface) -> Result<ValidatedSurface> {
    author.edit(prior, "Set memo to null")
        .generate(|_prompt, _attempt| async {
            Ok(json!([{"version":"v0.9.1","updateDataModel":{
                "surfaceId":"cart","path":"/memo","value":null
            }}]).to_string())
        }).await
}
```

An edit can only update the prior surface. Different target IDs, versions,
catalogs, creates, and deletes are rejected. Prior components and theme are also
revalidated against the current author's schema. Update only requested data
paths so unrelated input is preserved. Prior state is an observation and can be
older than the renderer's current user input; this API does not synchronize edits.

`ValidatedSurface` has private fields and read-only getters. `operations()` is the
current batch to send; `history()` includes all prior batches for storage. To
restore it, parse the saved history into raw JSON values and call
`author.validate_create(surface_id, &messages)`. The SDK does not deserialize an
unchecked value into `ValidatedSurface`.

## Send through AG-UI

```rust
use ag_ui::server::{AgentState, Result, RunContext};
use ag_ui_a2ui::{ValidatedSurface, agui::A2uiRunContextExt};

fn send<S: AgentState>(ctx: &mut RunContext<S>, surface: &ValidatedSurface) -> Result<()> {
    ctx.send_a2ui(surface)
}
```

This emits a `render_a2ui` tool call and an `a2ui_operations` result. A successful
return confirms local event enqueue, not renderer receipt or application. Do
not automatically resend a failed batch: earlier events may have been sent.
This API validates the whole generated batch before sending; it does not combine
partial rendering with generation retries.

## Manual operations and incremental recovery

Manual `AgentMessage`/`SurfaceSpec` builders and the low-level parser remain
available. `generate_with_recovery_async` accepts the same owned async callback
shape as the author while leaving the caller in charge of catalog/options.
This lower-level helper performs semantic validation, not full JSON Schema
validation. For incremental recovery, populate `RecoveryOptions::prior` with an
observed `SurfaceStore`; update options alone cannot reconstruct missing state.

`try_find_prior_surface` and `try_find_prior_surface_by_id` return malformed
history/lifecycle/pointer errors. The optional discovery helpers return `None`
when reconstruction fails; use the fallible versions when that distinction
matters. History replay and validation share the same surface reducer.

## Catalog negotiation

```rust
use ag_ui_a2ui::toolkit::negotiate::{ClientCapabilitiesWire, select_catalog_schema};
use serde_json::json;

let wire = ClientCapabilitiesWire::from_json(&json!({
    "v0.9":{"supportedCatalogIds":["example.com:cards"]}
})).unwrap();
let chosen = select_catalog_schema(
    &[json!({"catalogId":"example.com:cards","components":{}})],
    &wire.v0_9,
    false,
).unwrap();
assert_eq!(chosen["catalogId"], "example.com:cards");
```

`from_json` requires `schema-validation` and checks the official capabilities
schema before typed decoding. An accepted inline catalog is selected whole,
including its function array and theme, and must advertise its own ID. A
conflicting definition for an existing ID is an error. Empty capabilities do
not implicitly select the server's default.

For inline generation, `SchemaBundle::from_inline_catalog(selected)` builds the
component/function unions without changing the wire catalog ID. Supply that
bundle and any referenced local resources to `A2uiAuthor::new`. Registered
resources are local JSON values; URL-shaped `$ref`s never trigger retrieval.
