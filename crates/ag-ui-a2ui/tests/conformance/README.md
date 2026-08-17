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

## Current standing: 118 passed, 75 skipped, 0 failed

| File | Cases | Checks executed here |
|---|---:|---|
| `core/validator.yaml` | 45 | 21 of 51 — every v0.9 structural case, plus the depth limits |
| `core/catalog.yaml` | 24 | 23 of 24 — prune, render, load, modifiers |
| `core/accessibility.yaml` | 4 | none |
| `agent/parser.yaml` | 19 | 19 — all of them |
| `agent/inference_format.yaml` | 19 | 17 of 19 — catalog negotiation, prompts |
| `agent/streaming_parser.yaml` | 76 | 38 of 76 — every v0.9 case |
| `test_data/` | — | fixtures the cases above load |

A case with `steps` counts as one check per step, so the check totals exceed the
case counts in places.

## Why cases are skipped

Every case the harness cannot execute is skipped **with a reason and a count**,
printed by the test. Nothing is silently ignored, and no case is counted as
passing unless this crate actually produced the expected outcome. Run

```
cargo test -p ag-ui-a2ui --all-features -- --nocapture
```

to see the report, and set `A2UI_CONFORMANCE_VERBOSE=1` to list every check by
name.

The 75 skips break down as:

| Count | Reason |
|---:|---|
| 63 | **v0.8 wire format.** v0.8 nests component properties under the type name (`component: {Text: {...}}`) and uses different message names. This crate implements v0.9, where components are flat. |
| 6 | **JSON Schema validation.** Upstream validates envelopes, component properties and example files with a JSON Schema engine, and these cases assert its error codes (`missing_field`, `type_mismatch`) or its messages (`123 is not of type 'string'`). This crate has no JSON Schema engine and adding one for six cases is not worth the dependency. |
| 4 | **Renderer accessibility.** Accessibility trees and axe-core rules belong to a renderer; this crate does not render. |
| 2 | **v0.8 schema bundle in a prompt.** Two `generate_prompt` cases ask for the v0.8 schema documents to be embedded in the prompt; this crate ships v0.9. The other six prompt cases run. |

The depth-limit cases now pass: `ValidateOptions::max_depth` defaults to 50 and
`max_function_call_depth` to 5, matching every other toolkit, and a payload past
either reports `max_depth_exceeded`.

Version gating is applied only where the version actually matters. `validate`
and `process_chunk` are wire-format operations and are gated; schema surgery
(`prune`, `render`, `load`, `remove_strict_validation`) works on the shape of
the documents rather than on the protocol, so those run for v0.8 cases too.

## Two deliberate differences

**Unreachable components are a warning, not an error.**
`test_validate_orphaned_component_v09` expects an error when a component cannot
be reached from `root`. This crate reports that as
`ValidationReport::unreachable`, because the specification tells renderers to
buffer components until their parent arrives, so an unreachable component is
usually a half-streamed tree rather than a broken one. The harness therefore
asserts the condition is *detected*, not that it is fatal. This is the only
place the harness maps an upstream expectation onto a different severity, and it
is called out in `classify_expectation`.

**Validator cases run at upstream's scope.** For the `validate` cases the
harness turns off component-type and required-property checking, because
upstream delegates those to JSON Schema in the cases being compared, and turns
off data-binding resolution, because upstream's structural validator does not
look at bindings. Pointer *syntax* checking stays on, since upstream checks that
too. Matching upstream's scope is what makes the comparison meaningful.
