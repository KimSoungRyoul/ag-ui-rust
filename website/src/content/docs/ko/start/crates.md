---
title: crate 구성
description: crate 둘이 각각 무엇을 위한 것인지. agent를 serving하려면, 소비하려면, 둘 다 하려면 어느 feature가 필요한지. 그리고 왜 다섯이 아니라 둘인지.
---

crate 둘, 그리고 feature 한 묶음입니다. 나뉘는 선은 protocol 하나에 crate 하나입니다.
`ag-ui`가 AG-UI이고 `ag-ui-a2ui`가 A2UI입니다. `ag-ui` 안에서는 protocol이 *무엇인가*가
모두에게 compile되고, agent가 되거나 agent와 말하는 데 필요한 것은 feature입니다.

| crate / feature | 무엇인가 |
| --- | --- |
| `ag-ui` | protocol type, 33개 event variant 전부, 그리고 wire encoding. `serde`와 `serde_json`뿐. |
| ↳ `serve` | agent를 띄웁니다. `Agent` trait, typestate event emitter, 자동 state delta, protocol verification. executor를 가리지 않습니다. |
| ↳ `client` | 원격 agent를 소비합니다. transport, event 적용, 실체화된 message와 state. |
| ↳ `http` | `client`를 위한 `reqwest` transport. |
| ↳ `axum` | agent를 axum router에 올립니다. tokio를 끌어오는 유일한 feature. |
| `ag-ui-a2ui` | A2UI protocol type, 의미 validator, 그리고 agent 쪽 authoring toolkit. |

## 어느 것이 필요한가

**HTTP로 agent를 serving하려면:** `features = ["axum"]`. `serve`와 `sse`를 함의합니다.
signature에 이름을 쓰게 될 protocol type은 어느 쪽이든 crate root에 있습니다.

**agent를 다른 곳에서 serving하려면** `features = ["serve"]`입니다. Lambda, WebSocket,
process 안의 test가 그렇습니다. `serve::run(agent, input)`은 event `Stream`을 건네고
거기서 멈춥니다. 직렬화는 transport의 일입니다. axum이 아닌 무언가 위에 SSE를 얹어야
한다면 crate root의 `SseFormatter`가 frame을 만들어 줍니다.

**agent를 소비하려면:** `features = ["http"]`. 대신 `client`를 요청하면 transport 없는
runtime을 받습니다. `Transport`를 직접 구현하세요. wasm을 위해, tokio가 아닌 runtime을
위해, HTTP가 아닌 socket을 위해서입니다.

**한 process에서 둘 다:** `features = ["axum", "http"]`. 드문 일이 아닙니다. 다른 agent를
부르는 agent는 client이면서 server입니다. client 쪽 trait 이름이 `Agent`가 아니라
`RemoteAgent`인 이유가 그것입니다. 둘 다 하는 파일에서는 두 이름이 부딪힙니다.

**A2UI:** `ag-ui-a2ui`에, 위의 것 중 해당하는 것을 더합니다. 이것만 성격이 다릅니다.
이유는 다음 절에 있습니다.

## 왜 다섯이 아니라 둘인가

한때 crate 다섯이었습니다. `ag-ui-core`, `-server`, `-client`, `-axum`, `-a2ui`입니다.
그 다섯을 만든 규칙이 그대로 그것들을 접었습니다. crate를 쪼개려면 **dependency 격리**나
**독립 versioning** 중 하나로 정당화되어야 합니다. Rust에서는 둘 다 feature가 주된
수단이기 때문입니다.

그 분리는 둘 다 주지 못했습니다. feature gate가 dependency를 똑같이 격리합니다.
`--no-default-features`는 axum도 tokio도 reqwest도 compile하지 않고, CI가 그것을 feature
단위로 단언합니다. 그리고 이 workspace는 lockstep으로 versioning합니다. `workspace.version`
하나로 전부 함께 release합니다. 독립 version을 얻고 있던 것도 없었다는 뜻입니다.

