---
title: feature flag
description: 두 crate의 모든 Cargo feature — 기본 상태, 무엇을 끌어오는지, 끄면 무엇을 잃는지, CI가 그 조합을 어떻게 검사하는지.
---

crate 둘에 feature 열한 개가 있습니다. 이 page가 완전한 목록입니다. 각 crate의 rustdoc 첫
page에도 같은 정보가 있습니다 —
[`ag_ui`](/ag-ui-rust/api/ag_ui/index.html),
[`ag_ui_a2ui`](/ag-ui-rust/api/ag_ui_a2ui/index.html).

이 SDK가 dependency를 build 밖에 두는 수단이 feature입니다. crate를 쪼개서 하는 일을
feature가 합니다. `ag_ui::serve`, `ag_ui::client`, `ag_ui::axum`은 같은 이름의 feature로
gating되는 module입니다.

:::note[여기에 의존하는 방법]
다섯이 아니라 둘입니다. `ag-ui`가 protocol type과 `serve`·`client`·`axum` runtime을
feature 뒤에 담고, `ag-ui-a2ui`는 따로 남습니다. A2UI는 AG-UI 없이도 쓰는 별개
protocol이기 때문입니다. crates.io의 `ag-ui-core`, `ag-ui-server`, `ag-ui-client`
이름은 이전 community SDK의 것이고 이 project가 아닙니다.
:::

## 전체 matrix

| crate | feature | 기본값 | 끌어오는 것 | 더하는 것 |
| --- | --- | :---: | --- | --- |
| `ag-ui` | `sse` | on | — | `SseFormatter`와 `text/event-stream` framing. |
| `ag-ui` | `protobuf` | off | — | binary transport의 media type과 content negotiation. encoder는 없습니다. |
| `ag-ui` | `schemars` | off | `schemars` | 공개 type에 붙는 `schemars::JsonSchema` derive. |
| `ag-ui` | `utoipa` | off | `utoipa` | 공개 type에 붙는 `utoipa::ToSchema` derive. |
| `ag-ui` | `serve` | off | `futures-*`, `json-patch` | `serve` module. agent를 host합니다. |
| `ag-ui` | `verify` | on | — | `serve`의 runtime ordering state machine. |
| `ag-ui` | `client` | off | `futures-*`, `json-patch` | `client` module. agent를 소비합니다. transport를 가리지 않습니다. |
| `ag-ui` | `http` | off | `reqwest` | `HttpTransport`와 `HttpAgent`. `client`와 `sse`를 함의합니다. |
| `ag-ui` | `axum` | off | `axum`, `tokio` | `axum` module. `serve`와 `sse`를 함의합니다. |
| `ag-ui-a2ui` | `toolkit` | on | — | agent 쪽 authoring: operation builder, catalog negotiation, prompt assembly, stream parsing, recovery loop. |
| `ag-ui-a2ui` | `ag-ui` | on | `ag-ui` | AG-UI type과의 interop. `toolkit`을 함의합니다. |

`verify`가 `serve`에 함의되지 않고 `ag-ui`의 default 집합에 있습니다. 그래야 끌 수 있기
때문입니다. 다른 feature가 끌어온 집합에서 feature 하나를 빼낼 수는 없어서,
`serve = [..., "verify"]`였다면 붙박이가 됩니다. verifier를 compile에서 없애는 build는
`default-features = false`에 `features = ["serve", "sse"]`입니다.

열하나 중 dependency를 더하는 것은 여섯입니다. 나머지는 crate가 이미 compile하는 것에 대한
code gate입니다.

## feature 하나씩

**`ag-ui/sse`.** 끄면 `SseFormatter`와 content negotiation의 SSE 분기를 잃습니다.
`protobuf`까지 off면 `encode` module 전체를 잃습니다.
`any(feature = "sse", feature = "protobuf")`로 gating되기 때문입니다. protocol type만 남고,
그것을 wire용으로 framing하는 것은 남지 않습니다. `ag_ui::axum`은 `SseFormatter`를 직접 씁니다.
그래서 이 feature가 필요합니다. 기본이 on이고, 이 workspace의 어떤 것도 끄지 않습니다.

