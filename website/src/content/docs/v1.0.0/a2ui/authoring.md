---
title: Authoring surfaces
description: Generate and edit validated A2UI with an async provider, send it in AG-UI, and negotiate complete catalogs.
---

Enable `author` for `A2uiAuthor`. Add `ag-ui-server` to paint through a `RunContext`.
An author shares immutable version, catalog, and schema settings. Each request
has independent generation state.

## Generate with an async provider

```rust
use ag_ui_a2ui::{A2uiAuthor, A2uiVersion, Result};
use serde_json::json;

async fn create_cart() -> Result<ag_ui_a2ui::ValidatedSurface> {
    let author = A2uiAuthor::basic(A2uiVersion::V0_9_1)?.with_send_data_model(true);
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
                    "catalogId":"https://a2ui.org/specification/v0_9/catalogs/basic/catalog.json",
                    "sendDataModel":true
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

`with_send_data_model(true)` requires the creation to include `sendDataModel:
true`; the same policy is checked when restoring history or editing a surface.
This asks the renderer to attach current form input to later requests. Setting
`false` accepts omission or explicit false; without a policy, either is valid.

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

## Apply renderer form snapshots

Extract the `a2uiClientDataModel` metadata value from the incoming request and
validate it before applying the snapshot for a surface owned by that request.
`ClientDataModel` only decodes metadata; the application decides which state to
update. This example assumes `cart` has already been selected and authorized:

```rust
use ag_ui_a2ui::{A2uiAuthor, ClientDataModel, Result, ValidatedSurface};
use serde_json::{json, Value};

fn apply_cart_input(
    author: &A2uiAuthor,
    cart: &ValidatedSurface,
    metadata: &Value,
) -> Result<ValidatedSurface> {
    let input = ClientDataModel::from_json(metadata)?;
    let Some(snapshot) = input.surfaces.get(cart.surface_id()) else {
        return Ok(cart.clone());
    };
    author.validate_edit(cart, &[json!({
        "version":input.version,
        "updateDataModel":{
            "surfaceId":cart.surface_id(),"path":"/","value":snapshot
        }
    })])
}
```

Pass the returned surface to `author.edit` so the next generation sees the
renderer input. A root update replaces the full model; the application must
resolve stale requests and concurrent edits before choosing a snapshot.

## Send through AG-UI

```rust
use ag_ui::server::{AgentState, Error, Result, RunContext};
use ag_ui_a2ui::{ValidatedSurface, adapters::agui::SurfacePainter};

fn send<S: AgentState>(ctx: &mut RunContext<S>, surface: &ValidatedSurface) -> Result<()> {
    let mut painter = SurfacePainter::from_history(ctx.messages()).map_err(Error::agent)?;
    painter.paint(ctx, surface.operations())?;
    Ok(())
}
```

This emits a `render_a2ui` tool call with the complete surface, an
`a2ui_operations` result for the batch, and the
`a2ui-surface` activity a frontend paints from. A successful
return confirms local event enqueue, not renderer receipt or application. Do
not automatically resend a failed batch: earlier events may have been sent.
This API validates the whole generated batch before sending; it does not combine
partial rendering with generation retries.

## Expose a template as a tool

A developer can fix the layout and expose a tool whose arguments supply its content.
This example uses the `author` feature but makes no model call. `validate_create()`
checks code-generated messages against the official schema and returns a `ValidatedSurface`.

```rust
use ag_ui_a2ui::{A2uiAuthor, A2uiVersion, AgentMessage, Component, Result, ValidatedSurface};
use ag_ui_a2ui::constants::OFFICIAL_BASIC_CATALOG_ID;
use serde_json::json;

fn summary_card(surface_id: &str, title: &str, description: &str) -> Result<ValidatedSurface> {
    let author = A2uiAuthor::basic(A2uiVersion::V0_9_1)?;
    let messages = [
        AgentMessage::create_surface(surface_id, OFFICIAL_BASIC_CATALOG_ID),
        AgentMessage::update_components(surface_id, vec![
            Component::new("root", "Card").with("child", json!("body")),
            Component::new("body", "Column").with("children", json!(["title", "description"])),
            Component::new("title", "Text").with("text", json!({"path":"/title"})),
            Component::new("description", "Text").with("text", json!({"path":"/description"})),
        ]),
        AgentMessage::update_data_model(surface_id, "/", json!({
            "title": title, "description": description,
        })),
    ].into_iter()
        .map(|message| serde_json::to_value(message.with_version(A2uiVersion::V0_9_1)))
        .collect::<serde_json::Result<Vec<_>>>()?;
    author.validate_create(surface_id, &messages)
}

let card = summary_card("weather-1", "서울 날씨", "맑음 · 21도")?;
assert_eq!(card.data_model().to_json()?["title"], "서울 날씨");
# Ok::<(), ag_ui_a2ui::Error>(())
```

An agent application can connect this function behind tools such as:

```text
weather_get("Seoul")
  → Clear, 21°C

a2ui_get("summaryCard")
  → The template requires title and description

a2ui_render(template="summaryCard", data={title:"Seoul weather", description:"Clear, 21°C"})
  → summary_card(...) → validated A2UI → renderer
```

`a2ui_search`, `a2ui_get` and `a2ui_render` are application-defined example names,
not standard A2UI tools. Search helps when there are many templates; get provides
instructions and input schemas. A small catalog can expose only a direct tool such
as `a2ui_summarycard(title, description)`. Returning a tool description does not
register a new callable tool automatically.

Here summaryCard is an application template composed from Card, Column and Text.
Sending a new `SummaryCard` component type would require its own catalog definition
and renderer implementation. An AG-UI server can send the returned card through
`painter.paint(ctx, card.operations())?`. For the same existing surface, use `validate_edit()` and
updates rather than creating it again. The application owns template selection and
data retrieval; the SDK constructs, validates and transports protocol messages.

## Build operations and read history

Build messages with `AgentMessage`. For AG-UI, `SurfaceSpec` and `assemble_ops`
turn a surface specification into operations. Use `adapters::agui::surfaces_in`
or `latest_surface` to read a conversation. Both return a `Result` so a malformed
history can be reported. `SurfacePainter::from_history` uses the same history.

Use the [agent layer](/ag-ui-rust/v1.0.0/a2ui/agent/) for a model that works across surfaces
or a framework that processes replies as they stream.

## Catalog negotiation

```rust
use ag_ui_a2ui::A2uiVersion;
use ag_ui_a2ui::agent::{CatalogConfig, CatalogDocument, RendererCapabilities, resolve_catalogs};
use serde_json::json;

let cards = CatalogDocument::load(
    json!({"catalogId":"example.com:cards","components":{}}),
    None,
    Some(A2uiVersion::V0_9),
).unwrap();
let renderer = RendererCapabilities::from_json(&json!({
    "v0.9":{"supportedCatalogIds":["example.com:cards"]}
})).unwrap();
let active = resolve_catalogs(&[CatalogConfig::new(cards)], Some(&renderer), false).unwrap();
assert_eq!(active[0].catalog_id(), "example.com:cards");
```

`RendererCapabilities::from_json` reads the capability document. Inline catalogs are
used as complete documents when the generator accepts them. An already active
ID is kept instead of adopting a second catalog under that ID. A request with
capabilities must activate at least one catalog. See [version and catalog selection](/ag-ui-rust/v1.0.0/a2ui/versions/)
for metadata keys and transport-specific IDs.
