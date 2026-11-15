# Vendored A2UI conformance suite

The A2UI project ships its conformance tests as language-agnostic YAML and asks
every SDK to run them: read the YAML, feed the inputs to that language's
implementation, assert the outputs. The suite is vendored here whole, at
upstream's current layout, and driven by `crates/ag-ui-a2ui/tests/conformance.rs`.

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
| `specification/v0_9/catalogs/basic/catalog.json` | A specification document `core/validator_v0_9.yaml` names by repository path. Copied from the same commit, at the same path relative to the repository root. |
| `retired/` | Suites as they stood at the previous pin, `44a420b67957fafc0b02d55a153fdaf72e32ffb5` (2026-08-14). See "Retired suites". |
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

A case with `steps` counts one check per step in the validator suites. The
streaming suite counts one check per case and verifies its steps in order.

Totals: 168 passed, 11 expected divergence, 1865 skipped, 3 failing.

| Suite | Cases | Passed | Expected divergence | Skipped | Failing | Standing |
| `core/validator_v0_9.yaml` | 38 | 39 | 3 | 5 | 0 | executed |
| `agent/legacy/parser.yaml` | 23 | 22 | 0 | 0 | 1 | executed |
| `agent/legacy/inference_format.yaml` | 22 | 12 | 0 | 10 | 0 | executed |
| `agent/legacy/streaming_parser.yaml` | 88 | 33 | 8 | 45 | 2 | executed |
| `retired/core/validator.yaml` | 45 | 26 | 0 | 25 | 0 | executed; retired upstream |
| `retired/core/catalog.yaml` | 24 | 23 | 0 | 1 | 0 | executed; retired upstream |
| `retired/agent/inference_format.yaml` | 19 | 13 | 0 | 6 | 0 | executed; retired upstream |
| `core/message_processor_v0_9.yaml` | 48 | 0 | 0 | 48 | 0 | no runner yet: the surface state machine, driven as a renderer's MessageProcessor: catalog registry, rendererError payloads |
| `core/data_model.yaml` | 52 | 0 | 0 | 52 | 0 | no runner yet: data-model reads, writes and deletion, with a renderer's observer notifications |
| `core/data_context.yaml` | 11 | 0 | 0 | 11 | 0 | no runner yet: resolving a relative path against a scope; the crate has no public entry point that returns the absolute path |
| `core/expressions.yaml` | 98 | 0 | 0 | 98 | 0 | no runner yet: the formatString template parser; the crate evaluates a template and exposes no parse |
| `core/catalog.yaml` | 73 | 0 | 0 | 73 | 0 | no runner yet: the reference catalog API: fromJson, toJson, round trip, validationSchema, derive |
| `core/common_types.yaml` | 22 | 0 | 0 | 22 | 0 | no runner yet: values against the published common types; 3 cases compare a schema generated from an SDK's own types |
| `core/validator_v1_0.yaml` | 49 | 0 | 0 | 49 | 0 | later: A2UI v1.0 (#42): envelope, schema and topology validation of v1.0 messages |
| `core/message_processor_v1_0.yaml` | 84 | 0 | 0 | 84 | 0 | later: A2UI v1.0 (#42): the v1.0 message processor |
| `core/message_processor_scaling.yaml` | 3 | 0 | 0 | 3 | 0 | later: A2UI v1.0 (#42): timing of the v1.0 message processor |
| `core/data_deletion.yaml` | 7 | 0 | 0 | 7 | 0 | later: A2UI v1.0 (#42): updateDataModel removal by null |
| `core/data_model_pointers.yaml` | 7 | 0 | 0 | 7 | 0 | later: A2UI v1.0 (#42): data-model pointer rules in v1.0 messages |
| `core/reserved_keys.yaml` | 10 | 0 | 0 | 10 | 0 | later: A2UI v1.0 (#42): @-prefixed reserved keys |
| `core/multi_catalog.yaml` | 8 | 0 | 0 | 8 | 0 | later: A2UI v1.0 (#42): several catalogs on one surface |
| `core/composition_constraints.yaml` | 10 | 0 | 0 | 10 | 0 | later: A2UI v1.0 (#42): allowedParents and allowedChildren through the v1.0 processor |
| `core/index_function.yaml` | 4 | 0 | 0 | 4 | 0 | later: A2UI v1.0 (#42): the @index function in v1.0 templates |
| `core/validation_result.yaml` | 6 | 0 | 0 | 6 | 0 | later: A2UI v1.0 (#42): per-component validation results |
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
| `agent/request_processor.yaml` | 29 | 0 | 0 | 29 | 0 | later: agent layer (#41): building an inference format and a per-request processor |
| `agent/catalog_resolution.yaml` | 23 | 0 | 0 | 23 | 0 | later: agent layer (#41): negotiating renderer capabilities against registered catalogs |
| `agent/catalog_provider.yaml` | 10 | 0 | 0 | 10 | 0 | later: agent layer (#41): loading catalog documents from the filesystem |
| `agent/catalog_transformer.yaml` | 15 | 0 | 0 | 15 | 0 | later: agent layer (#41): pruning a catalog before it reaches a prompt or a validator |
| `agent/renderer_capabilities.yaml` | 24 | 0 | 0 | 24 | 0 | later: agent layer (#41): reading renderer capabilities from transport metadata |
| `agent/renderer_state.yaml` | 9 | 0 | 0 | 9 | 0 | later: agent layer (#41): renderer state across turns |
| `agent/skill.yaml` | 12 | 0 | 0 | 12 | 0 | later: agent layer (#41): skill documents generated from a format and its catalogs |
| `agent/streamed_answer.yaml` | 6 | 0 | 0 | 6 | 0 | later: agent layer (#41): reading a streamed answer into a message processor |
| `agent/direct_json/prompt_generator.yaml` | 15 | 0 | 0 | 15 | 0 | later: agent layer (#41): the direct JSON format: system prompt snippet |
| `agent/direct_json/response_parser.yaml` | 36 | 0 | 0 | 36 | 0 | later: agent layer (#41): the direct JSON format: unwrapping and parsing a response |
| `agent/direct_json/response_streaming.yaml` | 139 | 0 | 0 | 139 | 0 | later: agent layer (#41): the direct JSON format: parsing a response chunk by chunk |
| `agent/direct_json/compiler.yaml` | 38 | 0 | 0 | 38 | 0 | later: agent layer (#41): the direct JSON format: compiling a block into messages |
| `agent/direct_json/decompiler.yaml` | 11 | 0 | 0 | 11 | 0 | later: agent layer (#41): the direct JSON format: writing messages back as notation |
| `agent/direct_json/scaling.yaml` | 7 | 0 | 0 | 7 | 0 | later: agent layer (#41): the direct JSON format: parse time against payload size |
| `agent/express/prompt_generator.yaml` | 23 | 0 | 0 | 23 | 0 | later: agent layer (#41): the Express format: system prompt snippet |
| `agent/express/response_parser.yaml` | 38 | 0 | 0 | 38 | 0 | later: agent layer (#41): the Express format: unwrapping and parsing a response |
| `agent/express/response_streaming.yaml` | 41 | 0 | 0 | 41 | 0 | later: agent layer (#41): the Express format: parsing a response chunk by chunk |
| `agent/express/compiler.yaml` | 135 | 0 | 0 | 135 | 0 | later: agent layer (#41): the Express format: compiling a block into messages |
| `agent/express/decompiler.yaml` | 49 | 0 | 0 | 49 | 0 | later: agent layer (#41): the Express format: writing messages back as notation |
| `agent/express/scaling.yaml` | 1 | 0 | 0 | 1 | 0 | later: agent layer (#41): the Express format: compile time against payload size |
| `agent/atom/compiler.yaml` | 7 | 0 | 0 | 7 | 0 | later: agent layer (#41): the Atom format |
| `agent/elemental/compiler.yaml` | 8 | 0 | 0 | 8 | 0 | later: agent layer (#41): the Elemental format |
| `agent/builder/builder.yaml` | 22 | 0 | 0 | 22 | 0 | later: agent layer (#41): the typesafe builder API |
| `agent/macros/macros.yaml` | 70 | 0 | 0 | 70 | 0 | later: agent layer (#41): builder macros |
| `agent/multi_catalog_formats.yaml` | 35 | 0 | 0 | 35 | 0 | later: A2UI v1.0 (#42): components and function calls resolved across several active catalogs |
| `agent/agent_functions.yaml` | 64 | 0 | 0 | 64 | 0 | later: A2UI v1.0 (#42): agent functions and the agent's side of renderer messages |
| `agent/renderer_function_calls.yaml` | 30 | 0 | 0 | 30 | 0 | later: A2UI v1.0 (#42): the agent calling renderer functions |
| `e2e/v0_9_1/login_form_action.yaml` | 1 | 0 | 0 | 1 | 0 | later: agent layer (#41): a v0.9.1 transcript through the agent layer's response parser |
| `e2e/v0_9_1/missing_surface_error.yaml` | 1 | 0 | 0 | 1 | 0 | later: agent layer (#41): a v0.9.1 transcript through the agent layer's response parser |
| `e2e/v0_9_1/newsletter_signup.yaml` | 1 | 0 | 0 | 1 | 0 | later: agent layer (#41): a v0.9.1 transcript through the agent layer's response parser |
| `e2e/v0_9_1/restaurant_list_booking.yaml` | 1 | 0 | 0 | 1 | 0 | later: agent layer (#41): a v0.9.1 transcript through the agent layer's response parser |
| `e2e/login_form_action.yaml` | 1 | 0 | 0 | 1 | 0 | later: A2UI v1.0 (#42): a v1.0 transcript |
| `e2e/missing_surface_error.yaml` | 1 | 0 | 0 | 1 | 0 | later: A2UI v1.0 (#42): a v1.0 transcript |
| `e2e/newsletter_signup.yaml` | 1 | 0 | 0 | 1 | 0 | later: A2UI v1.0 (#42): a v1.0 transcript |
| `e2e/restaurant_list_booking.yaml` | 1 | 0 | 0 | 1 | 0 | later: A2UI v1.0 (#42): a v1.0 transcript |
| `adapter/adapter_conformance_v0_9.yaml` | 16 | 0 | 0 | 16 | 0 | out of scope: renderer: rendered view hierarchies in the UI framework adapters |
| `adapter/markdown_subset.yaml` | 7 | 0 | 0 | 7 | 0 | out of scope: renderer: rendering the Markdown subset |
| `extensions/a2a/a2a_integration.yaml` | 11 | 0 | 0 | 11 | 0 | later: adapters (#43): A2A events and parts |
| `extensions/adk/adk_extensions.yaml` | 8 | 0 | 0 | 8 | 0 | not applicable: integration with Google's ADK, which this workspace does not target |
| `core/validator_v0_9.yaml` | 38 | 39 | 3 | 5 | 0 | executed |
| `agent/legacy/parser.yaml` | 23 | 22 | 0 | 0 | 1 | executed |
| `agent/legacy/inference_format.yaml` | 22 | 12 | 0 | 10 | 0 | executed |
| `agent/legacy/streaming_parser.yaml` | 88 | 33 | 8 | 45 | 2 | executed |
| `retired/core/validator.yaml` | 45 | 26 | 0 | 25 | 0 | executed; retired upstream |
| `retired/core/catalog.yaml` | 24 | 23 | 0 | 1 | 0 | executed; retired upstream |
| `retired/agent/inference_format.yaml` | 19 | 13 | 0 | 6 | 0 | executed; retired upstream |
| `core/message_processor_v0_9.yaml` | 48 | 0 | 0 | 48 | 0 | no runner yet: the surface state machine, driven as a renderer's MessageProcessor: catalog registry, rendererError payloads |
| `core/data_model.yaml` | 52 | 0 | 0 | 52 | 0 | no runner yet: data-model reads, writes and deletion, with a renderer's observer notifications |
| `core/data_context.yaml` | 11 | 0 | 0 | 11 | 0 | no runner yet: resolving a relative path against a scope; the crate has no public entry point that returns the absolute path |
| `core/expressions.yaml` | 98 | 0 | 0 | 98 | 0 | no runner yet: the formatString template parser; the crate evaluates a template and exposes no parse |
| `core/catalog.yaml` | 73 | 0 | 0 | 73 | 0 | no runner yet: the reference catalog API: fromJson, toJson, round trip, validationSchema, derive |
| `core/common_types.yaml` | 22 | 0 | 0 | 22 | 0 | no runner yet: values against the published common types; 3 cases compare a schema generated from an SDK's own types |
| `core/validator_v1_0.yaml` | 49 | 0 | 0 | 49 | 0 | later: A2UI v1.0 (#42): envelope, schema and topology validation of v1.0 messages |
| `core/message_processor_v1_0.yaml` | 84 | 0 | 0 | 84 | 0 | later: A2UI v1.0 (#42): the v1.0 message processor |
| `core/message_processor_scaling.yaml` | 3 | 0 | 0 | 3 | 0 | later: A2UI v1.0 (#42): timing of the v1.0 message processor |
| `core/data_deletion.yaml` | 7 | 0 | 0 | 7 | 0 | later: A2UI v1.0 (#42): updateDataModel removal by null |
| `core/data_model_pointers.yaml` | 7 | 0 | 0 | 7 | 0 | later: A2UI v1.0 (#42): data-model pointer rules in v1.0 messages |
| `core/reserved_keys.yaml` | 10 | 0 | 0 | 10 | 0 | later: A2UI v1.0 (#42): @-prefixed reserved keys |
| `core/multi_catalog.yaml` | 8 | 0 | 0 | 8 | 0 | later: A2UI v1.0 (#42): several catalogs on one surface |
| `core/composition_constraints.yaml` | 10 | 0 | 0 | 10 | 0 | later: A2UI v1.0 (#42): allowedParents and allowedChildren through the v1.0 processor |
| `core/index_function.yaml` | 4 | 0 | 0 | 4 | 0 | later: A2UI v1.0 (#42): the @index function in v1.0 templates |
| `core/validation_result.yaml` | 6 | 0 | 0 | 6 | 0 | later: A2UI v1.0 (#42): per-component validation results |
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
| `agent/request_processor.yaml` | 29 | 0 | 0 | 29 | 0 | later: agent layer (#41): building an inference format and a per-request processor |
| `agent/catalog_resolution.yaml` | 23 | 0 | 0 | 23 | 0 | later: agent layer (#41): negotiating renderer capabilities against registered catalogs |
| `agent/catalog_provider.yaml` | 10 | 0 | 0 | 10 | 0 | later: agent layer (#41): loading catalog documents from the filesystem |
| `agent/catalog_transformer.yaml` | 15 | 0 | 0 | 15 | 0 | later: agent layer (#41): pruning a catalog before it reaches a prompt or a validator |
| `agent/renderer_capabilities.yaml` | 24 | 0 | 0 | 24 | 0 | later: agent layer (#41): reading renderer capabilities from transport metadata |
| `agent/renderer_state.yaml` | 9 | 0 | 0 | 9 | 0 | later: agent layer (#41): renderer state across turns |
| `agent/skill.yaml` | 12 | 0 | 0 | 12 | 0 | later: agent layer (#41): skill documents generated from a format and its catalogs |
| `agent/streamed_answer.yaml` | 6 | 0 | 0 | 6 | 0 | later: agent layer (#41): reading a streamed answer into a message processor |
| `agent/direct_json/prompt_generator.yaml` | 15 | 0 | 0 | 15 | 0 | later: agent layer (#41): the direct JSON format: system prompt snippet |
| `agent/direct_json/response_parser.yaml` | 36 | 0 | 0 | 36 | 0 | later: agent layer (#41): the direct JSON format: unwrapping and parsing a response |
| `agent/direct_json/response_streaming.yaml` | 139 | 0 | 0 | 139 | 0 | later: agent layer (#41): the direct JSON format: parsing a response chunk by chunk |
| `agent/direct_json/compiler.yaml` | 38 | 0 | 0 | 38 | 0 | later: agent layer (#41): the direct JSON format: compiling a block into messages |
| `agent/direct_json/decompiler.yaml` | 11 | 0 | 0 | 11 | 0 | later: agent layer (#41): the direct JSON format: writing messages back as notation |
| `agent/direct_json/scaling.yaml` | 7 | 0 | 0 | 7 | 0 | later: agent layer (#41): the direct JSON format: parse time against payload size |
| `agent/express/prompt_generator.yaml` | 23 | 0 | 0 | 23 | 0 | later: agent layer (#41): the Express format: system prompt snippet |
| `agent/express/response_parser.yaml` | 38 | 0 | 0 | 38 | 0 | later: agent layer (#41): the Express format: unwrapping and parsing a response |
| `agent/express/response_streaming.yaml` | 41 | 0 | 0 | 41 | 0 | later: agent layer (#41): the Express format: parsing a response chunk by chunk |
| `agent/express/compiler.yaml` | 135 | 0 | 0 | 135 | 0 | later: agent layer (#41): the Express format: compiling a block into messages |
| `agent/express/decompiler.yaml` | 49 | 0 | 0 | 49 | 0 | later: agent layer (#41): the Express format: writing messages back as notation |
| `agent/express/scaling.yaml` | 1 | 0 | 0 | 1 | 0 | later: agent layer (#41): the Express format: compile time against payload size |
| `agent/atom/compiler.yaml` | 7 | 0 | 0 | 7 | 0 | later: agent layer (#41): the Atom format |
| `agent/elemental/compiler.yaml` | 8 | 0 | 0 | 8 | 0 | later: agent layer (#41): the Elemental format |
| `agent/builder/builder.yaml` | 22 | 0 | 0 | 22 | 0 | later: agent layer (#41): the typesafe builder API |
| `agent/macros/macros.yaml` | 70 | 0 | 0 | 70 | 0 | later: agent layer (#41): builder macros |
| `agent/multi_catalog_formats.yaml` | 35 | 0 | 0 | 35 | 0 | later: A2UI v1.0 (#42): components and function calls resolved across several active catalogs |
| `agent/agent_functions.yaml` | 64 | 0 | 0 | 64 | 0 | later: A2UI v1.0 (#42): agent functions and the agent's side of renderer messages |
| `agent/renderer_function_calls.yaml` | 30 | 0 | 0 | 30 | 0 | later: A2UI v1.0 (#42): the agent calling renderer functions |
| `e2e/v0_9_1/login_form_action.yaml` | 1 | 0 | 0 | 1 | 0 | later: agent layer (#41): a v0.9.1 transcript through the agent layer's response parser |
| `e2e/v0_9_1/missing_surface_error.yaml` | 1 | 0 | 0 | 1 | 0 | later: agent layer (#41): a v0.9.1 transcript through the agent layer's response parser |
| `e2e/v0_9_1/newsletter_signup.yaml` | 1 | 0 | 0 | 1 | 0 | later: agent layer (#41): a v0.9.1 transcript through the agent layer's response parser |
| `e2e/v0_9_1/restaurant_list_booking.yaml` | 1 | 0 | 0 | 1 | 0 | later: agent layer (#41): a v0.9.1 transcript through the agent layer's response parser |
| `e2e/login_form_action.yaml` | 1 | 0 | 0 | 1 | 0 | later: A2UI v1.0 (#42): a v1.0 transcript |
| `e2e/missing_surface_error.yaml` | 1 | 0 | 0 | 1 | 0 | later: A2UI v1.0 (#42): a v1.0 transcript |
| `e2e/newsletter_signup.yaml` | 1 | 0 | 0 | 1 | 0 | later: A2UI v1.0 (#42): a v1.0 transcript |
| `e2e/restaurant_list_booking.yaml` | 1 | 0 | 0 | 1 | 0 | later: A2UI v1.0 (#42): a v1.0 transcript |
| `adapter/adapter_conformance_v0_9.yaml` | 16 | 0 | 0 | 16 | 0 | out of scope: renderer: rendered view hierarchies in the UI framework adapters |
| `adapter/markdown_subset.yaml` | 7 | 0 | 0 | 7 | 0 | out of scope: renderer: rendering the Markdown subset |
| `extensions/a2a/a2a_integration.yaml` | 11 | 0 | 0 | 11 | 0 | later: adapters (#43): A2A events and parts |
| `extensions/adk/adk_extensions.yaml` | 8 | 0 | 0 | 8 | 0 | not applicable: integration with Google's ADK, which this workspace does not target |

Run

```
cargo test -p ag-ui-a2ui --all-features --test conformance -- --nocapture
```

to see the report, and set `A2UI_CONFORMANCE_VERBOSE=1` to list every check by
name.

## Why suites are skipped

| Label | Suites | Cases | What it means |
|---|---:|---:|---|
| later: agent layer (#41) | 28 | 772 | The suite tests the agent layer: inference formats, the per-request processor, catalog resolution, renderer capabilities and state. The next pull request of #41 builds it. |
| later: A2UI v1.0 (#42) | 19 | 397 | The suite tests A2UI v1.0. |
| later: adapters (#43) | 1 | 11 | The suite tests the A2A transport adapter. |
| no runner yet | 6 | 304 | The suite tests the pre-1.0 core this crate implements, and this harness has no runner for its action. Nothing later is planned to add one; see below. |
| out of scope: renderer | 8 | 241 | The suite tests a renderer: drawing, evaluating functions, dispatching actions, reporting errors back. This crate does not render. |
| out of scope: v0.8 | 2 | 36 | The suite tests the v0.8 wire format. This crate speaks v0.9 and v0.9.1. |
| not applicable | 2 | 12 | The suite asks something of an SDK that does not apply here: a schema generated from the SDK's own types, an integration with Google's ADK. |

Upstream's `adapter/` tier is its UI framework adapters (`@a2ui/lit`, React,
Angular, Flutter, SwiftUI), that is, renderers. The suites about transports are
under `extensions/`.

### The six suites with no runner yet

These are in scope: they test the same core this crate implements, at the
versions it speaks. They are not run because each needs a runner for an action
this harness does not have, and in most cases an entry point the crate does not
expose.

| Suite | What a runner would need |
|---|---|
| `core/message_processor_v0_9.yaml` | A facade over `SurfaceStore` with a catalog registry, and the `rendererError` payload a renderer would send for each rejection. The lifecycle cases map onto `SurfaceStore::apply` directly. |
| `core/data_model.yaml` | Reads, writes and deletes map onto `DataModel`; the observer notifications half the steps assert are a renderer's. |
| `core/data_context.yaml` | A public function that returns the absolute path a relative one resolves to. `binding::Scope` resolves a value, not a path. |
| `core/expressions.yaml` | A parse of a `formatString` template into parts. `Scope::format_string` evaluates one. |
| `core/catalog.yaml` | 17 of the 73 cases state v0.9 or v0.9.1. `Catalog::from_schema` covers `from_json`; there is no `toJson`, `validationSchema` or `derive`. |
| `core/common_types.yaml` | 19 cases validate a value against a published common type; 3 compare a schema generated from an SDK's own types, which this crate does not generate. |

## Expected divergences

Eleven checks: three in `core/validator_v0_9.yaml` and eight in
`agent/legacy/streaming_parser.yaml`.

| Checks | Reference expectation | Checked behaviour |
|---:|---|---|
| 2 (`test_v09_topology_orphaned_component_error`, `test_v09_topology_plain_string_property_not_treated_as_child_ref`) | A component the root cannot reach is an error. | It is reported in `ValidationReport::unreachable`, as a warning. The specification tells renderers to buffer a component until its parent arrives, so an unreachable component is usually a half-streamed tree. |
| 1 (`test_custom_catalog_0_9`, step 2) | `children: 123` is `invalid_value`. | It is `type_mismatch`, at the same place. |
| 1 (`test_delta_streaming_correctness_v09`) | Emit components from a message that has not declared the required `version`. | Emit nothing until a supported version is known. |
| 6 (`test_incremental_data_model_streaming_v09` and five `test_sniff_partial_*_v09` cases) | Emit a root data-model update while the optional `path` field could still arrive after `value`. | Emit nothing until an explicit `path` is known or the message completes. |
| 1 (`test_cut_unicode_multibyte_emoji_v09`) | Emit text cut after the first half of a surrogate pair. | Emit nothing until the pair is whole: half a pair is not a string in Rust. The suite writes that half as a YAML escape, which is not valid YAML either; the harness reads it as U+FFFD and pins the case. |

The `test_sniff_partial_datamodel_with_cut_key_v09` fixture never becomes valid
JSON: its final chunk lacks a closing brace for the message. The harness checks
that the fixture remains malformed and that no speculative update is emitted.

Two divergences of the previous pin are gone. `test_partial_children_lists_v09`
and `test_sniff_partial_component_discards_empty_children_dict_v09` declared
`children` as plain strings, which this crate keeps as data where the reference
toolkit rewrote them by property name. Upstream has since changed both fixtures
to declare `children` as a `ChildList`, and both now pass as written.

## Failing

Three checks, all in cases upstream added after the previous pin, all about the
toolkit's parsers. The agent layer replaces those parsers, and these cases are
for it to pass.

| Check | What happens |
|---|---|
| `agent/legacy/parser.yaml::test_fix_payload_unescaped_latex_backslashes_diagnostic_hint` | The payload is rejected, with the JSON parser's message. The case wants the hint "Unescaped backslash found". |
| `agent/legacy/streaming_parser.yaml::test_no_placeholders_without_placeholder_component_v09` | With validation off, a `Row` placeholder is emitted for a catalog that defines no `Row`. The case wants nothing emitted until the children arrive. |
| `agent/legacy/streaming_parser.yaml::test_cut_escape_sequences_v09` | The fixture writes a second `<a2ui-json>` inside a block that is still open. It is read as JSON, and the messages after it are not dispatched. |

## Skips inside the executed suites

| Count | Reason |
|---:|---|
| 39 + 25 | **v0.8 wire format.** v0.8 nests component properties under the type name and uses different message names. 39 cases of `agent/legacy/streaming_parser.yaml`, 25 checks of `retired/core/validator.yaml`. |
| 6 + 4 | **A2UI v1.0** (#42). 6 cases of `agent/legacy/streaming_parser.yaml`, 4 of `agent/legacy/inference_format.yaml`. |
| 4 + 4 | **Superseded toolkit policy.** Default fallback and cross-ID inline merging conflict with the reviewed contract. `toolkit::negotiate` tests enforce explicit matching, complete inline documents, and conflict rejection. The same four cases in the current and the retired `inference_format.yaml`. |
| 2 + 2 | **v0.8 schema bundle in a prompt.** Two `generate_prompt` cases ask for the v0.8 schema documents to be embedded in the prompt; this crate ships v0.9. In both `inference_format.yaml` files. |
| 5 | **A JSON Schema failure** in `core/validator_v0_9.yaml`: an undeclared property, an undeclared function, a function argument of the wrong type, and two theme values. The semantic validator the harness runs does not evaluate these; `SchemaValidator`, behind the `schema-validation` feature, does. They are listed by name in `SCHEMA_ONLY`, because their fixtures are also invalid in ways the semantic validator does report, and would otherwise pass for the wrong reason. |
| 1 | **JSON Schema validation of an example file.** `test_load_examples_validation_fails_on_schema_error`, in `retired/core/catalog.yaml`, asks `load_examples` to validate each example against an arbitrary schema. |

## How the validator cases are run

`core/validator_v0_9.yaml` runs each case through one validator and one surface
state across its steps, so a later step is checked against the surfaces earlier
steps left (`Validator::validate_json_updates`). A step that fails changes
nothing. Three things about the comparison:

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
  error at all would satisfy it: two such steps are mapped by hand to the
  failure their description names (`CATEGORY_ONLY`), and the rest are skipped.
  The `rendererError` a renderer would send is not checked; this crate is not
  one.

## Retired suites

Upstream deleted `core/catalog.yaml` (schema pruning, prompt rendering, example
loading) and replaced `core/validator.yaml` with one suite per protocol version,
under new case names. `agent/inference_format.yaml` moved to `agent/legacy/` and
lost two cases on the way. The code those cases cover is still in this crate, so
the three files are kept as they stood at the previous pin and executed whole:
what passed there goes on being checked. They use the earlier, `snake_case` case
vocabulary; the harness reads a key in either spelling.

`retired/core/validator.yaml` is superseded by `core/validator_v0_9.yaml` and can
go once that is judged to cover it. `retired/core/catalog.yaml` and
`retired/agent/inference_format.yaml` go with the toolkit code they test.

The other three suites of the previous pin are not retired: `agent/parser.yaml`
and `agent/streaming_parser.yaml` moved to `agent/legacy/` with every case they
had, and `core/accessibility.yaml` is still in place.

## v0.9.1 regression coverage

The full official schemas are pinned separately in `schemas/v0_9_1/` at
`1c45c809b655878d06e3afc6dda22100afecc0a4`. `tests/author.rs` checks the actual
Draft 2020-12 engine, local refs, async recovery, targeted edits, and
multi-catalog transactional streams. `tests/protocol_091.rs` checks lifecycle,
null/removal/Undefined, snapshots, and fallible history replay.