**`ag-ui/protobuf`.** 이 feature가 더하는 것은 하나뿐입니다. content negotiation에
binary media type이 존재하게 하는 것입니다. dependency를 끌어오지 않고 encoder도 더하지
않습니다. `encode::media_type`은 `Accept` header를 현재 build가 emit할 수 있는 media type과
견줍니다. 어느 쪽이든 선호 순서에서는 SSE가 먼저입니다:

```rust
use ag_ui::encode::{SSE_MEDIA_TYPE, media_type};

// header가 없으면 RFC 9110에 따라 `*/*`입니다. 동점이면 SSE가 이깁니다.
assert_eq!(media_type(None).unwrap(), SSE_MEDIA_TYPE);
assert_eq!(media_type(Some("text/event-stream")).unwrap(), SSE_MEDIA_TYPE);
// 이 build가 emit하는 것을 모두 배제하는 header가 406입니다.
assert!(media_type(Some("application/xml")).is_err());
```

TypeScript encoder는 다릅니다. 맨 `*/*`를 protobuf로 승격시킵니다. 이유는 여기서 protobuf가
run을 실어 나를 수 없다는 것입니다. upstream의 `events.proto`에 있는 `Event` oneof는
protocol의 event type 33개 중 18개를 다룹니다.
모든 `REASONING_*` event, `ACTIVITY_*` event 둘, 폐기된 `THINKING_*` event 다섯, 그리고
`TOOL_CALL_RESULT`에는 binary 표현이 없습니다. 그것들을 쓰는 run을 encoding하면 event를 조용히
버리게 됩니다. 그래서 `ProtobufFormatter::encode`는 언제나 `Error::UnsupportedTransport`를
반환하고 이유를 밝힙니다. 이 feature가 있는 이유는 둘입니다. build가 그래도 그 media type을
이름 붙이고 협상할 수 있게 하는 것, 그리고 그 이유가 code 옆에 있게 하는 것입니다.

**`ag-ui/schemars`, `ag-ui/utoipa`.** 기본이 off입니다. 둘 다 대부분의 소비자에게
필요 없는 일을 위한 dependency이기 때문입니다. protocol type을 기술해야 하는 JSON Schema나
OpenAPI 문서를 만들 때 켜십시오.

**`ag_ui::serve/verify`.** 끄면 server 쪽 protocol verification을 잃습니다. verifier는 크기
0인 type이 되고 검사는 compile 과정에서 사라집니다. 그게 핵심입니다. 이 feature는 release
build에서도 기본이 on입니다. `HashSet` 조회 몇 번까지 되찾고 싶다고 측정으로 확인했을 때
쓰라고 있는 것입니다. 꺼도 agent가 emit하는 것은 달라지지 않습니다. 달라지는 것은 보고
위치입니다. 앞선 `START` 없이 `TEXT_MESSAGE_CONTENT`를 emit한 일이 원인 자리에서 보고될지,
network를 세 번 건넌 하류에서 보고될지가 바뀝니다.

**`ag_ui::client/http`.** 끄면 `HttpTransport`와 `HttpAgent`를 잃습니다. `reqwest`
dependency도 함께 갑니다. crate의 나머지는 계속 동작합니다. event application, normalisation,
verification은 평범한 동기 state machine입니다. `Transport`는 trait입니다. wasm frontend나
tokio 아닌 runtime은 자기 것을 끼워 넣습니다. off 상태가 크기 절충이 아니라
[platform 약속](/ag-ui-rust/ko/reference/platforms/)인 feature는 이것뿐입니다. `reqwest`가
tokio를 끌어오기 때문에, `ag_ui::client`는 `http`가 off일 때만 executor에 종속되지 않습니다.

**`ag-ui-a2ui/toolkit`.** 끄면 `toolkit::` 아래를 전부 잃습니다. operation builder, transport
envelope, prompt assembly, parser, recovery loop입니다. 남는 것은 protocol type과 catalog,
validator, binding 층입니다. A2UI를 *검사*하기에는 충분하고, authoring하기에는 부족합니다.
dependency를 끌어오지 않습니다. 그래서 끌 이유는 surface를 만들지 않는다는 것뿐입니다.

**`ag-ui-a2ui/ag-ui`.** 끄면 `agui` module과 `ag-ui` dependency를 잃습니다. AG-UI
message와 A2UI history entry 사이의 `From` 구현, toolkit tool 정의를 offer 가능한 `Tool`로
바꾸는 변환, `find_prior_surface_in`이 여기 해당합니다. crate의 나머지는 AG-UI의 존재를
모릅니다. 그래서 남는 것은 A2A나 MCP 위에서 구동하는 독립 A2UI 구현입니다. 이 feature는
`toolkit`을 함의합니다. 변환 대상이 모두 거기 있기 때문입니다.

```toml
# AG-UI 없는 A2UI.
[dependencies.ag-ui-a2ui]
version = "0.1"
default-features = false
features = ["toolkit"]
```

## CI가 이것들을 검사하는 방법

`.github/workflows/ci.yml`의 `features` job은 `cargo check --all-targets`를 열다섯 번
돌립니다. **feature 하나씩 전부, 그리고 crate마다 기본값 off**입니다.

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

runtime이 feature가 된 뒤로 `--all-targets`의 무게가 커졌습니다. 통합 test가 `ag-ui`
자신의 target이고 `#![cfg(feature = "…")]`로 gating되기 때문입니다. 그 feature가 없으면
test는 빈 crate로 compile됩니다. `--all-targets`가 없으면 gate가 엉뚱한 feature를 가리켜도
아무도 알아채지 못합니다.

