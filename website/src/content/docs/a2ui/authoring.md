---
title: Authoring surfaces
description: The toolkit feature — building operations, wrapping them for transport, prompting a model, parsing what comes back, and the validate-and-retry loop.
---

The `toolkit` feature is everything between "the user asked for a UI" and "valid A2UI is on
the wire". It is on by default, and it is where an authoring agent spends its time.

There is nothing here you are obliged to use as a whole. `assemble_ops` and
`wrap_as_operations_envelope` are enough for an agent that knows exactly what surface it wants
to draw; the prompt, parser, and recovery pieces exist for the agents that ask a model to
design one.

| Module | Job |
| --- | --- |
| [`negotiate`](/ag-ui-rust/api/ag_ui_a2ui/toolkit/negotiate/index.html) | Agree with the renderer on which catalog the surface speaks. |
| [`ops`](/ag-ui-rust/api/ag_ui_a2ui/toolkit/ops/index.html) | Build the operation stream, skipping `createSurface` on update. |
| [`envelope`](/ag-ui-rust/api/ag_ui_a2ui/toolkit/envelope/index.html) | Wrap operations for transport, or report a failure. |
| [`prompt`](/ag-ui-rust/api/ag_ui_a2ui/toolkit/prompt/index.html) | Assemble the generating model's prompt from catalog, context, and current surface. |
| [`parser`](/ag-ui-rust/api/ag_ui_a2ui/toolkit/parser/index.html) | Pull A2UI blocks back out of the model's response. |
| [`streaming`](/ag-ui-rust/api/ag_ui_a2ui/toolkit/streaming/index.html) | Do the same incrementally, so a surface renders while it is still being generated. |
| [`history`](/ag-ui-rust/api/ag_ui_a2ui/toolkit/history/index.html) | Recover a previously rendered surface so it can be edited. |
| [`recovery`](/ag-ui-rust/api/ag_ui_a2ui/toolkit/recovery/index.html) | Validate, feed the errors back, retry — up to three times. |
| [`schema`](/ag-ui-rust/api/ag_ui_a2ui/toolkit/schema/index.html) | Hold the schema documents; prune them to what the model needs. |
| [`tools`](/ag-ui-rust/api/ag_ui_a2ui/toolkit/tools/index.html) | The `generate_a2ui` and `render_a2ui` tool definitions. |

## Building the operation stream

A surface is described by a `SurfaceSpec`, and `assemble_ops` turns it into operations in the
order a renderer expects: create the surface, define its components, then supply the data.

```rust
use ag_ui_a2ui::Component;
use ag_ui_a2ui::toolkit::ops::{Intent, SurfaceSpec, assemble_ops};
use serde_json::json;

let spec = SurfaceSpec::new("cart")
    .with_components(vec![
        Component::new("root", "Column").with("children", json!(["heading", "items"])),
        Component::new("heading", "Text")
            .with("text", json!({"path": "/title"}))
            .with("variant", json!("h2")),
        Component::new("items", "List")
            .with("children", json!({"componentId": "row", "path": "/items"})),
        Component::new("row", "Text").with(
            "text",
            json!({"call": "formatString", "args": {"value": "${@index(offset: 1)}. ${name}"}}),
        ),
    ])
    .with_data_model(json!({
        "title": "Your cart",
        "items": [{"name": "Espresso"}, {"name": "Croissant"}],
    }));

// createSurface, updateComponents, updateDataModel.
assert_eq!(assemble_ops(Intent::Create, &spec).len(), 3);
// The same, minus createSurface.
assert_eq!(assemble_ops(Intent::Update, &spec).len(), 2);
```

Three things in that spec are worth naming.

`items` uses the **template** form of a child list — `{"componentId": ..., "path": ...}` rather
than an array of ids — which instantiates `row` once per element of `/items`. That is also what
opens a collection scope, so `${name}` inside `row` resolves relative to the current element
rather than to the root of the data model.

Defaults come from the wire constants: a spec with no explicit catalog uses
`BASIC_CATALOG_ID`, and a spec built with `SurfaceSpec::default()` targets the surface id
`dynamic-surface`.

And `Intent::Update` is not a cosmetic distinction. `createSurface` allocates a `surfaceId` and
fixes its catalog for the surface's lifetime; sending it again for a surface the renderer
already holds is an error per spec. `Intent::from_wire` returns `None` for anything it does not
recognise rather than defaulting, because guessing "update" wrong re-creates a live surface.

:::caution[Replacing the whole data model discards user input]
`SurfaceSpec::data_path` defaults to `/`, which replaces the entire data model. The renderer's
two-way bindings write user input straight into that model, so when you are updating a live
surface, point at the narrower path you actually mean.
:::

