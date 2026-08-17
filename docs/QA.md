# QA strategy

Two tiers, because they answer different questions.

| Tier | What it proves | Runs |
| --- | --- | --- |
| Deterministic E2E | The protocol plumbing is correct: full event ordering, state deltas, and the human-in-the-loop round trip, driven over real HTTP by `ag-ui-client` against a real axum server. | Always. CI gate. |
| Live smoke | A real streaming LLM maps onto AG-UI events correctly, and the SDK genuinely depends on no LLM crate. | `#[ignore]`, only when `GEMINI_API_KEY` is set. Not a CI gate. |

The deterministic tier uses a scripted mock agent, so it is fast and cannot flake. The live tier
is excluded from the CI gate on purpose — see the rate limit below.

## Live tier: Gemini

`gemini-2.5-flash-lite` on the free tier. Chosen because it needs no credential we do not already
have, and because it is the fastest usable option: `gemini-2.5-flash` defaults to thinking and
measured ~7.0s time-to-first-token versus ~1.0s for flash-lite.

Do not use `*-latest` aliases. `gemini-flash-lite-latest` currently resolves to
`gemini-3.5-flash-lite`, so the response shape can change under you without a code change.

### Endpoint

```
POST https://generativelanguage.googleapis.com/v1beta/models/gemini-2.5-flash-lite:streamGenerateContent?alt=sse
x-goog-api-key: $GEMINI_API_KEY
```

Send the key as a header, never as a query parameter — query strings end up in logs.

### Mapping Gemini SSE onto AG-UI events

| Gemini | AG-UI | Note |
| --- | --- | --- |
| `candidates[0].content.parts[i].text` | `TEXT_MESSAGE_CONTENT.delta` | Arrives incrementally across frames. |
| `responseId` | `messageId` | Stable for the whole stream, so it identifies the message directly. |
| `candidates[0].content.parts[i].functionCall` | `TOOL_CALL_START` + `ARGS` + `END` | Arrives **atomically in one frame**, fully formed — unlike OpenAI there is no partial-JSON accumulation. Emit all three back to back. |
| `functionCall.args` | `TOOL_CALL_ARGS.delta` | Gemini gives a JSON **object**; AG-UI wants a string. `serde_json::to_string(&args)`. |
| — | `toolCallId` | **Must be synthesized.** 2.5-flash-lite supplies no id. (3.x does, as `"id": "call_…"`.) |
| `finishReason == "STOP"` | end of run | There is **no `[DONE]` sentinel**; the stream simply ends at HTTP body EOF. |
| `parts[i].thoughtSignature` | — (never leaves the agent) | 3.x only, and it must be **echoed back**. See below. |

Two gotchas worth encoding as tests:

- Parallel tool calls arrive as multiple `functionCall` parts inside the *same* `parts` array of a
  single frame. Each needs its own synthesized `toolCallId`.
- A final frame may carry an empty text part `{"text": ""}` alongside `finishReason: STOP`.
  Do not emit an empty `TEXT_MESSAGE_CONTENT` for it.

And one about the framing itself: frames are terminated with **`\r\n\r\n`**, not `\n\n`. A decoder
that scans only for `\n\n` never finds a boundary, buffers the whole response, and emits everything
at EOF — which reads as "streaming does not work" rather than as a parse error.

Function-call schemas in `v1beta` use **uppercase** JSON Schema types (`OBJECT`, `STRING`), which
differs from the lowercase form AG-UI tool definitions carry. The example agent has to translate.

### Thought signatures: 3.x requires them, 2.5 does not

This is the mapping's sharpest edge, because **a client that only ever ran against 2.5 looks
finished and breaks the moment anything routes it to 3.x** — including the sibling-model fallback
below. The failure is a flat rejection of the follow-up request:

```
HTTP 400: Function call is missing a thought_signature in functionCall parts.
```

A 3.x model attaches an opaque `thoughtSignature` to the *part* — beside `functionCall`, not inside
it — and will not accept its own call back without it:

```json
{"functionCall": {"name": "get_weather", "args": {"city": "Seoul"}, "id": "call_1"},
 "thoughtSignature": "CvsBAdHtim8="}
```

The rules, from [the API's signature guide](https://ai.google.dev/gemini-api/docs/generate-content/thought-signatures):

- Return each signature **in the part it arrived in**, keeping the parts in their original order.
- For **parallel** calls the signature is attached to the **first** `functionCall` part only, and
  only that one is validated.
- The final part of a text answer may also carry one; echoing it is recommended, not enforced.
- Absent must stay **absent**. 2.5 sends none and needs none; an empty string is a signature the
  model never wrote.
- The model turn goes back as all the calls, then all the `functionResponse` parts as the next
  turn. Interleaving them is a separate 400.

Signatures are internal to the model loop and map onto no AG-UI event — the protocol carries the
call, not the provider's reasoning about it.

### Rate limit

Measured, not documented — Google no longer publishes per-model free-tier numbers. A burst of 30
concurrent requests returned 13 successes and 17 `429`s, and the error body named the quota:

```
quotaId:    GenerateRequestsPerMinutePerProjectPerModel-FreeTier
quotaValue: 10
```

So: **10 requests per minute, per project, per model.** There are no `X-RateLimit-*` response
headers, but the 429 body carries `details[].RetryInfo.retryDelay` (e.g. `"39s"`).

There is a **daily** quota too, and it is the one that actually stops work. Running the live suite a
handful of times exhausted it, and the `429` body named it:

```
quotaId:    GenerateRequestsPerDayPerProjectPerModel-FreeTier
quotaValue: 20
```

So: **20 requests per day**, not the ~1,000 third-party reports suggest. Two consequences. The live
suite has to be frugal — it spends three requests per run — and a per-day `429` **cannot be waited
out**, even though its body still carries a `RetryInfo.retryDelay` of about a minute. Only switching
models gets past it.

Four harness requirements follow, and the live tests are flaky or misleading without all four:

1. Run the LLM-backed suite **serialized** (`--test-threads=1`). Parallel Rust tests trip 10 RPM easily.
2. Retry on 429, honouring `RetryInfo.retryDelay` — but only when the quota named is per-minute.
3. Fall back to a sibling model on a per-day 429. Quota is isolated per model — verified: with
   flash-lite fully exhausted, `gemini-2.5-flash` and `gemini-3.1-flash-lite` still returned 200.
   The fallback is why the client must satisfy 3.x's thought-signature rule above.
4. **Skip, do not fail, when no model could be reached.** `503 UNAVAILABLE` ("this model is
   currently experiencing high demand") is transient and routine, and a test that reports it as a
   failure sends somebody looking for an SDK bug that is not there. Failure is for a model that
   answered and was mapped wrongly; everything else is a skip that names the reason.

### Why this doubles as an architecture test

The example agent talks to Gemini through plain `reqwest` and implements nothing but `trait Agent`.
No `ag-ui-*` crate depends on any LLM library. If that example compiles and streams, the claim in
`docs/DESIGN.md` that the `Agent` trait is the LLM boundary is demonstrated rather than asserted.