남은 것이 protocol 하나에 crate 하나입니다. `ag-ui-a2ui`는 feature가 할 수 없는 격리
논거로 따로 남습니다. A2UI는 별개 protocol이고 AG-UI 없이 A2A나 MCP 위에서 구동되며,
그 사용자가 `ag-ui`라는 이름의 crate에 의존할 이유가 없습니다.

비용은 실재하고 알아 둘 값어치가 있습니다. cargo는 dependency graph 전체에서 feature를
합칩니다. build 안의 어떤 crate가 `serve`를 요청하고 다른 crate가 `client`를 요청하면 둘
다 compile됩니다. 섞인 graph에서의 compile 시간이지 runtime이나 정확성 비용은 아닙니다.

## 전체 모양

```text
  ag-ui  (기본)                             ag-ui-a2ui
  serde · serde_json · thiserror           jsonptr
            │                              (ag-ui optional)
  ┌─────────┼──────────┐
  │         │          │
serve     client     axum
futures · futures ·  axum · tower · tokio
json-patch  json-patch     (serve와 sse를 함의)
            └ http
              reqwest
```

저 그림에서 무게를 지는 것이 셋입니다.

**tokio는 `axum`과 함께 들어오고 다른 곳에서는 들어오지 않습니다.** protocol type과
`serve`·`client` runtime은 `futures` primitive를 씁니다. emit 경로에
`tokio::sync::mpsc`가 아니라 `futures::channel::mpsc`를 쓰는 식입니다. 그래서 wasm
target과 tokio가 아닌 executor가 계속 돕니다. CI가 두 방향으로 강제합니다. feature마다
`wasm32-unknown-unknown`으로 build해 봅니다. tokio 자체도 wasm으로 compile되므로, 그
dependency graph에 tokio가 없다는 것까지 확인합니다. 이 단언은 crate였을 때보다 지금 더
중요합니다. cargo는 graph 전체에서 feature를 합치므로, `serve`에 `dep:tokio`를 한 번
잘못 달면 `axum`을 요청한 적 없는 모든 소비자에게 닿습니다. 자세한 것은
[platform과 MSRV](/ag-ui-rust/ko/reference/platforms/)에 있습니다.

**기본 build는 일부러 작게 둡니다.** `sse` 말고 아무 feature도 없으면 runtime도 I/O도
async도 없습니다. type, 그 type의 정확한 JSON 표현, 그리고 그것을 실어 나르는 SSE
framing뿐입니다. 그래서 양쪽 절반 아래에 놓이면서도 어느 쪽으로도 무언가를 끌고 들어가지
않습니다.

**`ag-ui-a2ui`는 나머지에 의존하지 않습니다.** A2UI는 별개의 protocol입니다. agent가
surface를 서술하는 JSON을 흘려보내면 renderer가 그것을 그립니다. 이 crate는 그 교환의
agent 쪽 절반입니다. `ag-ui` feature가 `ag-ui`와의 상호운용을 더합니다. 끄면 A2A나
MCP 위에서 굴릴 수 있는 crate가 됩니다:

```rust
use ag_ui_a2ui::{catalog::Catalog, message::Component, validate::Validator};
use serde_json::json;

let catalog = Catalog::basic();
let components = vec![
    Component::new("root", "Card").with("child", json!("greeting")),
    Component::new("greeting", "Text").with("text", json!("Hello!")),
];

let report = Validator::new(&catalog).validate(&components);
assert!(report.is_valid());
```

저기 어디에도 AG-UI 이야기가 없습니다. [A2UI](/ag-ui-rust/ko/a2ui/)가 그것을 다루는
절입니다.

## 앞서 접혀 들어간 것

같은 규칙이 이미 일곱을 다섯으로 접었고, 그 둘은 그대로 남습니다.

