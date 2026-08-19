---
title: Testing
description: The two commands that test this workspace, why the second is not optional, the two QA tiers, and the full CI job list.
---

## Two commands, and the second one is not optional

```sh
cargo nextest run --workspace --all-features
cargo test --doc --workspace --all-features
```

**`cargo nextest` does not run doctests.** Not "skips them", not "reports them
as ignored" — it never sees them. A green nextest run is therefore a partial
result, and it does not tell you that it is one.

That matters here more than in most workspaces, because a large part of what
this one proves lives in doctests: every crate README, the workspace quickstart,
every Rust snippet on this documentation site, and — in
`crates/ag-ui/src/server/emit/mod.rs` — the `compile_fail` example that is the
only executable proof of the typestate guarantee
[Design commitments](/ag-ui-rust/design/commitments/) sells as a headline
feature. Weaken the emitter API and nextest stays green.

The gap is easy to see, and easy to mistake for coverage. When this page was
written, `cargo nextest list --workspace --all-features` reported 672 test cases
across 50 binaries — and not one of them was a doctest. Everything the second
command reports is a test the first has nothing at all to say about.

If you would rather have one command and can live without nextest's output,
`cargo test --workspace --all-features` runs both kinds. CI runs both forms, and
runs the doctest command a second time on purpose — so that if someone ever swaps
`cargo test` for `cargo nextest run` to buy speed, the doctests keep running
instead of vanishing without a failing build.

### This site is part of that

Every Rust block on these pages is compiled by
`cargo test --doc -p ag-ui-e2e --all-features`. The pages are included as module
documentation in `e2e/src/website.rs`, which makes rustdoc extract their fenced
Rust blocks and compile them exactly as it does the ones in a `lib.rs`.
Frontmatter, prose and `:::note` directives pass through untouched.

So a snippet that has gone stale is a red build, on the machine of whoever broke
it, rather than something a newcomer discovers by pasting it. Blocks that must
not actually run — they bind a port, or reach the network — are marked `no_run`
and are still type-checked. The list of pages is written out rather than globbed,
because the list is what a reader can trust: a page whose snippets are not
compiled has to be left off it deliberately.

## Testing an agent you have written

The emit path is synchronous, which makes an agent testable without a runtime,
without a port and without a client. `RunContext::new` hands you a context and
the receiving end of its event stream; after calling your agent's code,
everything it emitted is already queued, and `drain` takes it:

```rust
use ag_ui::{Event, RunAgentInput, TextMessageRole};
use ag_ui::server::{Result, RunContext};

fn greet(ctx: &mut RunContext<()>) -> Result<()> {
    let mut message = ctx.assistant_message()?;
    message.delta("Hello")?;
    message.end()
}

fn main() {
    let (mut ctx, mut events) =
        RunContext::<()>::new(RunAgentInput::new("thread-1", "run-1")).unwrap();

    greet(&mut ctx).unwrap();

    assert_eq!(
        events.drain(),
        vec![
            Event::text_message_start("run-1-msg-1", TextMessageRole::Assistant),
            Event::text_message_content("run-1-msg-1", "Hello"),
            Event::text_message_end("run-1-msg-1"),
        ],
    );
}
```

Nothing emits `RUN_STARTED` here — that is the run driver's job, and skipping it
is what lets a test exercise one method in isolation. The message id was derived
from the run id and a counter rather than a UUID, which is what makes an expected
event list like the one above writable at all.

## Two QA tiers

`docs/QA.md` splits the suite in two, because the halves answer different
questions.

| Tier | What it proves | Runs |
| --- | --- | --- |
| **Deterministic E2E** | The protocol plumbing is correct: full event ordering, state deltas and the human-in-the-loop round trip, driven over real HTTP by `ag_ui::client` against a real axum server. Plus the LLM mapping itself, driven from recorded SSE frames. | Always. The CI gate. |
| **Live smoke** | A real streaming model is reachable, maps onto AG-UI events correctly, and the SDK genuinely depends on no LLM crate. | `#[ignore]`, only when a key or a local endpoint is configured. Never a CI gate. |

