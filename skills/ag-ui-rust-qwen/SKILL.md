---
name: ag-ui-rust-qwen
description: "Use when running ag-ui-rust against a real model on Qwen Cloud (Alibaba Model Studio / DashScope, including the token-plan Individual Plan) — the live e2e tests, task-board's --llm voice, or an agent of your own built the way e2e/src/llm.rs is. UNCONVENTIONAL, and wrong from memory: the repo reads QWEN_API_KEY / QWEN_BASE_URL / QWEN_MODEL by name, AG_UI_LLM_* wins over them, the default endpoint is Gemini's and needs its own key, a token-plan endpoint does NOT serve `qwen-plus` (list `$QWEN_BASE_URL/models`), the live tests are #[ignore] and SKIP without a key rather than fail, and a 404/429 from the provider is a skip, not an SDK bug. Triggers on: qwen, Qwen Cloud, DashScope, Model Studio, token-plan, compatible-mode, QWEN_API_KEY, QWEN_BASE_URL, QWEN_MODEL, qwen3.8-flash, live_llm, live LLM test, --llm, 'run the live tests', 'which model', 'model not found', 'SKIPPED: no model answered'."
---

# Qwen Cloud with ag-ui-rust

Docs: <https://kimsoungryoul.github.io/ag-ui-rust/> · this skill is written against
workspace version **0.3.0**. If the repository disagrees with it, the repository is right
and the skill is stale — see `ag-ui-rust-update`.

The SDK depends on no LLM crate. Everything here that talks to a model is `reqwest` and
`serde` against an OpenAI-compatible endpoint, and Qwen Cloud's **compatible mode** is one.
The repository recognises it by name, so a contributor with a Qwen subscription needs no
Gemini key and no `AG_UI_LLM_*` variables.

## The three variables

```sh
export QWEN_API_KEY=sk-…                                                   # never commit this
export QWEN_BASE_URL=https://token-plan.ap-southeast-1.maas.aliyuncs.com/compatible-mode/v1
export QWEN_MODEL=qwen3.8-flash
```

| Variable | What it does | Default |
| --- | --- | --- |
| `QWEN_BASE_URL` | Picks Qwen Cloud. `/chat/completions` is appended; a trailing `/` is fine. | — (unset means "not Qwen") |
| `QWEN_API_KEY` | The bearer token for it. Required with `QWEN_BASE_URL`: Qwen Cloud is hosted. | — |
| `QWEN_MODEL` | The model id. | `qwen-plus` |

Which base URL is yours depends on the subscription:

- **Token plan / Individual Plan** — `https://token-plan.ap-southeast-1.maas.aliyuncs.com/compatible-mode/v1`
- **International (pay-as-you-go)** — `https://dashscope-intl.aliyuncs.com/compatible-mode/v1`
- **China mainland** — `https://dashscope.aliyuncs.com/compatible-mode/v1`

The console's *API keys* page shows the base URL next to the key it issues; copy that one.

### Precedence

`AG_UI_LLM_BASE_URL` wins outright when set, with `AG_UI_LLM_MODEL` and `AG_UI_LLM_API_KEY`
(a `QWEN_API_KEY` is accepted as the key there too). Failing that, `QWEN_BASE_URL` picks Qwen
with `QWEN_API_KEY` and `QWEN_MODEL`. Failing both, the default endpoint is Gemini's and needs
`AG_UI_LLM_API_KEY` or `GEMINI_API_KEY`. So a shell that exports the Qwen variables
permanently keeps working with the Ollama recipe in the docs, because that recipe sets
`AG_UI_LLM_BASE_URL` — but set `AG_UI_LLM_MODEL` alongside it, or the model falls back to the
Gemini default, not to `QWEN_MODEL`.

The selection is one function, `Endpoint::resolve` in `e2e/src/llm.rs`, and
`Voice::from_env` in `examples/task-board/src/llm.rs` makes the same choices.

## Pick a model your endpoint serves

`qwen-plus`, the default, is the standard DashScope name and a **token-plan endpoint does not
serve it**. Ask the endpoint rather than guessing:

```sh
curl -s -H "Authorization: Bearer $QWEN_API_KEY" "$QWEN_BASE_URL/models" \
  | python3 -c 'import sys,json; print([m["id"] for m in json.load(sys.stdin)["data"]])'
```

