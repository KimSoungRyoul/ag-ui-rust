# Vendored A2UI conformance suite

The A2UI project ships its conformance tests as language-agnostic YAML and asks
every SDK to run them: read the YAML, feed the inputs to that language's
implementation, assert the outputs. These files are copied verbatim from
upstream and are driven by `crates/ag-ui-a2ui/tests/conformance.rs`.

- **Upstream:** <https://github.com/a2ui-project/a2ui>
- **Commit:** `44a420b67957fafc0b02d55a153fdaf72e32ffb5`
- **Path upstream:** `conformance/`
- **License:** Apache-2.0 (see the copyright headers in the YAML files)

Do not hand-edit these files. To update, re-copy from upstream at a newer commit
and change the SHA here and in `UPSTREAM_COMMIT` in the harness.

## What is vendored

| File | Cases | Executed here |
|---|---:|---|
| `core/validator.yaml` | 45 | 16 checks — the v0.9 structural cases |
| `core/catalog.yaml` | 24 | none |
| `core/accessibility.yaml` | 4 | none |
| `agent/parser.yaml` | 19 | 19 — all of them |
| `agent/inference_format.yaml` | 19 | none |
| `agent/streaming_parser.yaml` | 76 | none |
| `test_data/simplified_{catalog,common_types,s2c}_v09.json` | — | schema files two validator cases reference |

The rest of upstream `test_data/` is not vendored: it feeds the `load` and
`prune` cases, none of which this crate executes.

## Why cases are skipped

Every case the harness cannot execute is skipped **with a reason and a count**,
printed by the test. Nothing is silently ignored, and no case is counted as
passing unless this crate actually produced the expected outcome. Run

```
cargo test -p ag-ui-a2ui --no-default-features --features toolkit -- --nocapture
```

to see the report, and set `A2UI_CONFORMANCE_VERBOSE=1` to list every check by
name.

The skips fall into five groups:

1. **v0.8 wire format** (25 validator cases). v0.8 nests component properties
   under the type name (`component: {Text: {...}}`) and uses different message
   names. This crate implements v0.9, where components are flat.
2. **JSON Schema validation** (8 validator checks). Upstream validates envelopes
   and component properties with a JSON Schema engine, and those cases assert its
   error codes (`missing_field`, `type_mismatch`) or its messages (`123 is not of
   type 'string'`). This crate has no JSON Schema engine; it does the *semantic*
   checks a schema cannot express.
3. **Depth and pointer-syntax limits** (4 validator checks). Recursion depth caps
   and JSON Pointer escape validation are not implemented.
4. **Renderer behaviour** (`core/accessibility.yaml`, `agent/streaming_parser.yaml`,
   80 cases). Accessibility trees and incremental chunk parsing belong to a
   renderer; this crate does not render.
5. **Catalog tooling and prompt wording** (`core/catalog.yaml`,
   `agent/inference_format.yaml`, 43 cases). Schema pruning, example loading,
   catalog negotiation, and upstream's exact prompt text are not implemented.

## One deliberate difference in classification

`test_validate_orphaned_component_v09` expects an error when a component cannot
be reached from `root`. This crate reports that as a **warning**
(`ValidationReport::unreachable`) rather than an error, because the specification
tells renderers to buffer components until their parent arrives, so an
unreachable component is usually a half-streamed tree rather than a broken one.
The harness therefore asserts the condition is *detected*, not that it is fatal.
This is the only place the harness maps an upstream expectation onto a different
severity, and it is called out in `classify_expectation`.

Two further scoping notes on the validator cases the harness does run: it turns
off component-type and required-property checking, because upstream delegates
those to JSON Schema in the cases being compared, and it turns off data-binding
checking, because upstream's structural validator does not look at bindings at
all. Matching upstream's scope is what makes the comparison meaningful.