powerset은 아닙니다. job의 주석이 이유를 밝힙니다. powerset이면 `ag-ui` 하나만으로 2⁴입니다.
그런데 얻는 것은 적습니다. 이 feature들은 가산적이고 서로 독립입니다. 어느 것도 다른 것이
무엇으로 compile되는지를 바꾸지 않습니다. powerset이 잡을 만한 것은 어떤 조합에서 맞고 다른
조합에서 틀린 `cfg`입니다. 이 feature들의 생김새로는 그럴 일이 드뭅니다. build 열여섯 번을
치를 만큼은 아닙니다.

그 줄들에서 두 가지가 핵심입니다.

`--all-targets`는 library뿐 아니라 test와 bench, 예제까지 compile합니다. 없으면 어떤 조합에서
compile되지 않는 feature-gated test를 놓칩니다. `cargo check`만으로는 그것을 보지 않기
때문입니다.

`-p ag-ui-a2ui --no-default-features --features ag-ui`가 그 함의를 시험하는 줄입니다.
`ag-ui = ["dep:ag-ui", "toolkit"]`이므로 `ag-ui`만 요청해도 `toolkit`이 따라와야 합니다.
그 함의가 빠지면 `agui` module은 있지도 않은 `toolkit`에 대해 compile에 실패합니다. 그것을
알려 주는 검사가 이것입니다.

job 둘이 다른 방향에서 feature를 제약합니다. `msrv`는 Rust 1.85에서
`--workspace --all-features --all-targets`를 build합니다. 그래서 어떤 feature도 조용히 더
새로운 compiler를 요구할 수 없습니다. `executor-agnostic`은 dependency graph 넷에 tokio가
없음을 단언합니다. 그중 하나가 `ag_ui::client --no-default-features`입니다 —
[platform과 MSRV](/ag-ui-rust/ko/reference/platforms/)를 보십시오.
