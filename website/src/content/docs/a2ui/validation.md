---
title: Validation
description: What the semantic validator checks beyond a surface's shape, the error codes it reports, and what the vendored conformance suite says about it.
---

JSON Schema can say that `children` is an array of strings. It cannot say that every one of
those strings names a component that exists, that the tree has a root, or that `a → b → a` is a
loop the renderer will never finish drawing. That is what
[`ag_ui_a2ui::validate`](/ag-ui-rust/api/ag_ui_a2ui/validate/index.html) checks, and that is
what "semantic" means here.

```rust
use ag_ui_a2ui::validate::{ErrorCode, Validator};
use ag_ui_a2ui::{Catalog, Component};
use serde_json::json;

let report = Validator::new(&Catalog::basic()).validate(&[
    Component::new("root", "Card").with("child", json!("greeting")),
]);

assert_eq!(report.errors[0].code, ErrorCode::UnresolvedChild);
assert_eq!(report.errors[0].path, "components[0].child");
```

Every failure is a `ValidationError` carrying three things: a machine-readable `code`, a `path`
locating it in the components list, and a `message` written as a sentence a model can act on.
The validator collects *all* errors rather than stopping at the first, so one retry can fix
everything at once — which is the whole reason the
[recovery loop](/ag-ui-rust/a2ui/authoring/) works.

## Where a surface comes from decides how it is checked

[`Validator`](/ag-ui-rust/api/ag_ui_a2ui/validate/struct.Validator.html) has five entry points,
differing in what they are handed:

| Method | Input | Also checks |
| --- | --- | --- |
| `validate` | `&[Component]` | — |
| `validate_surface` | `&[Component]` plus a data model | Bindings resolve against real data. |
| `validate_json` | `&[Value]`, raw from a model | Missing `id` and missing `component`, which typed components cannot express. |
| `validate_messages` | `&[AgentMessage]` | The whole operation stream, folded. |
| `validate_json_messages` | `&[Value]`, raw from the wire | The message envelope, JSON nesting depth, function-call chain depth. |

The last two fold a stream: components from every `createSurface` and `updateComponents` are
collected together, `updateDataModel` operations are replayed to reconstruct the data model,
and the contract is chosen automatically — a stream with no `createSurface` is treated as an
incremental update.

The three checks in `validate_json_messages` exist only there because none of them survives
deserialization into typed messages. All three are properties of the document rather than of
any one component.

```rust
use ag_ui_a2ui::Catalog;
use ag_ui_a2ui::validate::{ErrorCode, Validator};
use serde_json::json;

let messages = vec![json!({
    "version": "v1.0",
    "updateComponents": {"surfaceId": "cart"}
})];

let report = Validator::new(&Catalog::basic()).validate_json_messages(&messages);
let codes: Vec<ErrorCode> = report.errors.iter().map(|e| e.code).collect();

// This crate speaks v0.9 and says so.
assert!(codes.contains(&ErrorCode::InvalidValue));
// `updateComponents` requires a `components` array.
assert!(codes.contains(&ErrorCode::MissingField));
```

Every other toolkit gets that envelope check from `server_to_client.json` through a JSON Schema
engine. This crate speaks exactly one protocol version, so the contract is a table transcribed
from the payload types instead — which is what lets a failure carry a locator into the message
the caller actually sent, rather than a path into a schema the caller never saw. The codes
match the ones the schema-driven toolkits report, because callers route on them.

## The full contract and the relaxed one

A payload that creates a surface is held to the whole contract: a `root` must exist and every
child reference must resolve within the payload. An incremental `updateComponents` is not —
its components may legitimately reference ids the renderer already holds, and it need not
include the root.

```rust
use ag_ui_a2ui::validate::{ErrorCode, Validator};
use ag_ui_a2ui::{Catalog, Component};
use serde_json::json;

let catalog = Catalog::basic();
let patch = [Component::new("heading", "Text").with("text", json!("Updated"))];

// The full contract wants a root.
let strict = Validator::new(&catalog).validate(&patch);
assert!(strict.errors.iter().any(|e| e.code == ErrorCode::NoRoot));

// An incremental update does not: the renderer already holds the rest of the tree.
assert!(Validator::incremental(&catalog).validate(&patch).is_valid());
```

`ValidateOptions::incremental_update()` relaxes exactly those two rules, `require_root` and
`allow_dangling_children`. Duplicate ids and cycles still fail, because those are broken either
way.

Between the two presets, every check is an individual switch on
[`ValidateOptions`](/ag-ui-rust/api/ag_ui_a2ui/validate/struct.ValidateOptions.html), and
`Validator::with_options` takes them:

