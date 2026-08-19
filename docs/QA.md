# QA strategy

Two tiers, because they answer different questions.

| Tier | What it proves | Runs |
| --- | --- | --- |
| Deterministic E2E | The protocol plumbing is correct: full event ordering, state deltas, and the human-in-the-loop round trip, driven over real HTTP by `ag_ui::client` against a real axum server. Plus the LLM mapping itself, driven from recorded SSE frames. | Always. CI gate. |
| Live smoke | A real streaming LLM is reachable and maps onto AG-UI events correctly, and the SDK genuinely depends on no LLM crate. | `#[ignore]`, only when a key or a local endpoint is configured. Not a CI gate. |

The deterministic tier uses a scripted mock agent and recorded model frames, so it is fast and
cannot flake. The live tier is excluded from the CI gate on purpose — see the rate limits below.

**The deterministic tier is the one that protects the mapping.** The live test cannot run in CI, so
every parsing and accumulation rule below is also covered by unit tests in `e2e/src/llm.rs` driven
from captured or synthetic frames. The live test only proves the wire is reachable.

## The live tier speaks OpenAI, not any vendor's own dialect

`e2e/src/llm.rs` talks to any OpenAI-compatible `POST {base}/chat/completions`. That is the one
shape nearly everything serves: Gemini's compatibility endpoint, Ollama, llama.cpp, LM Studio,
vLLM, Groq, Together.

It did not start that way. It spoke Gemini's native `:streamGenerateContent`, and being bound to
one vendor cost a day: the free tier ran out, the harness fell back to a sibling model, the sibling
was a 3.x model that requires thought signatures in tool loops, and the run died on a `400` that
was invisible until the fallback fired. The appendix keeps what that cost, because it was measured
rather than documented.

### Configuration

| Variable | Default | Meaning |
| --- | --- | --- |
| `AG_UI_LLM_BASE_URL` | `https://generativelanguage.googleapis.com/v1beta/openai` | `/chat/completions` is appended to this |
| `AG_UI_LLM_MODEL` | `gemini-2.5-flash-lite` | Model id |
| `AG_UI_LLM_API_KEY` | falls back to `GEMINI_API_KEY` | Bearer token |

Rules that hold everywhere in the harness:

- The key goes in the `Authorization: Bearer` **header**, never a query parameter — query strings
  end up in logs. It is never printed, not partially, not in an error, not in a `Debug` dump;
  `LlmAgent`'s `Debug` impl is hand-written to redact it, and there is a test for that.
- With no key at all the live tests **skip**, naming the variable. They never fail for it.
- A local server needs no key, and **absent must stay absent**: an empty `Authorization: Bearer` is
  a rejected request, not an anonymous one. So a missing key is only an error when the endpoint is
  the default one.

### Mapping OpenAI-compatible SSE onto AG-UI events

| Wire | AG-UI | Note |
| --- | --- | --- |
| `choices[0].delta.content` | `TEXT_MESSAGE_CONTENT.delta` | Arrives incrementally across frames. |
| `id` | `messageId` | The completion id, stable for the whole stream, so it identifies the message directly. |
| `choices[0].delta.tool_calls[]` | `TOOL_CALL_START` + `ARGS` + `END` | Accumulated first, emitted whole — see below. |
| `tool_calls[].function.arguments` | `TOOL_CALL_ARGS.delta` | **Partial JSON, concatenated across frames.** Already a string on both sides, so it passes through untouched. |
| `tool_calls[].id` | `toolCallId` | **Supplied by the server.** Used as-is; one is synthesized only for a server that sends none. |
| `tool_calls[].extra_content` | — (never leaves the agent) | Opaque provider extension, echoed back on the call it arrived on. See the appendix. |
| `data: [DONE]` | end of stream | A real sentinel, unlike the native API, which just EOFs. |
| `finish_reason` | — | Informational here; `[DONE]` or EOF is what ends the loop. |

Tool definitions go the other way and are **not translated at all**: an AG-UI `Tool` already
carries ordinary lowercase JSON Schema, which is exactly what this endpoint wants. The native
dialect wanted uppercase type names (`OBJECT`, `STRING`) and an OpenAPI keyword subset that
rejected `$schema` and `additionalProperties`, so this used to be a recursive translation with a
keyword whitelist. That code is gone.

Tool results are simpler too: a `role: "tool"` message is matched to its call by `tool_call_id`,
which AG-UI carries on the tool message already. The native dialect matched by *name*, which meant
indexing the assistant's calls on the way past to recover one.

#### Partial arguments are the difference that bites

This is the single biggest behavioural difference from the native dialect, where a `functionCall`
arrives atomically and fully formed. Here, arguments are a byte stream. A fragment can end
anywhere — mid-string, or between a backslash and the character it escapes — so **nothing may
parse a fragment on its own**, and the pieces must be concatenated in arrival order.

