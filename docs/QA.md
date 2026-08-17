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

Two gotchas worth encoding as tests:

- Parallel tool calls arrive as multiple `functionCall` parts inside the *same* `parts` array of a
  single frame. Each needs its own synthesized `toolCallId`.
- A final frame may carry an empty text part `{"text": ""}` alongside `finishReason: STOP`.
  Do not emit an empty `TEXT_MESSAGE_CONTENT` for it.

Function-call schemas in `v1beta` use **uppercase** JSON Schema types (`OBJECT`, `STRING`), which
differs from the lowercase form AG-UI tool definitions carry. The example agent has to translate.

### Rate limit

Measured, not documented — Google no longer publishes per-model free-tier numbers. A burst of 30
concurrent requests returned 13 successes and 17 `429`s, and the error body named the quota:

```
quotaId:    GenerateRequestsPerMinutePerProjectPerModel-FreeTier
quotaValue: 10
```

So: **10 requests per minute, per project, per model.** There are no `X-RateLimit-*` response
headers, but the 429 body carries `details[].RetryInfo.retryDelay` (e.g. `"39s"`).

Requests-per-day was not measured — probing it would burn the day's quota. Third-party reports put
flash-lite near 1,000/day; treat that as unverified.

Three harness requirements follow, and the live tests are flaky without all three:

1. Run the LLM-backed suite **serialized** (`--test-threads=1`). Parallel Rust tests trip 10 RPM easily.
2. Retry on 429, honouring `RetryInfo.retryDelay`.
3. Fall back to a sibling model on repeated 429. Quota is isolated per model — verified: with
   flash-lite fully exhausted, `gemini-2.5-flash` and `gemini-3.1-flash-lite` still returned 200.

### Why this doubles as an architecture test

The example agent talks to Gemini through plain `reqwest` and implements nothing but `trait Agent`.
No `ag-ui-*` crate depends on any LLM library. If that example compiles and streams, the claim in
`docs/DESIGN.md` that the `Agent` trait is the LLM boundary is demonstrated rather than asserted.