| Option | Default | What it governs |
| --- | --- | --- |
| `root_id` | `"root"` | The id the tree root must have. |
| `require_root` | `true` | Whether a component with that id must be present. |
| `allow_dangling_children` | `false` | Whether child references may point outside this payload. |
| `check_component_types` | `true` | Whether component types must exist in the catalog. Turned off automatically when the catalog defines nothing, since that means none was supplied. |
| `check_required_props` | `true` | Whether catalog-required properties are enforced. |
| `check_prop_types` | `true` | Whether values match the JSON type the catalog declares. |
| `check_envelope` | `true` | Whether messages satisfy the v0.9 wire contract. Raw-message entry points only. |
| `check_bindings` | `true` | Whether bindings resolve, and whether relative paths sit inside a list template. |
| `check_binding_syntax` | `true` | Whether absolute paths are syntactically valid JSON Pointers. |
| `max_depth` | `50` | Deepest nesting, for both the component graph and the raw JSON. |
| `max_function_call_depth` | `5` | Deepest chain of nested function calls. |

`check_binding_syntax` is separate from `check_bindings` because it needs no data model and
cannot produce a false positive: a malformed escape can never resolve, whatever the data turns
out to be.

## The error codes

Fourteen [`ErrorCode`](/ag-ui-rust/api/ag_ui_a2ui/validate/enum.ErrorCode.html) variants, and
the set is deliberately closed. Callers route on these codes — a recovery loop, a renderer's
error channel — so adding one is a breaking change.

| Code | Reported when |
| --- | --- |
| `empty_components` | The payload declares a surface but carries no components. |
| `missing_id` | A component has no usable `id`. |
| `missing_component_type` | A component has no usable `component` type name. |
| `duplicate_id` | Two components share an `id`. |
| `no_root` | No component has the root id, so the renderer has nothing to draw from. |
| `unknown_component` | A component's type is not defined by the surface's catalog. |
| `missing_required_prop` | A property the catalog marks required is missing. |
| `missing_field` | A field the *protocol* requires on a message envelope is missing. |
| `invalid_value` | A value has the right shape but is not one the protocol permits — a `version` naming a revision this crate does not speak. |
| `type_mismatch` | A value is of the wrong JSON type. |
| `unresolved_child` | A child reference names a component id that does not exist. |
| `child_cycle` | Following child references leads back to where it started. |
| `unresolved_binding` | A data binding cannot resolve against the surface's data model. |
| `max_depth_exceeded` | Nesting runs deeper than the configured maximum. |

`missing_field` and `missing_required_prop` look alike and are not: the first is fixed by the
wire format and holds whatever catalog is in play, the second is about a property some
*catalog* declares.

`max_depth_exceeded` covers three kinds of nesting — the component graph, the raw JSON, and
chained function calls — because all three are model-generated and none is bounded without a
cap.

## Unreachable components are a warning

A component that exists but cannot be reached from the root is reported in
[`ValidationReport::unreachable`](/ag-ui-rust/api/ag_ui_a2ui/validate/struct.ValidationReport.html),
not in `errors`:

```rust
use ag_ui_a2ui::{Catalog, Component, Validator};
use serde_json::json;

let report = Validator::new(&Catalog::basic()).validate(&[
    Component::new("root", "Text").with("text", json!("Hello")),
    Component::new("stray", "Text").with("text", json!("Nobody points at me")),
]);

assert!(report.is_valid());
assert_eq!(report.unreachable, vec!["stray".to_string()]);
```

The specification tells renderers to buffer components until their parent shows up, so an
unreachable component is usually a half-streamed tree rather than a broken one. It is still
worth telling a generating model about, so it is reported separately rather than dropped.

## Handing the result to a model

`ValidationReport::into_result` converts the report into `Error::Validation`, and the
`ValidationErrors` it carries renders one error per line — which is the format the retry prompt
wants.

```rust
use ag_ui_a2ui::{Catalog, Component, Error, Validator};
use serde_json::json;

let report = Validator::new(&Catalog::basic()).validate(&[
    Component::new("root", "Card").with("child", json!("missing")),
]);

let Err(Error::Validation { errors }) = report.into_result() else {
    panic!("this surface does not validate");
};

assert!(errors.to_string().starts_with("[unresolved_child] components[0].child:"));
```

## The catalog decides what "unknown" means

Validation is always against a
[`Catalog`](/ag-ui-rust/api/ag_ui_a2ui/catalog/struct.Catalog.html). `Catalog::basic()` is the
standard 18-component catalog, transcribed from the v0.9 `basic_catalog.json` and kept honest
by a test that parses the vendored specification document and compares. `Catalog::from_schema`
parses any A2UI catalog document, which is how a custom design system is described.

```rust
use ag_ui_a2ui::validate::{ErrorCode, Validator};
use ag_ui_a2ui::{Catalog, Component};
use serde_json::json;

let catalog = Catalog::from_schema(&json!({
    "catalogId": "https://example.com/design-system.json",
    "components": {
        "Chart": {
            "type": "object",
            "properties": {
                "columns": {"type": "integer"},
                "series": {"type": "array"}
            },
            "required": ["series"]
        }
    }
}))
.unwrap();

let report = Validator::new(&catalog).validate(&[
    Component::new("root", "Chart")
        .with("columns", json!("three"))
        .with("series", json!([1, 2])),
    Component::new("legend", "Sparkline"),
]);

let codes: Vec<ErrorCode> = report.errors.iter().map(|e| e.code).collect();
assert!(codes.contains(&ErrorCode::TypeMismatch));      // "three" is not an integer
assert!(codes.contains(&ErrorCode::UnknownComponent));  // no Sparkline in this catalog
```