```rust
use ag_ui_a2ui::message::AgentPayload;
use ag_ui_a2ui::toolkit::ops::{Intent, SurfaceSpec, assemble_ops};
use serde_json::json;

let spec = SurfaceSpec::new("cart")
    .with_data_model(json!("Ada"))
    .with_data_path("/user/name");

let ops = assemble_ops(Intent::Update, &spec);
let AgentPayload::UpdateDataModel(payload) = &ops[0].payload else {
    panic!("expected updateDataModel");
};
assert_eq!(payload.path, "/user/name");
```

## Wrapping for transport

`wrap_as_operations_envelope` produces the `{"a2ui_operations": [...]}` object as a JSON
string. That string is what fits, unchanged, in an AG-UI tool result, an A2A data part, or an
MCP tool result.

```rust
use ag_ui_a2ui::toolkit::envelope::{is_operations_envelope, wrap_as_operations_envelope};
use ag_ui_a2ui::{AgentMessage, Component};
use serde_json::{Value, json};

let json = wrap_as_operations_envelope(&[
    AgentMessage::create_surface("cart", "basic"),
    AgentMessage::update_components(
        "cart",
        vec![Component::new("root", "Text").with("text", json!("Your cart"))],
    ),
])
.unwrap();

let value: Value = serde_json::from_str(&json).unwrap();
assert!(is_operations_envelope(&value));
assert_eq!(value["a2ui_operations"][0]["version"], "v0.9");
```

`operations_envelope` returns the same thing as a `Value` for callers embedding it in a larger
payload, and `unwrap_operations_envelope` reads it back.

When generation fails, send `wrap_error_envelope` instead — and note what it does *not* carry:

```rust
use ag_ui_a2ui::toolkit::envelope::{is_operations_envelope, wrap_error_envelope};
use ag_ui_a2ui::validate::{ErrorCode, ValidationError};
use serde_json::Value;

let errors = vec![ValidationError::new(
    ErrorCode::NoRoot,
    "components",
    "No component has id 'root'.",
)];

let json = wrap_error_envelope("cart", "could not build the surface", &errors).unwrap();
let value: Value = serde_json::from_str(&json).unwrap();

assert_eq!(value["error"], "could not build the surface");
assert_eq!(value["code"], "VALIDATION_FAILED");
assert_eq!(value["details"][0]["code"], "no_root");
// The absent key is the point: a failure must not answer the frontend's sniff.
assert!(!is_operations_envelope(&value));
```

An empty operations envelope would be worse than useless. Consumers key on the
`a2ui_operations` key to decide whether a payload is A2UI, so a failure carrying it would clear
pending state on the frontend *and* be replayed later, by the history scan, as a surface that
was never on screen.

## Prompting a model

`PromptSpec` collects what the generating model needs and `build_subagent_prompt` renders it:
the role, the request, the workflow rules, the catalog, optional few-shot examples, the
conversation so far, the surface currently on screen, and the response format.

By default the prompt carries `catalog.render_summary()` — a compact description of the
component types and their properties. Attach a `SchemaBundle` with `PromptSpec::with_schemas`
and it carries the exact JSON Schema documents instead: precise, and far more tokens. Prune the
bundle first if the model only needs part of the catalog.

The default rules come from `GENERATION_GUIDELINES`, and they are tuned to what this crate's
validator rejects — every component needs a unique `id`, one of them must be `root`, no
reference loops, bindings must point at data you also send, relative paths only inside a list
template. An application with its own house style can replace them via
`PromptSpec::workflow_rules`.

## Editing what is already on screen

An update is only useful if the agent knows what it is updating, and the agent stores nothing
between runs: the surface it rendered went out over the transport and the renderer holds it.
What is still available is the conversation. `find_prior_surface_in` replays the A2UI
operations already in an AG-UI thread and reports the surface they built.

```rust
use ag_ui_a2ui::toolkit::ops::Intent;
use ag_ui_a2ui::toolkit::prompt::{PromptSpec, build_subagent_prompt};
use ag_ui_a2ui::{
    AgentMessage, Catalog, Component, find_prior_surface_in, wrap_as_operations_envelope,
};
use ag_ui::Message;
use serde_json::json;

let rendered = wrap_as_operations_envelope(&[
    AgentMessage::create_surface("cart", "basic"),
    AgentMessage::update_components(
        "cart",
        vec![Component::new("root", "Text").with("text", json!("Your cart"))],
    ),
])
.unwrap();

let thread = [
    Message::user("m-1", "show me my cart"),
    Message::tool("m-2", "call-1", rendered),
    Message::user("m-3", "add a checkout button"),
];

let prior = find_prior_surface_in(&thread).expect("the thread rendered a surface");
assert_eq!(prior.surface_id, "cart");
assert_eq!(prior.catalog_id.as_deref(), Some("basic"));

let catalog = Catalog::basic();
let spec = PromptSpec::new("You generate UI surfaces.", "add a checkout button", &catalog)
    .updating(&prior);

// `updating` switches the intent and targets the existing surface.
assert_eq!(spec.intent, Intent::Update);
assert!(build_subagent_prompt(&spec).contains("Your cart"));
```

