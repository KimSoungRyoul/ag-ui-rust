# Vendored A2UI conformance suite

The A2UI project ships its conformance tests as language-agnostic YAML and asks
every SDK to run them: read the YAML, feed the inputs to that language's
implementation, assert the outputs. The suite is vendored here whole, at
upstream's current layout, and driven by `crates/ag-ui-a2ui/tests/conformance.rs`.
The runners for the agent tier are beside it, in `tests/conformance_agent/`.

- **Upstream:** <https://github.com/a2ui-project/a2ui> (also reachable as `google/A2UI`)
- **Commit:** `3ed9601862ecc78027e20ac80ad8b1b98a5d9079` (2026-11-14)
- **Path upstream:** `conformance/`
- **License:** Apache-2.0. `LICENSE` is upstream's, and every YAML suite keeps
  its copyright header.

Do not hand-edit these files. To update, re-copy from upstream at a newer commit,
change the SHA here and in `UPSTREAM_COMMIT` in the harness, and update the table
below from the report the harness prints: the harness fails until the two agree.

## What is here

| Path | What it is |
|---|---|
| `core/`, `agent/`, `adapter/`, `e2e/`, `extensions/` | The suites, by upstream's tiers, copied verbatim. |
| `test_data/` | The fixtures the suites load, verbatim. |
| `features.yaml`, `*.schema.json`, `conformance_schema.json`, `manifest_corpora.json` | Upstream's feature registry and the schemas of the case vocabulary, verbatim. |
| `UPSTREAM_README.md`, `conformance_harness_implementation_guide.md` | Upstream's description of the suites and of how to write a harness. `UPSTREAM_README.md` is upstream's `README.md`, renamed because this file has the name. |
| `specification/`, `catalogs/` | The documents the executed suites name by repository path: under `specification/`, the basic catalogs and the common types of v0.9 and v0.9.1, the v0.9 minimal catalog, and the v1.0 schemas (`v1_0/json/`); under `catalogs/`, the v1 basic catalog, and the VegaChart and MCP catalogs of v0.9 and of v1. Copied from the same commit, byte for byte, at the same path relative to the repository root. |
| `specification/v0_9/catalogs/basic/examples/`, `specification/v0_9_1/catalogs/basic/examples/`, `catalogs/basic/v1/examples/` | The example surfaces upstream publishes beside each basic catalog: 44, 43 and 43 files. No suite names them; `tests/official_examples.rs` runs each against the schemas and the catalog of its version. Same commit, byte for byte. |
| `retired/` | One suite as it stood at the previous pin, `44a420b67957fafc0b02d55a153fdaf72e32ffb5` (2026-08-14), with its fixture. See "Retired suites". |
| `LICENSE` | Upstream's licence. |

Not copied: upstream's Python tooling for the suite (`scripts/`, `tests/`,
`pyproject.toml`), and the specification documents that only suites this harness
does not execute refer to.

## Standing

Every suite file is listed in `SUITES` in the harness with what this crate does
with it and how many of its checks end in each state. The test fails when a
count is anything else, when a suite on disk is not listed, and when a row below
differs from the report. A check is in one of four states:

- **Passed.** This crate produced the outcome the case expects.
- **Expected divergence.** The case was executed, and this crate does something
  else on purpose. Each one is pinned: the harness checks the crate's own
  outcome and that upstream's expectation is still the one it diverges from.
- **Skipped.** Not executed, with the reason. For a whole suite the reason is
  one of those under "Why suites are skipped"; for one case in an executed suite
  it is printed by the harness.
- **Failing.** Executed, and this crate does not do what the case expects. Each
  is listed by name in `KNOWN_FAILURES` with the reason, and must go on failing:
  one that starts passing fails the suite until it is taken off the list.

A case with `steps` counts one check per step in the validator suites, and a
transcript under `e2e/` counts one check per format it declares. Every other
suite counts one check per case and verifies its steps in order; a case that
stops at a step is reported under that step, `suite::case[step 2]`, and that is
the name `KNOWN_FAILURES` and `DIVERGENCES` list it by.

Totals: 753 passed, 34 expected divergence, 1242 skipped, 5 failing.