Two things about that check are narrower than they may look.

**Only structural properties link components.** The specification requires a catalog to type
child references as `ComponentId` or `ChildList` rather than as bare strings, and that is
exactly how the validator decides which fields are links. A plain `"type": "string"` is treated
as static text — a URL, a label — and its value is never resolved as a component id.

**Property types are carried, not interpreted.** Each property keeps the JSON type its schema
pins it to, and that is the one constraint taken out of JSON Schema. `pattern`, `minimum`,
`additionalProperties` and the rest are left to whatever validates the document itself. A
property whose schema states no type, or several, is unconstrained and is never rejected — a
catalog read loosely must not turn into false failures. Values the renderer resolves, a
`{"path": …}` binding or a function call, are skipped entirely: their type on the wire says
nothing about the type they will have.

Composition constraints — `allowedParents` and `allowedChildren` — are checked by
`Catalog::composition_violations`, deliberately outside `validate`. The specification assigns
them their own renderer-side codes, `UNALLOWED_PARENT` and `UNALLOWED_CHILD`, distinct from the
structural ones. The basic catalog declares no composition constraints at all, so this only
matters for custom catalogs.

## Bindings

`check_bindings` resolves data bindings against the surface's data model, when one is supplied.
Three failures it reports, all as `unresolved_binding`: a path that does not exist in the data,
a template path that points at something other than an array, and a relative path on a
component that is not inside a list template — relative paths only mean anything inside a
collection scope.

`check_binding_syntax` is the half that needs no data: inside a JSON Pointer, `~` must be
written `~0` and `/` must be written `~1`, so a raw `~` in a key is invalid rather than merely
absent.

## Depth is policy, not safety

The component graph is walked iteratively, with an explicit worklist, in every case — cycle
detection, reachability, scope assignment. That is not a style preference. The graph is
model-generated and its depth is bounded by nothing, so a recursive walk would abort the
process rather than fail a request.

So `MAX_DEPTH` (50) and `MAX_FUNCTION_CALL_DEPTH` (5) are a *policy* about what a renderer will
draw, not what keeps this crate standing, and raising them is safe here. They match the limit
every other A2UI toolkit enforces, so a payload one of them accepts is accepted here.

## What the conformance suite says

The A2UI project publishes its conformance tests as language-agnostic YAML and asks every SDK
to run them: read the YAML, feed the inputs to that language's implementation, assert the
outputs. The suite is vendored under `crates/ag-ui-a2ui/tests/conformance/` at upstream commit
`44a420b6` and driven by `tests/conformance.rs`.

Running it:

```sh
cargo test -p ag-ui-a2ui --all-features -- --nocapture
```

```text
core/validator.yaml              cases  45   checks: 26 passed, 25 skipped, 0 failed
core/catalog.yaml                cases  24   checks: 23 passed,  1 skipped, 0 failed
core/accessibility.yaml          cases   4   checks:  0 passed,  4 skipped, 0 failed
agent/parser.yaml                cases  19   checks: 19 passed,  0 skipped, 0 failed
agent/inference_format.yaml      cases  19   checks: 17 passed,  2 skipped, 0 failed
agent/streaming_parser.yaml      cases  76   checks: 38 passed, 38 skipped, 0 failed

TOTAL: 123 passed, 70 skipped, 0 failed
```

A case with `steps` counts as one check per step, which is why the check totals exceed the case
counts in places.

Every skip carries a reason and is counted; nothing is silently ignored, and no case is counted
as passing unless this crate actually produced the expected outcome. The 70 break down as:

| Count | Reason |
| ---: | --- |
| 63 | **v0.8 wire format.** v0.8 nests component properties under the type name and uses different message names. This crate implements v0.9, where components are flat. |
| 4 | **Renderer accessibility.** Accessibility trees and axe-core rules belong to a renderer; this crate does not render. |
| 2 | **v0.8 schema bundle in a prompt.** Two `generate_prompt` cases ask for the v0.8 schema documents to be embedded; the other six prompt cases run. |
| 1 | **JSON Schema validation of an example file.** One case asks `load_examples` to validate each example against a caller-supplied schema — a whole JSON Schema engine, which is the one part of upstream's schema story this crate does not reproduce. |

Version gating is applied only where the version matters. `validate` and `process_chunk` are
wire-format operations and are gated; schema surgery — prune, render, load, relax strict
validation — works on the shape of the documents rather than on the protocol, so those cases
run for v0.8 too.

:::note[Two places the harness departs from upstream, on purpose]
`test_validate_orphaned_component_v09` expects an error for a component unreachable from the
root. This crate reports that as a warning, for the reason above, so the harness asserts the
condition is *detected* rather than that it is fatal. It is the only expectation mapped onto a
different severity.

For the `validate` cases the harness matches upstream's scope: component-type and
required-property checking are turned off, because upstream delegates those to JSON Schema in
the cases being compared, and binding resolution is turned off, because upstream's structural
validator does not look at bindings. Pointer syntax, envelope, and property-type checks stay
on, since upstream checks those too. Matching the scope is what makes the comparison mean
anything.
:::