The deterministic tier uses a scripted mock agent and recorded model frames, so
it is fast and cannot flake. **It is also the tier that protects the mapping** —
every parsing and accumulation rule is covered by unit tests in `e2e/src/llm.rs`
driven from captured or synthetic frames. The live test only proves the wire is
reachable.

### Why the live tier is excluded from the gate

It talks to someone else's capacity-constrained service, so most of the ways it
can not-work say nothing at all about this SDK. A `503 high demand` reported as
a test failure costs somebody an hour looking for a bug that is not there.

So the harness sorts the outcomes rather than treating them alike. A stream is
asserted on, loudly. A `429` naming a *per-minute* quota waits and asks again; a
`429` naming a *per-day* quota cannot be waited out, so it moves to the next
model. `500`, `502`, `503` and `504` are transient by definition — back off,
retry, then try the next model. A `404` means that model does not exist here.
Nothing answering on the socket is a **skip**: the endpoint is not up. Running
out of models is a **skip** that names which model failed how. Anything else
fails, because a `400` or an agent error is ours.

**Failure is reserved for a model that answered and was mapped wrongly.**

The numbers behind that policy were measured rather than read: the Gemini free
tier allows about 10 requests per minute and only about **20 per day**, per
project per model, and the daily quota still reports a `RetryInfo.retryDelay` of
about a minute, which is a lie for that quota. Runs are serialized —
`--test-threads=1` *and* an in-process mutex — because parallel tests trip the
per-minute limit immediately.

### Running the live tier

```sh
cargo test -p ag-ui-e2e --test live_llm -- --ignored --test-threads=1 --nocapture
```

`--nocapture` is worth typing: a run that skips, and the model's actual reply,
are printed rather than asserted, and the harness swallows the output of a test
that passed.

Three environment variables configure it:

| Variable | Default | Meaning |
| --- | --- | --- |
| `AG_UI_LLM_BASE_URL` | Gemini's OpenAI-compatible endpoint | `/chat/completions` is appended to this |
| `AG_UI_LLM_MODEL` | `gemini-2.5-flash-lite` | Model id — pinned, never a `*-latest` alias |
| `AG_UI_LLM_API_KEY` | falls back to `GEMINI_API_KEY` | Bearer token |

The harness speaks the OpenAI-compatible `POST {base}/chat/completions` shape
rather than any vendor's own dialect, because that is the one shape nearly
everything serves. The same three variables point it at Ollama, LM Studio,
llama.cpp, vLLM, Groq, Together, OpenRouter or OpenAI itself.

:::caution[Rules about the key, all three of them load-bearing]
- The key goes in the **`Authorization: Bearer` header**, never a query
  parameter — query strings end up in logs. It is never printed: not partially,
  not in an error, not in a `Debug` dump. `LlmAgent`'s `Debug` impl is
  hand-written to redact it, and there is a test for that.
- With no key at all the live tests **skip**, naming the variable they looked
  for. They never fail for it, so a contributor without a key still sees a green
  run.
- **Absent must stay absent.** An empty `Authorization: Bearer` is a *rejected*
  request, not an anonymous one, so a missing key is only an error when the
  endpoint is the default one. Do not set the variable to an empty string to
  "turn it off"; leave it unset.
:::

### Point it at a local model instead

Nothing is rate limited, no key is involved, and the run costs nothing — which
makes this the better way to work on the mapping:

```sh
ollama serve && ollama pull qwen3:4b
export AG_UI_LLM_BASE_URL=http://localhost:11434/v1
export AG_UI_LLM_MODEL=qwen3:4b
cargo test -p ag-ui-e2e --test live_llm -- --ignored --test-threads=1 --nocapture
```

Do not set a key. Model fallback is switched off when the base URL is not the
default, because a local server has the one model you loaded and there is no
per-model quota to route around. Pick a model that actually supports tool
calling, or the tool test will fail for a real reason that is nevertheless not
the SDK's — small instruct models often emit a tool call as prose.

### It doubles as an architecture test

`LlmAgent` reaches the model through plain `reqwest` and implements nothing but
`Agent`. No `ag-ui-*` crate depends on any LLM library. If that agent compiles
and streams, the claim that
[the `Agent` trait is the LLM boundary](/ag-ui-rust/design/commitments/) is
demonstrated rather than asserted — which is why `rig`, `async-openai` and
friends stay out of `e2e/Cargo.toml`. The absence is the evidence.

