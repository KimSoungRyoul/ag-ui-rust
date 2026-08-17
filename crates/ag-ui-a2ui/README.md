# ag-ui-a2ui

[A2UI](https://a2ui.org) protocol types, semantic validator, and agent-side authoring
toolkit.

A2UI is a declarative, agent-driven UI protocol: an agent streams JSON describing a
surface, and a renderer draws it. This crate is the **agent half** of that exchange.

```toml
[dependencies]
ag-ui-a2ui = "0.1"
```

```rust
use ag_ui_a2ui::{catalog::Catalog, message::Component, validate::Validator};
use serde_json::json;

let catalog = Catalog::basic();
let components = vec![
    Component::new("root", "Card").with("child", json!("greeting")),
    Component::new("greeting", "Text").with("text", json!("Hello!")),
];

let report = Validator::new(&catalog).validate(&components);
assert!(report.is_valid());
```

## This crate does not render

Nothing here draws pixels, lays out a tree, or evaluates a UI at runtime. It produces A2UI,
validates it, and transports it. Rendering is the client's job, and it is a genuinely
different program — one with a widget toolkit, an event loop, and a reactive data model.
What this crate gives you instead:

- `message` — the ten protocol envelopes, in both directions.
- `catalog` — what a surface may contain, including the standard 18-component basic
  catalog.
- `validate` — the semantic checks JSON Schema cannot express: does every child reference
  resolve, is there a root, is the tree acyclic.
- `binding` — JSON Pointer resolution, template scopes, and the `formatString`
  interpolation grammar, so an agent can check its own bindings before shipping them.
- `toolkit` — building ops, negotiating a catalog, assembling prompts, parsing a model's
  output as it streams, recovering a surface from conversation history, and the
  validate-and-retry loop around a generating model.

## Version

Messages are stamped `v0.9`. The specification has moved on to v1.0, but every shipping
toolkit — TypeScript, .NET, Python — still speaks v0.9 on the wire, and .NET's constants
file marks these values a cross-language wire contract that must not diverge.
Interoperating with them matters more than tracking the newest revision.

## Conformance

The A2UI project publishes a language-agnostic conformance suite as YAML. It is vendored
under `tests/conformance/` and run as a normal test; the report prints what passed, what
was skipped, and why.

## Features

| Feature | Default | What it adds |
| --- | --- | --- |
| `toolkit` | yes | Agent-side authoring: op builders, prompt assembly, recovery loop. |

This crate depends on no transport, AG-UI included. Wrapping operations for the wire is
`toolkit::envelope`, which returns the `a2ui_operations` envelope as a JSON string that an
AG-UI message, an A2A data part or an MCP tool result can all carry as-is.

See the [repository](https://github.com/KimSoungRyoul/ag-ui-rust) for the design rationale.

## License

MIT