| Suite | Cases | Passed | Expected divergence | Skipped | Failing | Standing |
|---|---:|---:|---:|---:|---:|---|
| `core/validator_v0_9.yaml` | 38 | 39 | 3 | 5 | 0 | executed |
| `core/data_context.yaml` | 11 | 9 | 2 | 0 | 0 | executed |
| `core/data_model.yaml` | 52 | 45 | 4 | 1 | 2 | executed |
| `core/expressions.yaml` | 98 | 97 | 0 | 1 | 0 | executed |
| `core/message_processor_v0_9.yaml` | 48 | 42 | 2 | 4 | 0 | executed |
| `core/catalog.yaml` | 73 | 48 | 8 | 16 | 1 | executed |
| `core/common_types.yaml` | 22 | 19 | 0 | 3 | 0 | executed |
| `retired/core/validator.yaml` | 45 | 26 | 0 | 25 | 0 | executed; retired upstream |
| `core/validator_v1_0.yaml` | 49 | 56 | 2 | 2 | 0 | executed |
| `core/message_processor_v1_0.yaml` | 84 | 73 | 4 | 5 | 2 | executed |
| `core/message_processor_scaling.yaml` | 3 | 0 | 0 | 3 | 0 | later: timing (#75): the v1.0 message processor: time against the number of messages |
| `core/data_deletion.yaml` | 7 | 14 | 0 | 0 | 0 | executed |
| `core/data_model_pointers.yaml` | 7 | 14 | 0 | 0 | 0 | executed |
| `core/reserved_keys.yaml` | 10 | 10 | 0 | 0 | 0 | executed |
| `core/multi_catalog.yaml` | 8 | 5 | 1 | 2 | 0 | executed |
| `core/composition_constraints.yaml` | 10 | 9 | 1 | 0 | 0 | executed |
| `core/index_function.yaml` | 4 | 1 | 0 | 3 | 0 | executed |
| `core/validation_result.yaml` | 6 | 0 | 0 | 7 | 0 | executed |
| `core/rpc_functions.yaml` | 33 | 0 | 0 | 33 | 0 | later: A2UI v1.0 (#42): callRendererFunction and its response |
| `core/agent_function_routing.yaml` | 43 | 0 | 0 | 43 | 0 | later: A2UI v1.0 (#42): routing function calls to the agent with callAgentFunction |
| `core/validator_v0_8.yaml` | 27 | 0 | 0 | 27 | 0 | out of scope: v0.8: validation of v0.8 messages |
| `core/message_processor_v0_8.yaml` | 9 | 0 | 0 | 9 | 0 | out of scope: v0.8: the v0.8 message processor |
| `core/agent_to_renderer.yaml` | 4 | 0 | 0 | 4 | 0 | not applicable: a message schema generated from an SDK's own types against the published one; this crate ships the published schema |
| `core/functions.yaml` | 116 | 0 | 0 | 116 | 0 | out of scope: renderer: evaluating the basic catalog functions, which run on the renderer |
| `core/actions.yaml` | 18 | 0 | 0 | 18 | 0 | out of scope: renderer: dispatching a user action from a rendered component |
| `core/error_responses.yaml` | 24 | 0 | 0 | 24 | 0 | out of scope: renderer: the error messages a renderer sends back to the agent |
| `core/node_resolution.yaml` | 31 | 0 | 0 | 31 | 0 | out of scope: renderer: materialising the reactive component node tree |
| `core/accessibility.yaml` | 7 | 0 | 0 | 7 | 0 | out of scope: renderer: accessibility attributes of rendered components |
| `core/adapter_conformance.yaml` | 22 | 0 | 0 | 22 | 0 | out of scope: renderer: rendered view hierarchies in the UI framework adapters |
| `agent/request_processor.yaml` | 29 | 20 | 0 | 9 | 0 | executed |
| `agent/catalog_resolution.yaml` | 23 | 23 | 0 | 0 | 0 | executed |
| `agent/catalog_provider.yaml` | 10 | 9 | 0 | 1 | 0 | executed |
| `agent/catalog_transformer.yaml` | 15 | 15 | 0 | 0 | 0 | executed |
| `agent/renderer_capabilities.yaml` | 24 | 5 | 0 | 19 | 0 | executed |
| `agent/renderer_state.yaml` | 9 | 5 | 0 | 4 | 0 | executed |
| `agent/streamed_answer.yaml` | 6 | 6 | 0 | 0 | 0 | executed |
| `agent/direct_json/prompt_generator.yaml` | 15 | 1 | 0 | 14 | 0 | executed |
| `agent/direct_json/response_parser.yaml` | 36 | 35 | 0 | 1 | 0 | executed |
| `agent/direct_json/response_streaming.yaml` | 139 | 87 | 7 | 45 | 0 | executed |
| `agent/direct_json/compiler.yaml` | 38 | 18 | 0 | 20 | 0 | executed |
| `agent/direct_json/decompiler.yaml` | 11 | 9 | 0 | 2 | 0 | executed |
| `agent/direct_json/scaling.yaml` | 7 | 0 | 0 | 7 | 0 | later: timing (#75): the direct JSON format: parse time against payload size |
| `agent/skill.yaml` | 12 | 0 | 0 | 12 | 0 | later: other formats (#74): skill documents generated from a format and its catalogs; the goldens are Express |
| `agent/express/prompt_generator.yaml` | 23 | 0 | 0 | 23 | 0 | later: other formats (#74): the Express format: system prompt snippet |
| `agent/express/response_parser.yaml` | 38 | 0 | 0 | 38 | 0 | later: other formats (#74): the Express format: unwrapping and parsing a response |
| `agent/express/response_streaming.yaml` | 41 | 0 | 0 | 41 | 0 | later: other formats (#74): the Express format: parsing a response chunk by chunk |
| `agent/express/compiler.yaml` | 135 | 0 | 0 | 135 | 0 | later: other formats (#74): the Express format: compiling a block into messages |
| `agent/express/decompiler.yaml` | 49 | 0 | 0 | 49 | 0 | later: other formats (#74): the Express format: writing messages back as notation |
| `agent/express/scaling.yaml` | 1 | 0 | 0 | 1 | 0 | later: other formats (#74): the Express format: compile time against payload size |
| `agent/atom/compiler.yaml` | 7 | 0 | 0 | 7 | 0 | later: other formats (#74): the Atom format |
| `agent/elemental/compiler.yaml` | 8 | 0 | 0 | 8 | 0 | later: other formats (#74): the Elemental format |
| `agent/builder/builder.yaml` | 22 | 0 | 0 | 22 | 0 | later: other formats (#74): the typesafe builder API |
| `agent/macros/macros.yaml` | 70 | 0 | 0 | 70 | 0 | later: other formats (#74): builder macros |
| `agent/legacy/parser.yaml` | 23 | 0 | 0 | 23 | 0 | superseded: the SDK design before the agent layer: the tag parser; `agent/direct_json/response_parser.yaml` is the suite of the parser that reads the same tags now |
| `agent/legacy/inference_format.yaml` | 22 | 0 | 0 | 22 | 0 | superseded: the SDK design before the agent layer: catalog selection and prompt assembly; `agent/catalog_resolution.yaml`, `agent/request_processor.yaml` and `agent/direct_json/prompt_generator.yaml` are the suites of what does that now |
| `agent/legacy/streaming_parser.yaml` | 88 | 0 | 0 | 88 | 0 | superseded: the SDK design before the agent layer: the streaming parser, with placeholders for components still to arrive; `agent/direct_json/response_streaming.yaml` is the suite of the stream that replaced it |
| `agent/multi_catalog_formats.yaml` | 35 | 0 | 0 | 35 | 0 | later: A2UI v1.0 (#42): components and function calls resolved across several active catalogs |
| `agent/agent_functions.yaml` | 64 | 0 | 0 | 64 | 0 | later: A2UI v1.0 (#42): agent functions and the agent's side of renderer messages |
| `agent/renderer_function_calls.yaml` | 30 | 0 | 0 | 30 | 0 | later: A2UI v1.0 (#42): the agent calling renderer functions |
| `e2e/v0_9_1/login_form_action.yaml` | 1 | 1 | 0 | 1 | 0 | executed |
| `e2e/v0_9_1/missing_surface_error.yaml` | 1 | 1 | 0 | 1 | 0 | executed |
| `e2e/v0_9_1/newsletter_signup.yaml` | 1 | 1 | 0 | 1 | 0 | executed |
| `e2e/v0_9_1/restaurant_list_booking.yaml` | 1 | 1 | 0 | 1 | 0 | executed |
| `e2e/login_form_action.yaml` | 1 | 0 | 0 | 1 | 0 | later: A2UI v1.0 (#42): a v1.0 transcript |
| `e2e/missing_surface_error.yaml` | 1 | 0 | 0 | 1 | 0 | later: A2UI v1.0 (#42): a v1.0 transcript |
| `e2e/newsletter_signup.yaml` | 1 | 0 | 0 | 1 | 0 | later: A2UI v1.0 (#42): a v1.0 transcript |
| `e2e/restaurant_list_booking.yaml` | 1 | 0 | 0 | 1 | 0 | later: A2UI v1.0 (#42): a v1.0 transcript |
| `adapter/adapter_conformance_v0_9.yaml` | 16 | 0 | 0 | 16 | 0 | out of scope: renderer: rendered view hierarchies in the UI framework adapters |
| `adapter/markdown_subset.yaml` | 7 | 0 | 0 | 7 | 0 | out of scope: renderer: rendering the Markdown subset |
| `extensions/a2a/a2a_integration.yaml` | 11 | 9 | 0 | 2 | 0 | executed |
| `extensions/adk/adk_extensions.yaml` | 8 | 0 | 0 | 8 | 0 | not applicable: integration with Google's ADK, which this workspace does not target |

Run

```
cargo test -p ag-ui-a2ui --all-features --test conformance -- --nocapture
```

to see the report, and set `A2UI_CONFORMANCE_VERBOSE=1` to list every check by
name.

## The agent tier

The suites under `agent/` test the layer an agent framework delegates A2UI to
(`ag_ui_a2ui::agent`): catalog negotiation, the prompt snippet, reading a
model's answer whole or chunk by chunk, renderer state. Upstream writes them
once, against A2UI v1.0, and asks an SDK that speaks an older version to lower
what a case hands it and lift what it hands back, rather than keep a second copy
of the suites (`conformance_harness_implementation_guide.md`, "Patterns and
pitfalls in Inference harnesses"). The protocol modules of this crate speak
v1.0, and the core-tier suites run as written. Its agent layer speaks v0.9 and
v0.9.1 until #42 brings it v1.0, so that is what the agent-tier runners do.

Every rule is in one test-only module, `tests/conformance_agent/lowering.rs`,
and a case is lowered only when its catalogs declare v1.0. A case whose catalog
declares v0.9 runs as written.

| # | v1.0, as the suites write it | v0.9, as this crate reads it |
|---|---|---|
| 1 | `"version": "1.0"` | `"version": "v0.9"`. A case that writes `v0.9` means "a version other than the catalog's", and becomes `1.0`. |
| 2 | A catalog document's `"protocolVersion": "1.0"` | `"0.9"` |
| 3 | `common_types.json#/$defs/Child` in a catalog | `…#/$defs/ComponentId`, which v1.0's `Child` is a reference to |
| 4 | Capabilities keyed `"1.0"` | Keyed `"v0.9"`, with inline catalogs lowered by rules 2 and 3 |
| 5 | A binding `{"@path": …}` and a call `{"@call": …}` in a component | `{"path": …}` and `{"call": …}`. A `ChildList` template (`componentId` beside `path`) keeps its keys in both versions. |
| 6 | `"@checks"` and `"@accessibility"` on a component | `"checks"` and `"accessibility"` |
| 7 | A `createSurface` that names no catalog, in a case that never names one | The first catalog of the case is named, and removed again from what comes back |
| 8 | A `createSurface` carrying `components` and `dataModel`, in decoded messages | `createSurface`, then the `updateComponents` and the `updateDataModel` at `/` that fill it |

Lifting applies rules 1, 5, 6 and 7 backwards to the messages the crate emits.
Two things are compared as equal that are written differently: a `createSurface`
that states `sendDataModel: false` and one that leaves it out, and an
`updateDataModel` that states the path `/` and one that states none. A typed
message states both defaults.

A rule rewrites how a case is spelled. A case whose meaning v0.9 cannot state is
skipped, not bent: a `callRendererFunction` message, a component that names a
catalog of its own, a `createSurface` that carries its components in text a
model wrote, a prompt snippet compared byte for byte with a file that embeds the
v1.0 schemas, a provider told a version of another protocol family than its
document's. Those are the "v1.0" column below, and what #42 takes off the list.

Three more things about how the cases run:

- **Holding components until they close.** From v1.0 a stream emits a component
  only once its JSON object has closed, and the cases written against v1.0 fix
  that timing. A lowered case therefore runs with
  `DirectJson::holding_components_until_closed(true)`; a case written against
  v0.9 runs with the default, where a component is emitted while it arrives.
- **Validation is the semantic validator's.** The processors are built without
  the JSON Schema check (`checking_schemas`), so the counts do not depend on a
  schema engine.
- **Transcripts.** `e2e/v0_9_1/*.yaml` are stated in v0.9 and run as written:
  each turn's chunks through a stream, and joined through one parse, against a
  state that lives across the turns. Only the agent half is run, and only for
  direct JSON. The catalog path names the published basic catalog, which is the
  copy this crate pins.

How each executed agent-tier suite ran. "Lowered" and "as written" count
executed checks; a case that hands the crate nothing a rule would rewrite, such
as text to unwrap, counts as written.

| Suite | Cases | Lowered from v1.0 | As written | Skipped: v1.0 | Skipped: v0.8 | Skipped: another format |
|---|---:|---:|---:|---:|---:|---:|
| `agent/request_processor.yaml` | 29 | 19 | 1 | 0 | 0 | 9 |
| `agent/catalog_resolution.yaml` | 23 | 22 | 1 | 0 | 0 | 0 |
| `agent/catalog_provider.yaml` | 10 | 6 | 3 | 1 | 0 | 0 |
| `agent/catalog_transformer.yaml` | 15 | 15 | 0 | 0 | 0 | 0 |
| `agent/renderer_capabilities.yaml` | 24 | 0 | 5 | 17 | 2 | 0 |
| `agent/renderer_state.yaml` | 9 | 5 | 0 | 0 | 0 | 4 |
| `agent/streamed_answer.yaml` | 6 | 6 | 0 | 0 | 0 | 0 |
| `agent/direct_json/prompt_generator.yaml` | 15 | 0 | 1 | 14 | 0 | 0 |
| `agent/direct_json/response_parser.yaml` | 36 | 5 | 30 | 1 | 0 | 0 |
| `agent/direct_json/response_streaming.yaml` | 139 | 51 | 43 | 6 | 39 | 0 |
| `agent/direct_json/compiler.yaml` | 38 | 18 | 0 | 20 | 0 | 0 |
| `agent/direct_json/decompiler.yaml` | 11 | 9 | 0 | 2 | 0 | 0 |
| `e2e/v0_9_1/login_form_action.yaml` | 1 | 0 | 1 | 0 | 0 | 1 |
| `e2e/v0_9_1/missing_surface_error.yaml` | 1 | 0 | 1 | 0 | 0 | 1 |
| `e2e/v0_9_1/newsletter_signup.yaml` | 1 | 0 | 1 | 0 | 0 | 1 |
| `e2e/v0_9_1/restaurant_list_booking.yaml` | 1 | 0 | 1 | 0 | 0 | 1 |

In all: 156 checks ran lowered from v1.0 and 88 as written.

`agent/renderer_capabilities.yaml` states a version in every case, so each runs
as written or not at all; its seventeen v1.0 cases hold most of the rules about
a malformed entry. Those rules do not vary by version, and the crate's own tests
state them for v0.9 (`agent::capabilities`). The same goes for the prompt
snippet: fourteen of its fifteen cases compare a v1.0 golden, and the v0.9
snippet is tested in `agent::direct_json::prompt`.

## Why suites are skipped

| Label | Suites | Cases | What it means |
|---|---:|---:|---|
| later: other formats (#74) | 11 | 406 | The suite tests an output format other than direct JSON (Express, Atom, Elemental), or something built on one: the typesafe builder, its macros, skill documents. |
| later: timing (#75) | 2 | 10 | The suite times an operation instead of checking its result: the direct JSON stream, and the v1.0 message processor. The stream is not yet linear in the size of one message, and timing cases need a job of their own. |
| later: A2UI v1.0 (#42) | 9 | 209 | The suite tests a part of A2UI v1.0 this crate does not have yet: the agent layer on v1.0 (`agent/multi_catalog_formats.yaml`, the four v1.0 transcripts), and function calls between the two sides (`core/rpc_functions.yaml`, `core/agent_function_routing.yaml`, `agent/agent_functions.yaml`, `agent/renderer_function_calls.yaml`). |
| superseded: the SDK design before the agent layer | 3 | 133 | The suite tests the design upstream's SDK had before its agent layer: a tag parser, a streaming parser that drew placeholders for components still to arrive, a schema manager that selected a catalog and assembled a prompt. Upstream keeps those suites under `agent/legacy/` and wrote the agent-tier suites in their place. This crate had ports of that design in `toolkit` and removed them when the agent layer took over (#41); see "The legacy suites". |
| out of scope: renderer | 8 | 241 | The suite tests a renderer: drawing, evaluating functions, dispatching actions, reporting errors back. This crate does not render. |
| out of scope: v0.8 | 2 | 36 | The suite tests the v0.8 wire format. This crate speaks v0.9, v0.9.1 and v1.0. |
| not applicable | 2 | 12 | The suite asks something of an SDK that does not apply here: a schema generated from the SDK's own types, an integration with Google's ADK. |

Upstream's `adapter/` tier is its UI framework adapters (`@a2ui/lit`, React,
Angular, Flutter, SwiftUI), that is, renderers. The suites about transports are
under `extensions/`.

## The legacy suites

`agent/legacy/parser.yaml`, `agent/legacy/inference_format.yaml` and
`agent/legacy/streaming_parser.yaml` were executed while this crate carried the
code they describe: `toolkit::parser`, `toolkit::negotiate` with
`toolkit::prompt`, and `toolkit::streaming`. That code is gone. One generation
path is left, the agent layer, and its suites are the agent-tier ones:

| Legacy suite | What reads or does that now | Its suite |
|---|---|---|
| `parser.yaml` | `DirectJson`, through `A2uiRequestProcessor::parse_response` | `agent/direct_json/response_parser.yaml`, `compiler.yaml` |
| `streaming_parser.yaml` | `ResponseStream`, through `A2uiRequestProcessor::open_stream` | `agent/direct_json/response_streaming.yaml`, `agent/streamed_answer.yaml` |
| `inference_format.yaml` | `resolve_catalogs`, `A2uiGenerator`, the format's prompt snippet | `agent/catalog_resolution.yaml`, `agent/request_processor.yaml`, `agent/direct_json/prompt_generator.yaml` |

The legacy cases are not run against the agent layer, because the two designs
answer differently on purpose, and the agent-tier suites are where upstream
states the current answers. A response with no tags is an error to the legacy
parser and plain text to the agent layer. The legacy stream holds a component
back until the root reaches it and draws a `loading_…` placeholder in its
place; the agent layer's stream emits a component when it arrives, since the
specification has the renderer buffer it. The legacy prompt is a whole system
prompt with a role and a workflow section; the agent layer supplies the A2UI
part and the framework writes the rest.

Of the three checks these suites had failing, one is taken up where it still
applies: an invalid escape in a block now says "Unescaped backslash found" and
how to write one, since the message is read by the model that has to answer
again (`agent::direct_json`). The other two were about placeholders and about
a second open tag inside an open block, and went with the parser they
described.

## Expected divergences

Thirty-four checks: three in `core/validator_v0_9.yaml`, two in
`core/data_context.yaml`, four in `core/data_model.yaml`, two in
`core/message_processor_v0_9.yaml`, eight in `core/catalog.yaml`, two in
`core/validator_v1_0.yaml`, four in `core/message_processor_v1_0.yaml`, one
each in `core/multi_catalog.yaml` and `core/composition_constraints.yaml`, and
seven in `agent/direct_json/response_streaming.yaml`.
Those of the core suites run one check per case are listed in `DIVERGENCES` in
the harness with what this crate gives, and the harness holds it to exactly
that.

The seven agent-tier ones are listed in `AGENT_DIVERGENCES` in the harness, each
with what its failure has to say, so that an entry cannot go on covering a case
that started failing for another reason. Two follow from what
`ag_ui_a2ui::validate` holds a finished surface to, which the agent layer does
not relax. Five follow from when a stream emits an `updateDataModel` that
states no `path`.

| Checks | Reference expectation | Checked behaviour |
|---:|---|---|
| 1 (`test_stream_orphan_component_fails_v09`) | A block that leaves a component the root cannot reach is rejected. | The block is emitted. An unreachable component is a warning, for the reason the next row gives. |
| 1 (`test_stream_relative_path_kept_v09`) | A binding with a relative path on the root component is emitted as written. | It is rejected as `unresolved_binding`: a relative path has no scope outside a list template. The reference does not look at bindings. |
| 5 (`test_chunk_progressive_key_heals_a_partial_string`, `test_chunk_data_model_grows_across_chunks`, `test_stream_data_model_grows_v09`, `test_stream_data_model_cut_key_and_value_v09`, `test_stream_data_model_keeps_earlier_entries_v09`) | An `updateDataModel` that states no `path` is emitted as its `value` grows, at the root, from the first chunk that holds part of the value. | It is emitted once, whole, on the chunk that closes it. The keys of a message arrive in the order the model wrote them, so a `path` may still follow the `value`; a value applied at the root before its path arrives replaces the surface's whole data model, and the stream then leaves something else than the same answer read whole. An `updateDataModel` that already holds its `path` is emitted as it grows, at that path. The last of the five would be rejected on its closing chunk as `unresolved_binding`: its list template iterates a path that holds a map, and the reference does not look at the data model. |
| 2 + 1 + 2 + 1 (`test_v09_topology_orphaned_component_error`, `test_v09_topology_plain_string_property_not_treated_as_child_ref`; `test_topology_orphaned_component_error` in the v0.9 processor suite; `test_v10_topology_orphaned_component_error`, `test_v10_topology_plain_string_property_not_treated_as_child_ref`; `test_composition_dropped_child_is_reported_as_orphan`, step 1) | A component the root cannot reach is an error. | It is reported in `ValidationReport::unreachable`, as a warning. The specification tells renderers to buffer a component until its parent arrives, so an unreachable component is usually a half-streamed tree. The processor accepts the message, and `MessageProcessor::report` names the component. |
| 1 (`test_custom_catalog_0_9`, step 2) | `children: 123` is `invalid_value`. | It is `type_mismatch`, at the same place. |
| 2 + 1 (`test_resolve_path_context_with_trailing_slash`, `test_resolve_path_empty_in_trailing_slash_context`; `test_resolve_path_relative_trailing_slash` in the processor suite) | A trailing slash on the scope is dropped: `foo` under `/bar/` is `/bar/foo`. | An empty segment is the empty key, as RFC 6901 reads it, and `""` is a key JSON allows: `foo` under `/bar/` is `/bar//foo`, and the scope `/user/` itself is `/user/`. Only `/` alone is the root. |
| 2 (`test_data_model_normalizes_trailing_slash`, `test_data_model_normalizes_empty_path_segments`) | Empty segments are dropped from a data-model path: `/foo/` is `/foo`, and `//a///b//` is `/a/b`. | The same rule as above: `/foo/` is the key `""` under `foo`. The write is accepted and stored there, so `/foo` then holds `{"": "bar"}`. |
| 8 (`test_v09_published_minimal_catalog_is_self_contained`, `test_v091_published_basic_catalog_is_self_contained`, `test_v09_published_vegachart_catalog_is_self_contained`, `test_v10_published_mcp_catalog_is_self_contained`, `test_v10_published_vegachart_catalog_is_self_contained`, `test_v09_basic_catalog_from_json`, `test_v10_basic_catalog_from_json`, `test_v10_catalog_from_json_leaf_defs`) | Loading a catalog gives a resolved schema that is self-contained: the common types are inlined and every `$ref` is inside the document. | This crate bundles nothing. A catalog's references resolve through the validator's registry of local documents, with retrieval off. The harness checks that: a `SchemaValidator` compiles from the catalog and the documents the crate ships, and from nothing else. Everything else these cases expect is held first: the two `basic_catalog_from_json` cases, that the published document read with `Catalog::from_schema` is `Catalog::basic_for` of its version. |
| 1 (`test_batch_atomic_rollback_on_candidate_topology_cycle`, step 1) | A loop of child references is an `IntegrityError` in the v1.0 processor suite. | It is a `RecursionError`, as in the v0.9 suite and as the reply it is sent with says: both suites want `RECURSION_LIMIT_EXCEEDED`. The batch is refused whole either way. |
| 3 (`test_v10_update_components_stores_component_metadata`, `test_v10_update_components_replaces_component_metadata`, `test_v10_inline_catalog_id_and_metadata_standard_only_beside_inline_id`) | A component sent with the bare `metadata` of the v1.0 drafts is read back with `@metadata`. | A component is stored as it was sent. The bare key is validated as the envelope where the component declares no property of that name, and is not rewritten: what a store replays is what it was given. |
| 1 (`test_multi_catalog_version_mismatch_error`) | A surface that lists a v0.9 catalog among the catalogs it supports is refused, whether or not anything on it is drawn from that catalog. | A processor holds the catalogs of every version it serves, and a surface has no list of its own. A catalog written for another version is refused where a surface, a component or a function call names it. |
| 2 (`test_data_model_get_out_of_range_index`, `test_data_model_rejects_leading_zero_list_index`) | Reading a list at `-1`, `invalid` or `01` finds no value. | It is an `Error::Pointer`, on a read as on a write. The specification says of a path that "it is an error to use a non-numeric index on a path segment that refers to an array", and RFC 6901 has no index `01`. |

## Failing

Five checks. Two are about removing a path that is not there from a v0.9 data
model, where upstream's rule and the renderer this workspace tests against
disagree (#42). Three are about JSON Schema validation of a v1.0 message that
draws from more than one catalog (#97).

| Check | What happens |
|---|---|
| `core/data_model.yaml::test_data_model_delete_out_of_range_list_index_is_noop[step 1]` | Removing `/items/10` from a list of three pads the list to eleven slots, eight of them undefined. The case wants the list left as it was. Upstream added the case on 2026-09-15. The official renderer core that `examples/review-desk/interop` runs against, `@a2ui/web_core` 0.11.0, pads as this crate does, and that test and `tests/protocol_091.rs` assert it. On a v1.0 surface the removal does nothing (`core/data_deletion.yaml` passes); for v0.9 the rule is read again in #42, when that harness moves to a newer renderer. |
| `core/data_model.yaml::test_data_model_delete_absent_deep_path_is_noop[step 0]` | Removing `/nonexistent/child/path` from an empty model leaves `{"nonexistent": {"child": {}}}`. The case wants nothing created, and no watched path changed. The same rule, and the same issue (#42). |
| `core/message_processor_v1_0.yaml::test_v10_function_call_naming_catalog_with_invalid_args_errors` | A call that names another catalog than its surface's has an argument of the wrong type. The call's catalog and function are found; its arguments are not held to that catalog's JSON Schema, so the message is accepted. |
| `core/message_processor_v1_0.yaml::test_v10_call_in_list_with_invalid_args_errors` | The same, for a call inside a list in a component of another catalog. |
| `core/catalog.yaml::test_v10_published_basic_catalog_validates_nested_calls` | A `SchemaValidator` is compiled for one catalog. A call that names another one matches no function of its schema, so a component the case lists as valid is refused. A `MessageProcessor` does not run the schema check on such a message. |

## Skips inside the executed suites

The agent-tier suites are in the table under "The agent tier", by suite and by
label: 61 checks for v1.0 (#42), 41 for v0.8, 13 cases for another format and
the Express half of the four transcripts (#74). The rest:

| Count | Reason |
|---:|---|
| 25 + 4 + 2 | **v0.8.** v0.8 nests component properties under the type name and uses different message names. 25 checks of `retired/core/validator.yaml`, 4 cases of `core/catalog.yaml` on the v0.8 catalog, and the 2 cases of `extensions/a2a/a2a_integration.yaml` that activate the v0.8 extension. |
| 1 + 3 + 2 + 7 | **A value only a renderer has.** The messages of the step are validated; what the case then expects is something a renderer computes. The text a catalog function returns (`formatCurrency` in `core/expressions.yaml`, `formatDate` in `core/multi_catalog.yaml`), the components a list template draws (three steps of `core/index_function.yaml`, one of `core/multi_catalog.yaml`), and the result of a component's check rules (all seven steps of `core/validation_result.yaml`). The first six are listed by name in `RENDERER_VALUES`. |
| 5 + 1 | **A JSON Schema failure**: in `core/validator_v0_9.yaml`, an undeclared property, an undeclared function, a function argument of the wrong type, and two theme values; in `core/validator_v1_0.yaml`, a function argument of the wrong type. The semantic validator the harness runs does not evaluate these; `SchemaValidator`, behind the `schema-validation` feature, does. They are listed by name in `SCHEMA_ONLY`, because their fixtures are also invalid in ways the semantic validator does report, and would otherwise pass for the wrong reason. |
| 1 | **A category and nothing else.** `test_v10_invalid_json_pointer_path_error` in `core/validator_v1_0.yaml` expects a `ValidationError` for an `updateDataModel` whose path is not a JSON Pointer, on a surface that was never created. This crate refuses the message for the surface, before it reads the path, so the refusal would satisfy the category for another reason than the case's. |
| 3 | **Validation rules given for one call.** Three cases of `core/message_processor_v1_0.yaml` put a `validationConfig` on a step. That belongs to the optional `agent_renderer_app` feature of the reference SDKs; a `MessageProcessor` has the options it was made with. |
| 1 | **Detaching observers.** `test_data_model_stops_notifying_after_dispose`, in `core/data_model.yaml`. Registering observers on a data model is a renderer's; `DataModel` has none to detach. |
| 4 | **`validation_schema`**, in `core/catalog.yaml`. `validationSchema` is the reference's bundled schema: `allOf` envelopes flattened, an `id` envelope added to each component, the referenced common types inlined, compared whole with a golden file. This crate validates against the documents as published, through a registry, and has no bundled form to compare. |
| 8 | **`derive`**, in `core/catalog.yaml`. `Catalog` has no `derive`. Restricting a catalog to some of its entries is `CatalogConfig::keeping_components` and `keeping_functions` in the agent layer; building a new catalog from named entries and new ones is #96. |
| 3 | **A schema generated from an SDK's own types.** The three `common_types_schema` cases of `core/common_types.yaml` compare the common types an SDK generates from its models with the published document. This crate generates none: it ships the published document, and `the_shipped_common_types_are_the_published_ones` in the harness checks that it is the published one, for each version. Not applicable, like `core/agent_to_renderer.yaml`. |
| 4 + 2 | **Advertising capabilities.** The `get_renderer_capabilities` cases of the two processor suites ask for the capabilities a renderer sends, with its catalogs rewritten inline. This crate reads capabilities (`agent::RendererCapabilities`), and no renderer in it writes them. |

## How the validator cases are run

A `validate` case runs through one validator and one surface state across its
steps, so a later step is checked against the surfaces earlier steps left
(`Validator::validate_json_updates`). A step that fails changes nothing. That
is `core/validator_v0_9.yaml` and `core/validator_v1_0.yaml`, and the
`validate` cases of `core/reserved_keys.yaml`, `core/index_function.yaml`,
`core/validation_result.yaml`, `core/multi_catalog.yaml`,
`core/composition_constraints.yaml` and `core/expressions.yaml`.

A case with one catalog gets a validator for it, which stands for whatever id
its surfaces name. A case with several gets `Validator::over` them, and each
surface, component and call is read from the catalog it names. Where the
messages name `basic` beside the catalogs the case declares, that is the basic
catalog of the case's version, which every reference harness holds without
being told. A catalog fragment that states no version is written for the
case's. Four things about the comparison:

- **Scope.** Component types, required properties, property types and enums,
  the envelope and pointer syntax are checked. Data bindings are not resolved,
  because upstream's validator does not look at the data model.
- **A step that only creates surfaces leaves them pending.** Upstream accepts a
  `createSurface` with nothing drawn into it yet. This crate's whole-surface
  contract wants a root, so the harness applies the lifecycle and lets the step
  that fills the surface be the one judged.
- **Matching an expectation.** A sentence of the reference validator is mapped
  to the code this crate reports for the same defect. A list of coded `details`
  is matched on code and location. A bare `category` names no failure, and any
  error at all would satisfy it: twelve such steps are mapped by hand to the
  failure their description names, by its code and where it is
  (`CATEGORY_ONLY`), and the rest are skipped. The `rendererError` a renderer
  would send is not checked; this crate is not one.
- **What the surfaces hold.** Where a case says what a surface holds after its
  steps, the store the validator hands back is compared with it, as a
  processor's is. A value only a renderer has is not: see "Skips inside the
  executed suites".

## How the other core suites are run

Each case is one check. A runner reads every key of a case, and a case that
carries a key the runner does not know fails rather than passing on the part
that was understood.

- **`resolve_path`** (`core/data_context.yaml`). `binding::absolute_path(scope,
  path)`, with no `contextPath` read as the root scope. The result is compared
  as written, so the root is `/`.
- **`data_model`** (`core/data_model.yaml`). One `DataModel` per case, built
  from `initial`. `get` is `DataModel::lookup`; `set` and `delete` are
  `DataModel::apply_reporting` with a value and with a removal. A `DataError`
  is an `Error::Pointer`, and a phrase of the reference's message is mapped to
  the reason this crate gives for the same fault.
  `DataModel` has no observers. The paths in `watch` are asked, after each
  write, whether their value changed (`ModelChange::changed`), and a path
  that says so counts as notified, once for each time `watch` lists it. That
  is the reference's own rule for firing an observer. The one case that
  detaches observers (`dispose`) has nothing to run against and is skipped.
  One case changes a value after handing it to `set` (`mutate_input`) and
  expects the model not to follow. The harness does change its copy; a
  `DataModel` owns what it stores, so in Rust the case cannot fail.
- **`parse_expression_template`** (`core/expressions.yaml`).
  `binding::parse_template`, each part written as the value a component would
  carry in its place, in the spelling of the case's version
  (`TemplatePart::to_json_in`): a literal, `{path}` or `{@path}`, and
  `{call, args, returnType}` or `{@call, args}`. As the suite asks, adjacent literal strings are
  joined and empty ones dropped on both sides before comparing, and a number is
  compared as a number, so `1e5` is the `100000` the suite writes. A
  `ParseError` is an `Error::Binding`, with the phrase of the reference's
  message mapped to this crate's reason for the same fault. The parser takes
  one thing the reference grammar does not, `~0` and `~1` in a path, the JSON
  Pointer escapes. No case has a `~` in a template, so none forbids it.
- **`process_messages`** (`core/message_processor_v0_9.yaml`,
  `core/message_processor_v1_0.yaml`, `core/data_deletion.yaml`,
  `core/data_model_pointers.yaml`, and the cases of that action in
  `core/reserved_keys.yaml` and `core/composition_constraints.yaml`). One
  `processor::MessageProcessor` per case; the case's messages, or each step's,
  go in as one batch (`process_all`), unwrapped when they come as
  `{"messages": [...]}`.
  - *Catalogs.* Inline `catalogs` are registered as they are, written for the
    case's version where they state none. A catalog named by `catalogPaths` is
    a published one, while the messages create surfaces under ids of their own
    (`test-catalog`, `basic`), so it is registered under each id the messages
    name, as the reference harnesses do. Where a case declares no catalog at
    all, such an id is the basic catalog of the message's version. A case that
    expects a `CatalogError` gets no such alias.
  - *Strictness.* With `strictMode: true` every message must leave its surface
    whole, by the options the validator suite uses. Without it the reference
    processor checks the envelope and the surface's life only, and its
    fixtures rely on that (a `Button` with only a `label`, a type no catalog
    defines), so the harness turns the catalog and tree checks off. A case
    that gives a `validationConfig` is strict, with `allowDanglingReferences`
    read as `ValidateOptions::allow_dangling_children`; a `validationConfig`
    on one step is a rule for one call, which a processor here does not have.
  - *Schemas.* A strict processor is also given the schemas of its catalogs
    (`MessageProcessor::register_schema`), so a message is refused for what
    only a JSON Schema engine reads: the case that creates a surface with a
    theme off the `pattern` of its catalog's theme schema is one. A published
    catalog document compiles as it is. An inline catalog of a case is a
    fragment, with a `theme` schema at its top level and no `$defs`; the
    harness completes it into a document around that theme, and leaves the
    fragment's components to the semantic validator, which is what reads them.
  - *The component keys of v1.0.* `@metadata`, `@catalogId`, `@accessibility`
    and `@checks` on a component of a v0.9 surface are refused unless the
    catalog declares a property by that name, strict or not. On a v1.0 surface
    they are the envelope, every other key that starts with one `@` is
    refused, and what a component or a call names is resolved: its catalog,
    and with the catalog checks on, its type or its function. The reference
    processor has these rules in every mode, and so they are the validator's
    own here (`A2uiVersion::component_keys`, `reserves_at_keys`,
    `resolves_function_calls`) and not the schema engine's.
  - *A refusal.* `expectError.category` is held with `RejectionKind::is`, so
    `ValidationError` accepts an integrity or a recursion refusal and
    `IntegrityError` does not accept a plain validation one. `surfaceId` is
    `Rejection::surface_id`. A `message` is a phrase of the reference's
    sentence, mapped to the fault this crate reports. Where the case gives the
    `rendererError` a renderer would send, `Rejection::renderer_error` has to
    have its `version`, `code`, `surfaceId` and `path`; its sentence is this
    crate's, and has to name what the case's sentence names where that is not
    the phrase already held.
  - *State.* `expect.surfaces.<id>`: `exists` is `Lifecycle::Live`;
    `sendDataModel` treats "not asked" as `false`; `dataModel` is the whole
    model; `components` as a list is every component, as a map some of them.
    A component's property is compared as a renderer would show it, bound
    against the surface's data model with `Scope::resolve_dynamic`.
- **`get_renderer_data_model`** (the processor suites). The messages go
  through a processor, and `SurfaceStore::client_data_model` is compared
  whole: the surfaces created in the version asked for, and no others.
- **`select_catalog`** (`core/multi_catalog.yaml`). A case describes a surface
  and what is drawn on it in the abstract; the harness writes that as
  messages. A catalog is registered for each id the surface supports,
  declaring what the case draws from it and nothing else; the surface is
  created under its default; the components go in as one update, a call in a
  component of its own. A component read from another catalog than the one
  that declares it is then refused as unknown. What the case selects is asked
  of the surface for its last component (`SurfaceState::catalog_of`). A call's
  function is declared by the catalog the case selects and by no other, so the
  call resolves only if it is read from that one.
- **`from_json`** (`core/catalog.yaml`). `Catalog::from_schema` on the inline
  `catalog`, or on the document `catalogPath` names. A case's own `catalogId`
  and `protocolVersion` are the ones the document is read under where it
  states none, as the reference harnesses hand both to their loader; a
  `catalogId` is also set on the loaded catalog. A v1.0 catalog is held to
  the rules of v1.0 when it is read, and a `CatalogError` phrase is one of
  the sentences those rules give.
  - `catalogId`, and `components` and `functions`: the names are exactly the
    ones the catalog defines, and where the case gives a schema for an entry
    it is the entry's schema in `Catalog::document`.
  - `protocolVersion`: `Catalog::version`, which is v0.9 for a document that
    states none, as the reference reads one.
  - `componentMetadata`: `ComponentDef::metadata` of each component named.
  - `validComponents`, `invalidComponents`: a `SchemaValidator` built from the
    documents the crate ships and this catalog, and each component sent to it
    in an `updateComponents` message.
  - `matchesBuiltinCatalog`: against `Catalog::basic_for` the catalog's
    version, on the id, the component and function names, each component's
    property and required names, and each function's return type.
  - `selfContained`: see "Expected divergences".
- **`round_trip`** (same suite). `Catalog::from_schema(c)?.document()` is `c`,
  and reading what it gave back gives it back again.
- **`to_json`** (same suite). The document of `Catalog::basic_for` the case's
  version against the published file.
- **`validate_common_type`** (`core/common_types.yaml`).
  `schema_validation::validate_common_type_in(version, definition, value)` on
  each step's value. A `ValidationError` is an `Error::Validation`. The suite also wants an
  accepted value to serialize back to itself; a schema rewrites nothing, so
  there is no second form to compare.

The version of a catalog case is the one it states, or the one its document
states: 21 cases about a v1.0 catalog say so only in the document. A case
about v0.8 is skipped whichever way it says so.

The harness is compiled only with the `schema-validation`, `agent` and `a2a`
features: these cases and the common-types ones run a JSON Schema engine, the
suites under `agent/` run the agent layer, and the A2A suite runs its adapter.
Without them, `cargo test -p ag-ui-a2ui` lists one ignored test in the suite's
place, `conformance_suite` in `tests/conformance_needs_schema_validation.rs`,
whose reason says which features to turn on.

The v0.9 suite has no case that deletes a surface that does not exist.
`core/message_processor_v1_0.yaml` has one, and its rule is that such a delete
does nothing. `MessageProcessor` follows it; `SurfaceStore::apply` refuses the
delete, as before.

## How the A2A suite is run

`extensions/a2a/a2a_integration.yaml` runs against `ag_ui_a2ui::adapters::a2a`,
which builds and reads A2A's JSON shapes and depends on no A2A library. Each
case is one check.

- **`create_a2ui_part`.** `a2a::data_part(data, version)`; the part has to be
  an A2UI part (`is_a2ui_part`) that carries the data, with the `mimeType` the
  case names.
- **`is_a2ui_part`.** A data part with the case's `mimeType`.
- **`get_extension`.** `a2a::agent_extension(version, acceptsInlineCatalogs,
  supportedCatalogIds)`; its `uri` and its `params`, which are absent where the
  case says `null`. The version is a string here, so the three cases that ask
  for the v0.8 extension's URI run.
- **`select_newest`.** `a2a::select_newest(requested, advertised)`.
- **`try_activate`.** `a2a::activate(requested, advertised)`: the URI
  activated and the version it names.
- **`try_activate_extension`.** `a2a::activate` against an agent card that
  advertises every version this crate speaks. The reference harness advertises
  v0.8, v0.9 and v1.0.

`activate` only considers an extension that names a version the crate speaks,
as the reference does for its own versions. So the two cases whose expected
activation is the v0.8 extension are skipped as v0.8, not bent to another
version.

## Retired suites

Upstream replaced `core/validator.yaml` with one suite per protocol version,
under new case names. The code its cases cover is still in this crate, so the
file is kept as it stood at the previous pin, under `retired/`, and executed
whole: what passed there goes on being checked. It uses the earlier,
`snake_case` case vocabulary; the harness reads a key in either spelling. It
is superseded by `core/validator_v0_9.yaml` and can go once that is judged to
cover it.

Two more files of the previous pin were kept here until the code they covered
was removed, and went with it (#41): `core/catalog.yaml`, which upstream had
deleted (schema pruning, prompt rendering, example loading), and
`agent/inference_format.yaml` as it stood before upstream moved it to
`agent/legacy/`. Pruning lives on in the agent layer, where
`agent/catalog_transformer.yaml` and `agent/direct_json/prompt_generator.yaml`
are its suites.

## v0.9.1 regression coverage

The official schemas the crate ships are in `schemas/`, one directory per
version, from the commit this suite is vendored at (`schemas/README.md`).
`tests/author.rs` checks the actual
Draft 2020-12 engine, local refs, async recovery, targeted edits, and
multi-catalog transactional streams. `tests/protocol_091.rs` checks lifecycle,
null/removal/Undefined, snapshots, and fallible history replay.
