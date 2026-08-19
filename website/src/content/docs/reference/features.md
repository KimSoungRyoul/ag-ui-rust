---
title: Feature flags
description: Every Cargo feature across the two crates — default state, what it pulls in, what turning it off costs, and how CI checks the combinations.
---

Eleven features across two crates. This page is the complete list; each crate's rustdoc front
page carries the same information —
[`ag_ui`](/ag-ui-rust/api/ag_ui/index.html),
[`ag_ui_a2ui`](/ag-ui-rust/api/ag_ui_a2ui/index.html).

Features are how this SDK keeps a dependency out of a build, which is the job a crate split
would otherwise do. `ag_ui::serve`, `ag_ui::client` and `ag_ui::axum` are modules gated on the
features of the same name.

:::note[How to depend on this]
Two crates, not five: `ag-ui` carries the protocol types plus the `serve`, `client` and
`axum` runtimes behind features, and `ag-ui-a2ui` stays separate because A2UI is a different
protocol that can be used with no AG-UI at all. The `ag-ui-core`, `ag-ui-server` and
`ag-ui-client` names on crates.io belong to the earlier community SDK and are not this
project.
:::

## The whole matrix

| Crate | Feature | Default | Pulls in | Adds |
| --- | --- | :---: | --- | --- |
| `ag-ui` | `sse` | on | — | `SseFormatter` and `text/event-stream` framing. |
| `ag-ui` | `protobuf` | off | — | The binary transport's media type and content negotiation. There is no encoder. |
| `ag-ui` | `schemars` | off | `schemars` | `schemars::JsonSchema` derives on the public types. |
| `ag-ui` | `utoipa` | off | `utoipa` | `utoipa::ToSchema` derives on the public types. |
| `ag-ui` | `serve` | off | `futures-*`, `json-patch` | The `serve` module: hosting an agent. |
| `ag-ui` | `verify` | on | — | `serve`'s runtime ordering state machine. |
| `ag-ui` | `client` | off | `futures-*`, `json-patch` | The `client` module: consuming an agent, transport-agnostic. |
| `ag-ui` | `http` | off | `reqwest` | `HttpTransport` and `HttpAgent`. Implies `client` and `sse`. |
| `ag-ui` | `axum` | off | `axum`, `tokio` | The `axum` module. Implies `serve` and `sse`. |
| `ag-ui-a2ui` | `toolkit` | on | — | Agent-side authoring: op builders, catalog negotiation, prompt assembly, stream parsing, the recovery loop. |
| `ag-ui-a2ui` | `ag-ui` | on | `ag-ui` | Interop with AG-UI types. Implies `toolkit`. |

`verify` sits in `ag-ui`'s default set rather than being implied by `serve`, and that is what
makes it possible to drop: a feature cannot be subtracted from the set another feature pulls
in, so `serve = [..., "verify"]` would weld it on. `default-features = false` plus
`features = ["serve", "sse"]` is the build that compiles the verifier away.

Six of the eleven add a dependency. The rest are code gates over what the crate already
compiles.

## Feature by feature

**`ag-ui/sse`.** You lose `SseFormatter` and the SSE branch of content negotiation. With
`protobuf` also off you lose the whole `encode` module — it is gated on
`any(feature = "sse", feature = "protobuf")` — leaving the protocol types and nothing that
frames them for a wire. `ag_ui::axum` uses `SseFormatter` directly, so it needs this feature;
it is on by default and nothing in this workspace turns it off.

**`ag-ui/protobuf`.** All this feature adds is the binary media type's presence in content
negotiation. It pulls in no dependency, and it adds no encoder. `encode::media_type` scores an
`Accept` header against the media types the current build can emit, and SSE is first in the
preference order either way:

