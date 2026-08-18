---
title: Overview
description: What A2UI is, why it is a separate protocol from AG-UI, how a surface rides an AG-UI run, and which revision of the spec this crate speaks.
---

[A2UI](https://a2ui.org) is a declarative, agent-driven UI protocol. An agent streams JSON
describing a *surface* — a flat list of components and the data they bind to — and a renderer
draws it. `ag-ui-a2ui` is the **agent half** of that exchange.

Nothing in this crate draws pixels, lays out a tree, or evaluates a UI at runtime. It builds
A2UI, validates it, and wraps it for transport. Rendering is a genuinely different program,
with a widget toolkit, an event loop, and a reactive data model, and it lives on the other end
of the wire.

```rust
use ag_ui_a2ui::{Catalog, Component, Validator};
use serde_json::json;

let catalog = Catalog::basic();
let components = vec![
    Component::new("root", "Column").with("children", json!(["title", "count"])),
    Component::new("title", "Text").with("text", json!("Your cart")),
    Component::new("count", "Text").with("text", json!({"path": "/items"})),
];

let report = Validator::new(&catalog).validate_surface(&components, Some(&json!({"items": 2})));
assert!(report.is_valid());
```

## A2UI is a different protocol

A2UI is not part of AG-UI. It has its own specification, its own version number, and its own
toolkits in other languages. `ag-ui-a2ui` ships in this workspace because an AG-UI agent that
wants to put a form in front of a user needs it, not because AG-UI defines any of it — and
that separation is enforced rather than merely stated. Nothing below `ag_ui_a2ui::agui` knows
what AG-UI is.

Two Cargo features draw the line:

| Feature | Default | What it is |
| --- | --- | --- |
| `toolkit` | on | Agent-side authoring: operation builders, catalog negotiation, prompt assembly, stream parsing, the recovery loop. |
| `ag-ui` | on | Interop with `ag-ui-core`: history entries from AG-UI messages, toolkit tool definitions as offerable tools. Implies `toolkit`. |

Turn `ag-ui` off and the dependency on `ag-ui-core` goes with it:

```toml
[dependencies.ag-ui-a2ui]
git = "https://github.com/KimSoungRyoul/ag-ui-rust"
default-features = false
features = ["toolkit"]
```

What is left is a crate you can drive over A2A or MCP instead. The envelope this crate
produces is a plain JSON string, and `ag_ui_a2ui::constants::MIME_TYPE` is
`application/a2ui+json` for the transports that want a media type. See
[Feature flags](/ag-ui-rust/reference/features/) for the full matrix.

## The component model is a flat list

Components are sent as a flat adjacency list. Parent and child are linked by **id reference**,
never by nesting: a `Card` names its child by id, a `Column` holds an array of ids. The
renderer stores every component in a map and rebuilds the tree at render time.

That indirection is what makes the protocol streamable. The agent can define components in any
order, and the renderer can start painting as soon as the component with id `root` arrives.
The specification fixes that id — one component in one of the component lists must have
`id: "root"` — and it is the anchor for everything the [validator](/ag-ui-rust/a2ui/validation/)
checks.

Ten message envelopes carry all of it — six from the agent, four back:

| Direction | Payload keys |
| --- | --- |
| agent → renderer | `createSurface`, `updateComponents`, `updateDataModel`, `deleteSurface`, `callRendererFunction`, `agentFunctionResponse` |
| renderer → agent | `action`, `callAgentFunction`, `rendererFunctionResponse`, `error` |

Each message carries a `version` discriminator plus exactly one payload key. `ag_ui_a2ui::message`
is the port of all ten, and `AgentMessage` has a constructor for the four an authoring agent
sends most.

## How a surface rides an AG-UI run

A2UI says nothing about how messages reach the renderer, so every toolkit had to agree on
something. What they agreed on is a single JSON object keyed by `a2ui_operations`, carrying an
array of operations. The frontend sniffs for exactly that key to decide whether a payload is
A2UI at all.

```rust
use ag_ui_a2ui::toolkit::ops::{Intent, SurfaceSpec, assemble_ops};
use ag_ui_a2ui::{Component, wrap_as_operations_envelope};
use serde_json::json;

let spec = SurfaceSpec::new("cart")
    .with_components(vec![Component::new("root", "Text").with("text", json!("Your cart"))]);

let envelope = wrap_as_operations_envelope(&assemble_ops(Intent::Create, &spec)).unwrap();
assert!(envelope.starts_with(r#"{"a2ui_operations":["#));
```

The envelope is a JSON **string**, which is what lets it sit in an AG-UI tool result, an A2A
data part, or an MCP tool result without further wrapping. Over AG-UI, the carrier is a tool
call named `render_a2ui`, whose result is the envelope:

```rust
use ag_ui_a2ui::constants::RENDER_A2UI_TOOL_NAME;
use ag_ui_a2ui::toolkit::ops::{Intent, SurfaceSpec, assemble_ops};
use ag_ui_a2ui::{Component, wrap_as_operations_envelope};
use ag_ui_core::RunOutcome;
use ag_ui_server::{Agent, Error, Result, RunContext};
use serde_json::json;

struct Merchant;

impl Agent for Merchant {
    type State = ();

    async fn run(&self, ctx: &mut RunContext<()>) -> Result<RunOutcome> {
        let spec = SurfaceSpec::new("cart").with_components(vec![
            Component::new("root", "Text").with("text", json!("Your cart")),
        ]);
        let envelope = wrap_as_operations_envelope(&assemble_ops(Intent::Create, &spec))
            .map_err(Error::agent)?;

        let mut call = ctx.tool_call(RENDER_A2UI_TOOL_NAME)?;
        call.args_json(&json!({ "surfaceId": "cart" }))?;
        call.result(envelope)?;

        ctx.say("Here is your cart.")?;
        Ok(RunOutcome::Success)
    }
}
```

`render_a2ui` is a call the agent answers itself. No client ever offered it, because there is
nothing for a client to execute — the frontend draws the result rather than running the tool.
This is why `ag-ui-server` treats `RunAgentInput.tools` as a capability list rather than an
allow-list: emitting a call for a name absent from that list is a well-formed stream, and the
ordering verifier says nothing about it. What the protocol constrains is ordering, and that is
what gets checked.

`e2e/tests/a2ui_surface.rs` is the test that keeps this honest: an agent builds a surface with
the toolkit, ships it as a tool result, a real `ag-ui-client` receives it over a real port, and
the operations that come out the other end are asserted to be the ones that went in and to
still validate against the catalog they were authored for.

:::note[A failure is not an empty surface]
When generation fails, `wrap_error_envelope` produces a payload with an `error` key and
**no** `a2ui_operations` key. That is deliberate: the key is the content sniff, so an envelope
carrying it with an empty list would leave a failed generation indistinguishable from a
rendered one — including to the history scan that later replays a thread to find out what the
user is looking at.
:::

## This crate speaks v0.9

Every message is stamped `"version": "v0.9"`.

The A2UI specification itself has moved on to v1.0. The shipping toolkits have not: TypeScript,
.NET, and Python all still stamp `v0.9` on the wire, and .NET's constants file marks those
values a cross-language wire contract that must not diverge. Implementing v1.0 wire values
today would mean interoperating with none of them, so this crate pins to what the ecosystem
actually speaks. v1.0 goes behind a feature when the toolkits move.

The pin is not decorative. `Validator` reports a message declaring any other version as
`invalid_value`, and the vendored conformance suite's v0.8 cases are skipped rather than
adapted — 63 of the 70 skips are exactly that.

## The crate's shape

| Module | What is in it |
| --- | --- |
| [`message`](/ag-ui-rust/api/ag_ui_a2ui/message/index.html) | The ten protocol envelopes, `Component`, `ChildList`, and data-model update semantics. |
| [`catalog`](/ag-ui-rust/api/ag_ui_a2ui/catalog/index.html) | What a surface may contain. `Catalog::basic()` is the standard 18-component catalog; `Catalog::from_schema` parses a custom one. |
| [`validate`](/ag-ui-rust/api/ag_ui_a2ui/validate/index.html) | The semantic checks JSON Schema cannot express, plus the envelope and property-type checks a generating model gets wrong. |
| [`binding`](/ag-ui-rust/api/ag_ui_a2ui/binding/index.html) | JSON Pointer resolution, template scopes, and the `formatString` interpolation grammar. |
| [`constants`](/ag-ui-rust/api/ag_ui_a2ui/constants/index.html) | The cross-language wire values: the envelope key, the protocol version, the two tool names. |
| [`toolkit`](/ag-ui-rust/api/ag_ui_a2ui/toolkit/index.html) *(feature)* | Everything between "the user asked for a UI" and "valid A2UI is on the wire". |
| [`agui`](/ag-ui-rust/api/ag_ui_a2ui/agui/index.html) *(feature)* | The AG-UI glue, and nothing else in the crate knows AG-UI exists. |

Two pages go deeper: [Authoring surfaces](/ag-ui-rust/a2ui/authoring/) for the toolkit, and
[Validation](/ag-ui-rust/a2ui/validation/) for what gets checked and what the conformance suite
says about it.