The scan recognises two encodings, because both occur in practice: the `a2ui_operations`
transport envelope, and raw `<a2ui-json>` blocks in an assistant turn. It walks newest-first to
pick the surface, then forwards to replay it, so an update targets whatever the user is
actually looking at. `find_prior_surface_by_id` restricts it to one `surfaceId` when several
are live.

Without the `ag-ui` feature the same scan is available as
`toolkit::history::find_prior_surface`, over the crate's own `HistoryMessage` type. The AG-UI
version is a `From` impl and a one-line wrapper over it.

## Parsing what the model returns

When A2UI is produced by prompting rather than by structured output, the model returns prose
with the A2UI fenced in `<a2ui-json>` tags. `parse_response` splits that into ordered parts.

```rust
use ag_ui_a2ui::toolkit::parser::parse_response;

let response = r#"Here is your cart. <a2ui-json>[
    {"version": "v0.9", "createSurface": {"surfaceId": "cart", "catalogId": "basic"}}
]</a2ui-json>"#;

let parts = parse_response(response).unwrap();
assert_eq!(parts[0].text, "Here is your cart.");
assert!(parts[0].is_final);
assert_eq!(parts[0].a2ui.as_ref().unwrap().len(), 1);
```

This is a scanner rather than a string split, and it has to be: a closing tag can legitimately
appear inside a JSON string literal — a `Text` component whose content mentions
`</a2ui-json>` is valid A2UI — so the scanner tracks string state and escapes.

`parse_and_fix` applies two repairs, and only after a straight parse has failed: normalising
smart quotes, and dropping trailing commas. Both are safe because neither can change the
meaning of valid JSON. It also wraps a lone object in an array, since A2UI payloads are lists
of messages.

## The recovery loop

Prompted generation is not schema-constrained, so a model will sometimes return a surface that
does not hold together. That is recoverable: the validator says exactly what is wrong, in
sentences written to be handed back to a model. `generate_with_recovery` runs
validate → explain → retry, up to `MAX_A2UI_ATTEMPTS`, which is 3.

```rust
use ag_ui_a2ui::catalog::Catalog;
use ag_ui_a2ui::toolkit::recovery::{RecoveryOptions, RecoveryStatus, generate_with_recovery};

fn response(components: &str) -> String {
    format!(
        r#"<a2ui-json>[
             {{"version":"v0.9","createSurface":{{"surfaceId":"cart","catalogId":"basic"}}}},
             {{"version":"v0.9","updateComponents":{{"surfaceId":"cart","components":{components}}}}}
           ]</a2ui-json>"#
    )
}

let catalog = Catalog::basic();
let mut statuses = Vec::new();
let mut attempt = 0;

let surface = generate_with_recovery(
    "build a cart summary",
    &catalog,
    &RecoveryOptions::default(),
    |prompt, _n| {
        attempt += 1;
        Ok(if attempt == 1 {
            assert!(!prompt.contains("Correction required"));
            // References a component it never defined.
            response(r#"[{"id":"root","component":"Card","child":"missing"}]"#)
        } else {
            // The retry prompt now carries the validator's complaint.
            assert!(prompt.contains("unresolved_child"));
            response(r#"[{"id":"root","component":"Text","text":"Your cart"}]"#)
        })
    },
    |activity| statuses.push(activity.status),
)
.unwrap();

assert_eq!(surface.attempts, 2);
assert_eq!(surface.components.len(), 1);
assert_eq!(
    statuses,
    vec![
        RecoveryStatus::Started,
        RecoveryStatus::Retrying,
        RecoveryStatus::Started,
        RecoveryStatus::Succeeded,
    ]
);
```

The loop is **synchronous** and takes the model as a closure, so it imposes no async runtime:
wrap a blocking call directly, or drive an async client with whatever executor the host already
uses. Every step is reported through `on_activity` under the `a2ui_recovery` activity type —
`RecoveryActivity::activity_type` is that constant, so a caller can route on it and show
progress rather than a stall. What it does with the report is its own decision; the toolkit
does not emit anything itself.

When every attempt fails, the error is `Error::RecoveryExhausted`, carrying the attempt count
and the final error list. `RecoveryOptions::for_update()` swaps in the relaxed
[validation contract](/ag-ui-rust/a2ui/validation/) for editing an existing surface.

## Streaming instead

The recovery loop waits for the whole generation before anything reaches the user, which buys
a validate-and-retry safety net at the cost of latency. `StreamParser` makes the other trade:
feed it chunks and it emits renderable A2UI as soon as enough of the tree has arrived to draw
something.