## Before you commit

Hygiene is gated by [prek](https://github.com/j178/prek) — pre-commit's hook
runner rebuilt as a single Rust binary, so there is no Python to install. Two
commands, once:

```sh
brew install prek   # or: cargo install --locked prek
prek install
```

`prek install` wires up both shims, and the second one matters: whitespace, file
syntax (YAML, TOML, JSON), line endings, merge-conflict markers, oversized
files, spelling and `cargo fmt --all -- --check` run on every **commit**, while
`cargo clippy --workspace --all-targets --all-features -- -D warnings` runs on
every **push**, where the wait buys something. Without the pre-push shim the
clippy hook would never fire locally at all.

`prek run --all-files` runs the lot by hand, and CI's `hygiene` job runs exactly
the same `.pre-commit-config.yaml` — so anyone who has run `prek install` cannot
produce a commit that job then rejects.

The config is deliberately small. CI already owns fmt, clippy, tests, doctests,
the feature matrix, MSRV, docs, the wasm and dependency-graph checks and the
drift check; repeating any of those on every commit would make every commit
slower without catching anything new.

It also has an exclude list, and that list is not incidental: the A2UI
conformance suite, the spec fixtures, the insta snapshots and the drift baseline
are all either copied from another project or written by a tool, and their worth
depends on staying byte-identical to that source. The first run of this config
quietly added a trailing newline to seventeen vendored fixtures, which is how
the list came to exist.

## What CI runs

Ten jobs. Nine run on every push and pull request; the tenth runs on a weekly
timer, and any of them can be triggered by hand.

| Job | What it does |
| --- | --- |
| `hygiene (prek)` | The `.pre-commit-config.yaml` above, `--all-files`. This is where `cargo fmt --all -- --check` lives — one formatting gate, in the one place a contributor also runs it. |
| `test` | `cargo clippy --workspace --all-targets --all-features -- -D warnings`, then `cargo test --workspace --all-features`, then `cargo test --doc --workspace --all-features` again on purpose. |
| `doctest error codes (nightly)` | The doctests again on nightly, which is the only thing that enforces the error code on a `compile_fail,E0499` annotation. The only use of nightly in the build. |
| `executor-agnostic` | Builds core, server, client and a2ui for `wasm32-unknown-unknown` (five `cargo check` invocations), then asserts tokio is absent from four dependency graphs. |
| `feature matrix` | Fifteen `cargo check --all-targets` runs: every feature alone, and every crate with its defaults off. |
| `MSRV 1.85` | `cargo check --workspace --all-features --all-targets` on 1.85 — the first compiler that understands edition 2024, so there is no slack in the promise. |
| `docs` | `cargo doc --workspace --all-features --no-deps` with `RUSTDOCFLAGS: -D warnings`. The public API is the product, so a broken intra-doc link is a defect in the deliverable. |
| `package manifest` | `cargo package --list` for the two crates that carry no `publish = false` — `ag-ui` and `ag-ui-a2ui`, as against `xtask`, the e2e suite and the examples — asserting each would package its `README.md` and `LICENSE`. Offline: it builds no archive and uploads nothing. |
| `protocol drift vs upstream` | `cargo run -p xtask -- drift-check`. Offline and deterministic, which is what qualifies it as a required check. |
| `upstream freshness (scheduled)` | `drift-check --upstream`, weekly. It needs the network, so it is a timer rather than a gate: a rate limit cannot fail it, only real upstream movement can. |

The last two are [Verification](/ag-ui-rust/design/verification/).

Two of these jobs are worth knowing the reasoning for before editing them. The
`hygiene` job's Rust toolchain is load-bearing — its `cargo-fmt` hook shells out
to `cargo fmt`, and without rustfmt installed it fails rather than skips — and
deleting the job removes formatting from CI altogether. And clippy is
deliberately *not* in `hygiene`: the config puts it on the pre-push stage, which
`prek run` does not reach by default, so it stays in `test` where it shares that
job's compilation cache instead of paying for its own.
