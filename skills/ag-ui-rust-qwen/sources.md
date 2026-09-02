# Sources

What this skill was written from. A link goes stale silently; a path does not — when one of
these files changes, the section it feeds is the one to re-read.

This skill carries shell and text blocks only, so `e2e/src/skills.rs` lists it for
completeness rather than for compilation. The behaviour it describes is what the files below
implement and test.

## SKILL.md

- `e2e/src/llm.rs` — `QWEN_BASE_URL_ENV`, `QWEN_API_KEY_ENV`, `QWEN_MODEL_ENV`,
  `QWEN_DEFAULT_MODEL`, `Endpoint::resolve` and its unit tests (the precedence table), the
  `MissingApiKey` message, `LlmAgent`
- `examples/task-board/src/llm.rs` — `Voice::from_env`, the same precedence
- `e2e/tests/live_llm.rs` — the header's Qwen recipe, the four-request budget, the
  skip/retry table, `Delegating` and `a_delegated_answer_arrives_attributed_to_the_subagent`
- `examples/task-board/README.md`, "Letting a model do the talking" — the `--llm` recipe and
  the note on which models a token-plan endpoint serves
- The Qwen Cloud console's *Supported models* list for the Individual Plan, and the
  endpoint's own `/models` listing, both read on 2026-09-02