#### `tool_calls[].index` is not reliably present

The OpenAI streaming format keys parallel calls by `tool_calls[].index`, and OpenAI, Ollama and
Groq send it. **Gemini's compatibility endpoint does not send it at all.** Captured from the wire:

```json
"tool_calls":[{"function":{"arguments":"{\"city\":\"Seoul\"}","name":"get_weather"},
               "id":"function-call-7026415214984972976","type":"function"},
              {"function":{"arguments":"{\"city\":\"Oslo\"}","name":"get_weather"},
               "id":"function-call-7026415214984972901","type":"function"}]
```

Defaulting a missing `index` to `0` concatenates those two into `{"city":"Seoul"}{"city":"Oslo"}` —
one call with unparseable arguments. A 3.x model makes it worse by putting each parallel call in
its own frame, so array position is `0` for both as well.

So the accumulator resolves a fragment's slot by `index`, then by `id`, then by array position, and
the three cascade rather than being exclusive. There is a regression test for each path.

#### Calls are emitted whole, not streamed

Text streams as it arrives. Tool calls accumulate and are emitted after the message closes, as
`START` → one `ARGS` → `END`.

That is forced by the emitter's typestate design, not by laziness: a handle borrows the run context
mutably, so two open `ToolCallHandle`s at once is a borrow-check error *by design*
(see `docs/DESIGN.md`). Parallel calls arrive interleaved by `index`, so streaming them as they
arrive would need exactly that. Accumulating first is the only mapping that keeps interleaved
arguments from being spliced into each other.

#### Framing

Watch the terminator: it differs **between endpoints of the same vendor**. Gemini's native SSE ends
frames with `\r\n\r\n`; its OpenAI-compatible endpoint ends them with `\n\n`. A decoder that scans
for only one never finds a boundary, buffers the whole response, and emits everything at EOF —
which reads as "streaming does not work" rather than as a parse error. The decoder accepts `\n\n`,
`\r\n\r\n` and `\r\r`, as the SSE spec allows.

Two more, both tested:

- An empty or contentless final frame — `{"content": null}` next to `finish_reason`, or a
  usage-only frame with no `delta` at all — must **not** become an empty `TEXT_MESSAGE_CONTENT`.
- A stream can end at `[DONE]` *or* at EOF. Both happen; a body cut short by a proxy still has to
  end the loop rather than hang, and anything after `[DONE]` must not be read.

### Retry and skip policy

This suite talks to someone else's capacity-constrained service, so most of the ways it can
not-work say nothing about this SDK. A `503` reported as a test failure costs somebody an hour
looking for a bug that is not there.

| What came back | What happens |
| --- | --- |
| A stream | Asserted on, loudly. This is the point of the file. |
| `429` naming a **per-minute** quota | Wait for `RetryInfo.retryDelay`, ask again |
| `500`, `502`, `503`, `504` | Transient by definition — back off and retry, then try the next model |
| `429` naming a **per-day** quota | Cannot be waited out; move to the next model |
| `404` | That model does not exist here; move to the next |
| Nothing answered on the socket | **Skip** — the endpoint is not up |
| No model left | **Skip**, naming which model failed how |
| Anything else | **Fail** — a `400` or an agent error is ours |

**Failure is reserved for a model that answered and was mapped wrongly.** Everything else is a skip
that names the reason. Runs are serialized (`--test-threads=1` *and* an in-process mutex), because
parallel tests trip a per-minute limit immediately.

## Pointing the harness at a local model

Nothing here is rate limited, no key is involved, and the run costs nothing — which makes this the
better way to work on the mapping.

```sh
ollama serve && ollama pull qwen3:4b
export AG_UI_LLM_BASE_URL=http://localhost:11434/v1
export AG_UI_LLM_MODEL=qwen3:4b
cargo test -p ag-ui-e2e --test live_llm -- --ignored --test-threads=1 --nocapture
```

LM Studio serves the same API on `http://localhost:1234/v1`; llama.cpp's `llama-server` and vLLM
both do too. Notes that apply to all of them:

- **Do not set a key.** A missing key on a non-default base URL is sent as absent, which is what
  these servers want.
- Model fallback is switched off when the base URL is not the default: a local server has the one
  model you loaded, and there is no per-model quota to route around.
- Pick a model that actually supports tool calling, or the tool test will fail for a real reason
  that is nevertheless not the SDK's. Small instruct models often emit a tool call as prose.
- A base URL with a trailing slash is fine; it is trimmed before `/chat/completions` is appended.

The same variables point it at Groq, Together, OpenRouter or OpenAI itself.

## Why this doubles as an architecture test

`LlmAgent` reaches the model through plain `reqwest` and implements nothing but `trait Agent`. No
`ag-ui-*` crate depends on any LLM library. If that agent compiles and streams, the claim in
`docs/DESIGN.md` that the `Agent` trait is the LLM boundary is demonstrated rather than asserted.
So keep `rig`, `async-openai` and friends out of `e2e/Cargo.toml` — the absence is the evidence.