The Individual Plan lists these text models (September 2026): `qwen3.8-max`, `qwen3.8-flash`,
`qwen3.7-plus`, `qwen3.7-max`, `qwen3.6-flash`, `deepseek-v4-pro`, `deepseek-v4-pro-0813`,
`deepseek-v4-flash-0731`, `glm-5.2`. The rest of the listing is image, audio and video
models, which no chat endpoint here can use.

- **For tests and the example, `qwen3.8-flash`.** Fast, cheap, and it follows "reply with
  the single word: pong" and the tool-call round trip exactly. This is what the live suite
  was verified against.
- **For quality, `qwen3.7-plus` or `qwen3.8-max`.** Reasoning models; slower and pricier,
  and the reasoning arrives as ordinary text unless the endpoint is asked otherwise.

A model the endpoint does not serve answers `404`, which the live suite reports as *skipped*
("that model does not exist here") and task-board reports as a reasoning line before saying
the scripted sentence instead. Neither is an SDK failure.

## Run the live suite

Every live test is `#[ignore]`, so `cargo test` and CI never reach the network:

```sh
cargo test -p ag-ui-e2e --test live_llm -- --ignored --test-threads=1 --nocapture
```

Four requests when everything works — one for the text run, two for the tool round trip
(the call, then the answer to its result), one for the run delegated to a subagent — and the
output names what happened:

```text
live endpoint: https://token-plan.ap-southeast-1.maas.aliyuncs.com/compatible-mode/v1 | models: ["qwen3.8-flash"]
live delegated reply: ["pong"] under live-subagent-sub-1
live answer: "The weather in Seoul right now is clear with a temperature of 21°C."
live reply: "pong" in 5 events
test result: ok. 3 passed; 0 failed; 0 ignored
```

`--nocapture` is worth typing: a skip and the model's actual reply are printed, not asserted.
`--test-threads=1` keeps two runs from tripping a per-minute limit at once. A `SKIPPED:` line
names the variable that is missing or the reason every model refused — quota, capacity, a
model that does not exist here — and is not a failure. A `400`, or an agent error with no
provider status in it, **is** ours and fails loudly.

## The example with a model doing the talking

```sh
cargo run -p task-board -- serve --llm
printf 'research onboarding\nlist\n' | cargo run -p task-board -- chat
```

`serve --llm` prints the model and the endpoint (never the key) and rewrites the reply
sentence only — ids, counts, state and the two subagents' scripted sentences stay
deterministic, which is what keeps the transcripts in `tests/flows.rs` assertable. A model
that fails does not fail the run: the failure is said as a `~` reasoning line and the
scripted sentence goes out.

## Your own agent against Qwen

Copy the shape of `e2e/src/llm.rs` rather than adding an LLM crate: `Endpoint::from_env()` for
the configuration, a `reqwest` POST to `{base_url}/chat/completions` with `"stream": true`,
and the SSE frames mapped onto `ctx.assistant_message()` / `ctx.tool_call()` as they arrive.
`LlmAgent` there is the reference implementation, and `e2e/tests/live_llm.rs`'s `Delegating`
shows the same agent run through `ctx.subagent(..)` — nothing about it knows it was
delegated to, and every event it emits comes out attributed.

## Do not write

| Wrong | Why |
| --- | --- |
| `QWEN_MODEL=qwen-plus` against a token-plan URL | Not served there; a 404 that reads like a broken SDK. List `/models`. |
| `QWEN_BASE_URL` without `QWEN_API_KEY` | Qwen Cloud is hosted; `from_env` refuses with `MissingApiKey` rather than sending an unauthenticated request. |
| The key in `.envrc`, a test, a fixture, a commit | It is a bearer token to a paid plan. The repository reads it from the environment and prints only the endpoint. |
| `AG_UI_LLM_BASE_URL=$QWEN_BASE_URL` with no `AG_UI_LLM_MODEL` | The generic path takes the Gemini default model, not `QWEN_MODEL`. Either set both generic variables or use only the `QWEN_*` ones. |
| Treating `429` or `503` as a test failure | Quota and capacity are the provider's; the harness waits or skips, and says so. |

## Deeper

- `e2e/src/llm.rs` — `Endpoint`, `LlmAgent`, the streaming mapping, and the constants
- `e2e/tests/live_llm.rs` — the suite, the skip/retry policy, `Delegating`
- `examples/task-board/src/llm.rs` and its README, "Letting a model do the talking"