`ag-ui-encoder`는 `ag_ui::encode`가 되었습니다. SSE framing은 추가 dependency가
하나도 없는 수백 줄입니다. 격리할 것이 없었습니다. 무거운 부분은 protobuf뿐인데, 그것은
이미 optional dependency가 처리합니다.

`ag-ui-a2ui-toolkit`은 `ag-ui-a2ui`의 `toolkit` feature가 되었습니다. prompt 문자열과
orchestration입니다. 이것도 격리할 것이 없습니다.

값까지 포함한 전체 논거는 `docs/DESIGN.md`에 있습니다.
[설계 원칙](/ag-ui-rust/ko/design/commitments/)에 요약해 두었습니다.

## feature 한눈에 보기

| crate | feature | 기본값 | 무엇을 더하는가 |
| --- | --- | --- | --- |
| `ag-ui` | `sse` | 켜짐 | `SseFormatter`와 `text/event-stream` framing. 추가 dependency 없음. |
| `ag-ui` | `verify` | 켜짐 | `serve`의 protocol ordering state machine. 끄면 통째로 사라집니다. |
| `ag-ui` | `serve` | 꺼짐 | agent를 host합니다. `futures`, `json-patch`. |
| `ag-ui` | `client` | 꺼짐 | agent를 소비합니다. transport를 가리지 않습니다. `futures`, `json-patch`. |
| `ag-ui` | `http` | 꺼짐 | `reqwest`를 쓰는 transport. `client`와 `sse`를 함의합니다. |
| `ag-ui` | `axum` | 꺼짐 | axum binding. `serve`와 `sse`를 함의하고, tokio를 끌어오는 유일한 feature입니다. |
| `ag-ui` | `protobuf` | 꺼짐 | binary transport의 media type과 content negotiation. encoder는 없음. `events.proto`가 33개 event type 중 18개만 다룹니다. |
| `ag-ui` | `schemars` | 꺼짐 | 공개 type에 `schemars::JsonSchema`를 derive합니다. |
| `ag-ui` | `utoipa` | 꺼짐 | 공개 type에 `utoipa::ToSchema`를 derive합니다. |
| `ag-ui-a2ui` | `toolkit` | 켜짐 | agent 쪽 authoring. op builder, prompt 조립, 복구 loop. |
| `ag-ui-a2ui` | `ag-ui` | 켜짐 | `ag-ui`와의 상호운용. `toolkit`을 함의합니다. |

각 feature가 무엇을 치르고 언제 끄면 되는지는
[feature flag](/ag-ui-rust/ko/reference/features/)가 설명합니다.

## 여기 없는 것

이 tree 어디에도 LLM crate는 없습니다. 빠뜨린 것이 아니라 결정입니다. .NET SDK가
`Microsoft.Extensions.AI` 위에 서는 것은 .NET에 축복받은 chat 추상화가 하나 있기
때문입니다. Rust에는 없습니다. 생태계가 `async-openai`, `rig-core`, `genai`로 갈라져
있습니다. 그래서 `trait Agent`가 *곧* 경계입니다. model client는 각자 가져옵니다.
framework 연동은 별도 crate 안의 `impl Agent for …`입니다. 예제 둘 다 `reqwest`와
`serde` struct 두 개로 진짜 model과 대화합니다. 그것이 증명입니다.

renderer도 없습니다. `ag-ui-a2ui`는 A2UI를 만들고 검증하고 실어 나릅니다. 그것을
그리려면 widget toolkit과 event loop와 반응형 data model이 필요합니다. 그것은 다른
program입니다.

## 다음

- [시작하기](/ag-ui-rust/ko/start/) — 의존성 선언, 그리고 돌아가는 agent 하나.
- [Agent trait](/ag-ui-rust/ko/server/agent/) — `ag_ui::serve`가 요구하는 것.
- [session](/ag-ui-rust/ko/client/session/) — `ag_ui::client`가 내어 주는 것.
- [API 문서](/ag-ui-rust/api/ag_ui/index.html) — crate 둘 모두의 rustdoc.