```rust
use ag_ui_a2ui::catalog::Catalog;
use ag_ui_a2ui::toolkit::streaming::StreamParser;

let mut parser = StreamParser::new(Catalog::basic());

// Conversational text comes out immediately.
let parts = parser.process_chunk("Building that now. <a2ui-json>[").unwrap();
assert_eq!(parts[0].text, "Building that now. ");

// A message is emitted the moment it closes, mid-array.
let parts = parser
    .process_chunk(r#"{"version":"v0.9","createSurface":{"surfaceId":"cart","catalogId":"basic"}},"#)
    .unwrap();
assert_eq!(parts[0].a2ui.as_ref().unwrap().len(), 1);
assert_eq!(parser.surface_id(), Some("cart"));
```

Four mechanisms do the work, and they are why this is not a JSON parser fed one byte at a time:

- **Healing cut tokens.** A chunk boundary can land inside a string. The parser closes open
  braces and brackets to make the fragment parseable, but it only closes an open *string* for a
  key on the cuttable list — `text`, `label`, `hint`, and four others in
  `DEFAULT_CUTTABLE_KEYS`. Closing `"id"` or `"path"` early would invent an identifier or a
  binding the model never wrote, so those fragments wait for the next chunk instead.
- **Placeholder synthesis.** A parent usually arrives before its children, so its child
  references are rewritten to `loading_<id>` and a stand-in component is emitted alongside. The
  renderer lays out the tree immediately and swaps in the real component when it lands.
- **Reachability filtering.** Only components reachable from `root` are emitted. One that
  arrives before its parent is cached rather than sent — it would have nowhere to attach — and
  is re-sent once the path from the root exists.
- **Validation as a filter, not a gate.** Partial fragments are validated and silently dropped
  if they do not hold up yet. Only failures no further input can fix — a reference loop, a
  message matching no envelope — are errors.

A parser instance carries the state for one generation: which surfaces exist, which components
have been seen and emitted, the data model so far. Create a new one per generation.

Pick the batch loop when correctness matters more than latency, and the stream parser when the
user is watching.

## Catalog negotiation

`createSurface` fixes a surface's catalog for its lifetime, so the choice has to be made before
the model is prompted. `select_catalog` does that negotiation: the renderer's preference order
wins, because the renderer is the one that has to draw the result.

```rust
use ag_ui_a2ui::constants::BASIC_CATALOG_ID;
use ag_ui_a2ui::toolkit::negotiate::{ClientCapabilities, select_catalog};
use serde_json::json;

let known = vec![
    json!({"catalogId": "https://example.com/design-system.json", "components": {}}),
    json!({"catalogId": BASIC_CATALOG_ID, "components": {}}),
];
let renderer = ClientCapabilities::supporting([BASIC_CATALOG_ID]);

let chosen = select_catalog(&known, &renderer, false).unwrap();
assert_eq!(chosen.catalog_id, BASIC_CATALOG_ID);
```

A renderer may also ship inline catalog documents for components only it has; pass
`accepts_inline` and their components are merged into the selection, keeping the selected
catalog's `catalogId` — that id is what the two sides negotiated on and what goes on the wire.
`CatalogRegistry` holds the catalogs an agent knows, by application-facing name, and reports
their wire ids.

## The two tool definitions

The toolkit exposes two tools, and they sit at different levels:

- `generate_a2ui` is **planner-facing**. The orchestrating model calls it to say "render this,
  as a new surface or as an edit to that one". Its arguments are intent and description, not
  components.
- `render_a2ui` is the **inner structured-output** tool. The generating model calls it to emit
  the actual surface: a flat component list and a data model.

Keeping them apart is what lets the planner stay out of the component catalog — it describes
what it wants, and the inner call produces it.

```rust
use ag_ui_a2ui::Catalog;
use ag_ui_a2ui::toolkit::tools::{generate_a2ui_tool, render_a2ui_tool};
use ag_ui::Tool;

let catalog = Catalog::basic();
let tools: Vec<Tool> = vec![
    generate_a2ui_tool().into(),
    render_a2ui_tool(Some(&catalog)).into(),
];

assert_eq!(tools[0].name, "generate_a2ui");
assert_eq!(tools[1].name, "render_a2ui");
assert_eq!(tools[1].parameters["type"], "object");
```

`ToolDefinition` is provider-neutral: `name`, `description`, and `parameters` as a JSON Schema
object. `to_anthropic_value()` renders the Messages API shape, where the schema key is
`input_schema`; the `From<ToolDefinition> for ag_ui::Tool` impl above is the AG-UI shape,
and it needs the `ag-ui` feature.

Once a surface exists, [Validation](/ag-ui-rust/a2ui/validation/) is what decides whether it is
worth shipping.