The `llm_agent` example serves the same agent over HTTP, so the smoke test exercises exactly the
code a reader is pointed at.

---

## Appendix: Gemini free-tier specifics

Measured, not documented. Re-deriving any of this costs a day, which is why it stays here even
though the harness is no longer Gemini-specific.

### Rate limits

Google no longer publishes per-model free-tier numbers. A burst of 30 concurrent requests returned
13 successes and 17 `429`s, and the error body named the quota:

```
quotaId:    GenerateRequestsPerMinutePerProjectPerModel-FreeTier
quotaValue: 10
```

So **10 requests per minute, per project, per model**. There are no `X-RateLimit-*` response
headers; the numbers only ever appear in a `429` body.

There is a **daily** quota too, and it is the one that actually stops work:

```
quotaId:    GenerateRequestsPerDayPerProjectPerModel-FreeTier
quotaValue: 20
```

So **20 requests per day**, not the ~1,000 third-party reports suggest. Two consequences:

1. The live suite has to be frugal. It spends three requests per full run.
2. A per-day `429` **cannot be waited out** — and its body still carries a
   `RetryInfo.retryDelay` of about a minute, which is a lie for this quota. Waiting that long and
   retrying just burns another request.

**`quotaId` is the only reliable discriminator.** Not the status, not the message, not
`RetryInfo` — the harness matches on `PerDay` versus `PerMinute` in the violation's `quotaId`, and
there is a test pinning both against verbatim captured bodies.

Quota is isolated per model, which is what makes falling back to a sibling work at all. Verified:
with `gemini-2.5-flash-lite` fully exhausted, `gemini-2.5-flash` and `gemini-3.1-flash-lite` still
returned 200.

### Never use a `*-latest` alias

`gemini-flash-lite-latest` currently resolves to a 3.x model. Aliases move, and the response shape
and tool-loop requirements move with them — without a code change on your side. Pin the id.

### Thought signatures, and why the compatibility endpoint does not fully hide them

A Gemini 3.x model signs its tool calls and rejects the follow-up request unless the signature comes
back with the call it arrived on:

```
HTTP 400: Function call is missing a thought_signature in functionCall parts.
```

2.5 sends none and needs none. That is the trap: **a client written and tested against 2.5 looks
finished and breaks the first time anything routes it to 3.x** — including the sibling-model
fallback above.

Moving to the OpenAI-compatible endpoint was expected to make this someone else's problem. **It does
not.** The live tool test failed with exactly that `400` on the compat endpoint the first time it
ran against `gemini-3.1-flash-lite`. What changed is only *where* the signature lives — it is a
vendor extension on the tool call rather than a field on a content part:

```json
"tool_calls":[{"extra_content":{"google":{"thought_signature":"EnEKbwER…"}},
               "function":{"arguments":"{\"city\":\"Seoul\"}","name":"get_weather"},
               "id":"call_272732","type":"function"}]
```

The rules, from [the signature guide](https://ai.google.dev/gemini-api/docs/generate-content/thought-signatures)
and confirmed against the wire:

- Return the signature **on the call it arrived on**.
- For **parallel** calls only the **first** is signed, and only that one is validated.
- Absent must stay **absent**. An empty extension is not a signature the model wrote.

The harness handles this by round-tripping `extra_content` as an **opaque blob**: it is stored,
echoed back on the matching call, and never inspected. That keeps the mapping vendor-neutral — the
code does not know what a thought signature is — and it will carry the next vendor's extension for
free. It is still much cheaper than the native handling, which had to place the signature on the
correct *part*, preserve part order, and send all calls before all responses.

Signatures map onto no AG-UI event: the protocol carries the call, not the provider's reasoning
about it. There is a test asserting none of it leaks into the event stream.

**Known limitation, and the protocol has the answer.** The round trip currently spans one run only.
A signature is held in memory for the duration of the agent loop, so a human-in-the-loop pause —
where the client executes the tool and sends the result back on a *new* request — loses it, and
against a 3.x model that second request would `400`.

The protocol already models this: `ToolCall.encrypted_value` is spec'd as an "opaque provider
payload for zero-data-retention reasoning modes", and `ToolCallHandle::encrypted_value` emits it as
`REASONING_ENCRYPTED_VALUE` with subtype `toolCall`. A thought signature is precisely that. Closing
the gap means emitting the extension there, and reading it back off the assistant message's
`ToolCall` in the next run.

It is deliberately **not** done yet: it adds an event to the tool-call stream, which the ordering
verifier and the live assertions would both need to account for, and a test harness does not
exercise the path that would prove it works. A production agent talking to 3.x through a
human-in-the-loop flow needs it.