```rust
use ag_ui::encode::{SSE_MEDIA_TYPE, media_type};

// A missing header is `*/*` per RFC 9110, and ties go to SSE.
assert_eq!(media_type(None).unwrap(), SSE_MEDIA_TYPE);
assert_eq!(media_type(Some("text/event-stream")).unwrap(), SSE_MEDIA_TYPE);
// A header that excludes everything this build emits is the 406 case.
assert!(media_type(Some("application/xml")).is_err());
```

That differs from the TypeScript encoder, which upgrades a bare `*/*` to protobuf. The reason
is that here protobuf cannot carry a run. Upstream's `events.proto` has an `Event` oneof
covering 18 of the protocol's 33 event types.
Every `REASONING_*` event, both `ACTIVITY_*` events, the five deprecated `THINKING_*` events,
and `TOOL_CALL_RESULT` have no binary representation at all. Encoding a run that uses them
would mean silently dropping events, so `ProtobufFormatter::encode` always returns
`Error::UnsupportedTransport` and says why. The feature exists so a build can still name and
negotiate the media type, and so the reason sits next to the code.

**`ag-ui/schemars`, `ag-ui/utoipa`.** Off by default: each is a dependency added for
something most consumers do not need. Turn one on when you are generating a JSON Schema or an
OpenAPI document that has to describe the protocol types.

**`ag_ui::serve/verify`.** You lose server-side protocol verification. The verifier becomes a
zero-sized type whose checks compile away, which is the point: it is on by default, in release
builds too, and the feature is there to get the last handful of `HashSet` lookups back if you
have measured that you want them. Turning it off does not change what your agent emits — it
changes whether emitting `TEXT_MESSAGE_CONTENT` without a preceding `START` is reported where
it was caused or three network hops downstream.

**`ag_ui::client/http`.** You lose `HttpTransport` and `HttpAgent`, and the `reqwest` dependency
goes with them. Everything else in the crate — application, normalisation, verification — is a
plain synchronous state machine and keeps working. `Transport` is a trait, so a wasm frontend
or a non-tokio runtime substitutes its own. This is the one feature whose off-state is a
[platform commitment](/ag-ui-rust/reference/platforms/) rather than a size trade: `ag_ui::client`
is executor-agnostic only with `http` off, because `reqwest` pulls tokio.

**`ag-ui-a2ui/toolkit`.** You lose everything under `toolkit::` — the operation builders, the
transport envelope, the prompt assembly, the parsers, the recovery loop. What is left is the
protocol types, the catalog, the validator, and the binding layer: enough to *check* A2UI, not
to author it. It pulls in no dependency, so the only reason to turn it off is that you are not
generating surfaces.

**`ag-ui-a2ui/ag-ui`.** You lose the `agui` module and the `ag-ui` dependency: the `From`
impls between AG-UI messages and A2UI history entries, the conversion of toolkit tool
definitions into offerable `Tool`s, and `find_prior_surface_in`. Nothing else in the crate
knows AG-UI exists, so what remains is a standalone A2UI implementation you can drive over A2A
or MCP. It implies `toolkit`, because everything it converts lives there.

```toml
# A2UI without AG-UI.
[dependencies.ag-ui-a2ui]
version = "0.1"
default-features = false
features = ["toolkit"]
```

## How CI checks them

The `features` job in `.github/workflows/ci.yml` runs fifteen `cargo check --all-targets`
invocations: **every feature alone, and every crate with its defaults off.**

```sh
cargo check --all-targets -p ag-ui      --no-default-features
cargo check --all-targets -p ag-ui      --no-default-features --features sse
cargo check --all-targets -p ag-ui      --no-default-features --features protobuf
cargo check --all-targets -p ag-ui      --no-default-features --features schemars
cargo check --all-targets -p ag-ui      --no-default-features --features utoipa
cargo check --all-targets -p ag-ui      --no-default-features --features serve
cargo check --all-targets -p ag-ui      --no-default-features --features serve,verify,sse
cargo check --all-targets -p ag-ui      --no-default-features --features client
cargo check --all-targets -p ag-ui      --no-default-features --features http
cargo check --all-targets -p ag-ui      --no-default-features --features axum
cargo check --all-targets -p ag-ui      --all-features
cargo check --all-targets -p ag-ui-a2ui --no-default-features
cargo check --all-targets -p ag-ui-a2ui --no-default-features --features toolkit
cargo check --all-targets -p ag-ui-a2ui --no-default-features --features ag-ui
cargo check --all-targets -p ag-ui-a2ui --all-features
```

Since the runtimes became features, `--all-targets` carries more weight than it did: the
integration tests are targets of `ag-ui` itself and are gated with `#![cfg(feature = "…")]`,
so a test that needs `serve` compiles to an empty crate without it. Without `--all-targets`
nothing would notice a gate that names the wrong feature.

Not a powerset. The job's own comment gives the reason: a powerset would be 2⁴ for `ag-ui`
alone and would not buy much, because these features are additive and independent — none of
them changes what another one compiles to. What a powerset would catch is a `cfg` that is right
for one combination and wrong for another, and the shape of these features makes that
unlikely enough not to pay sixteen builds for.

Two details in those lines are load-bearing.

`--all-targets` compiles tests, benches, and examples as well as the library. Without it, a
feature-gated test that no longer compiles under some combination would go unnoticed, because
`cargo check` alone never looks at it.

`-p ag-ui-a2ui --no-default-features --features ag-ui` is the line that exercises the
implication. `ag-ui = ["dep:ag-ui", "toolkit"]`, so asking for `ag-ui` alone has to bring
`toolkit` with it; if that implication were ever dropped, the `agui` module would fail to
compile against a `toolkit` that is not there, and this is the check that says so.

Two other jobs constrain features from a different direction. `msrv` builds
`--workspace --all-features --all-targets` on Rust 1.85, so a feature cannot quietly require a
newer compiler. `executor-agnostic` asserts tokio is absent from four dependency graphs, one of
which is `ag_ui::client --no-default-features` — see
[Platforms and MSRV](/ag-ui-rust/reference/platforms/).
