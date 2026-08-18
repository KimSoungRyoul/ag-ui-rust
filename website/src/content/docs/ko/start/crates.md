---
title: crate 구성
description: 다섯 crate가 각각 무엇을 위한 것인지. agent를 serving하려면, 소비하려면, 둘 다 하려면 어느 것이 필요한지.
---

workspace는 crate 다섯입니다. 나뉘는 선은 하나입니다. protocol이 *무엇인가*. agent가
되려면 무엇이 필요한가. agent와 말하려면 무엇이 필요한가. 그리고 일부에게만 필요한
binding 둘입니다.

| crate | 무엇인가 |
| --- | --- |
| `ag-ui-core` | protocol type, 33개 event variant 전부, 그리고 wire encoding. `serde`와 `serde_json`뿐. |
| `ag-ui-server` | agent를 띄웁니다. `Agent` trait, typestate event emitter, 자동 state delta, protocol verification. executor를 가리지 않습니다. |
| `ag-ui-axum` | agent를 axum router에 올립니다. tokio를 끌어오는 유일한 crate. |
| `ag-ui-client` | 원격 agent를 소비합니다. transport, event 적용, 실체화된 message와 state. |
| `ag-ui-a2ui` | A2UI protocol type, 의미 validator, 그리고 agent 쪽 authoring toolkit. |

## 어느 것이 필요한가

**HTTP로 agent를 serving하려면:** `ag-ui-server`와 `ag-ui-axum`. 그리고 signature에 이름을
쓰게 될 type을 위한 `ag-ui-core`. `ag-ui-axum`은 아무것도 re-export하지 않으므로 셋 다
의존성으로 넣습니다.

**agent를 다른 곳에서 serving하려면** `ag-ui-server` 하나면 됩니다. Lambda, WebSocket,
process 안의 test가 그렇습니다. `run(agent, input)`은 event `Stream`을 건네고 거기서
멈춥니다. 직렬화는 transport의 일입니다. axum이 아닌 무언가 위에 SSE를 얹어야 한다면
`ag-ui-core`의 `SseFormatter`가 frame을 만들어 줍니다.

**agent를 소비하려면:** `ag-ui-client`와 `ag-ui-core`. 기본 `http` feature가 `reqwest`
transport를 가져옵니다. 끄고 `Transport`를 직접 구현하세요. wasm을 위해, tokio가 아닌
runtime을 위해, HTTP가 아닌 socket을 위해서입니다.

**한 process에서 둘 다:** 넷 모두. 드문 일이 아닙니다. 다른 agent를 부르는 agent는
client이면서 server입니다. client 쪽 trait 이름이 `Agent`가 아니라 `RemoteAgent`인
이유가 그것입니다. 둘 다 하는 파일에서는 두 이름이 부딪힙니다.

**A2UI:** `ag-ui-a2ui`에, 위의 것 중 해당하는 것을 더합니다. 이것만 성격이 다릅니다.
이유는 다음 절에 있습니다.

## 전체 모양

```text
                      ag-ui-core
                serde · serde_json · thiserror
                            │
             ┌──────────────┼──────────────┐
             │              │              │
      ag-ui-server    ag-ui-client   ag-ui-a2ui
       futures ·       futures ·      jsonptr
      json-patch      json-patch ·   (core optional)
             │        reqwest (opt)
             │
       ag-ui-axum
    axum · tower · tokio
```

저 그림에서 무게를 지는 것이 셋입니다.

**tokio는 `ag-ui-axum`에서 들어오고 다른 곳에서는 들어오지 않습니다.** `core`와
`server`와 `client`는 `futures` primitive를 씁니다. emit 경로에
`tokio::sync::mpsc`가 아니라 `futures::channel::mpsc`를 쓰는 식입니다. 그래서 wasm
target과 tokio가 아닌 executor가 계속 돕니다. CI가 두 방향으로 강제합니다. 그
crate들을 `wasm32-unknown-unknown`으로 build해 봅니다. tokio 자체도 wasm으로
compile되므로, 그들의 dependency graph에 tokio가 없다는 것까지 확인합니다. 자세한 것은
[platform과 MSRV](/ag-ui-rust/ko/reference/platforms/)에 있습니다.

**`ag-ui-core`는 일부러 작게 둡니다.** runtime도 I/O도 async도 없습니다. type, 그
type의 정확한 JSON 표현, 그리고 그것을 실어 나르는 SSE framing뿐입니다. 그래서 양쪽
절반 아래에 놓이면서도 어느 쪽으로도 무언가를 끌고 들어가지 않습니다.

**`ag-ui-a2ui`는 나머지에 의존하지 않습니다.** A2UI는 별개의 protocol입니다. agent가
surface를 서술하는 JSON을 흘려보내면 renderer가 그것을 그립니다. 이 crate는 그 교환의
agent 쪽 절반입니다. `ag-ui` feature가 `ag-ui-core`와의 상호운용을 더합니다. 끄면 A2A나
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

## 왜 일곱이 아니라 다섯인가

첫 초안은 .NET SDK의 assembly 분할을 하나씩 그대로 옮겼습니다. 잘못된 직관입니다.
.NET에서 assembly는 배포와 version 관리의 단위입니다. 쪼개는 값이 싸고 자연스럽습니다.
Rust에서는 **feature가 일차 도구**입니다. crate를 쪼개려면 feature가 못 하는 것으로
정당화해야 합니다. dependency 격리, 아니면 독립적인 version 관리입니다.

그 시험을 통과하지 못하고 흡수된 crate가 둘입니다.

`ag-ui-encoder`는 `ag-ui-core::encode`가 되었습니다. SSE framing은 추가 dependency가
하나도 없는 수백 줄입니다. 격리할 것이 없었습니다. 무거운 부분은 protobuf뿐인데, 그것은
이미 optional dependency가 처리합니다.

`ag-ui-a2ui-toolkit`은 `ag-ui-a2ui`의 `toolkit` feature가 되었습니다. prompt 문자열과
orchestration입니다. 이것도 격리할 것이 없습니다.

따로 남은 셋은 저마다 다른 이유로 시험을 통과합니다. `ag-ui-axum`은 axum과 tower와
tokio를 끌고 들어옵니다. 이 규칙이 말하는 dependency 격리가 바로 그것입니다.
`ag-ui-client`는 그 자체로 온전히 쓸모가 있습니다. frontend가 server를 compile할 이유는
없습니다. `ag-ui-a2ui`는 다른 protocol이고, AG-UI 없이도 씁니다.

값까지 포함한 전체 논거는 `docs/DESIGN.md`에 있습니다.
[설계 원칙](/ag-ui-rust/ko/design/commitments/)에 요약해 두었습니다.

## feature 한눈에 보기

| crate | feature | 기본값 | 무엇을 더하는가 |
| --- | --- | --- | --- |
| `ag-ui-core` | `sse` | 켜짐 | `SseFormatter`와 `text/event-stream` framing. 추가 dependency 없음. |
| `ag-ui-core` | `protobuf` | 꺼짐 | binary transport의 media type과 content negotiation. encoder는 없음. `events.proto`가 33개 event type 중 18개만 다룹니다. |
| `ag-ui-core` | `schemars` | 꺼짐 | 공개 type에 `schemars::JsonSchema`를 derive합니다. |
| `ag-ui-core` | `utoipa` | 꺼짐 | 공개 type에 `utoipa::ToSchema`를 derive합니다. |
| `ag-ui-server` | `verify` | 켜짐 | protocol ordering state machine. 끄면 통째로 사라집니다. |
| `ag-ui-client` | `http` | 켜짐 | `reqwest`를 쓰는 HTTP transport. 끄면 crate가 wasm으로 build됩니다. |
| `ag-ui-a2ui` | `toolkit` | 켜짐 | agent 쪽 authoring. op builder, prompt 조립, 복구 loop. |
| `ag-ui-a2ui` | `ag-ui` | 켜짐 | `ag-ui-core`와의 상호운용. `toolkit`을 함의합니다. |

`ag-ui-axum`에는 feature가 없습니다. 각 feature가 무엇을 치르고 언제 끄면 되는지는
[feature flag](/ag-ui-rust/ko/reference/features/)가 설명합니다.

## 여기 없는 것

이 tree 어디에도 LLM crate는 없습니다. 빠뜨린 것이 아니라 결정입니다. .NET SDK가
`Microsoft.Extensions.AI` 위에 서는 것은 .NET에 축복받은 chat 추상화가 하나 있기
때문입니다. Rust에는 없습니다. 생태계가 `async-openai`, `rig-core`, `genai`로 갈라져
있습니다. 그래서 `trait Agent`가 *곧* 경계입니다. model client는 각자 가져옵니다.
framework 연동은 별도 crate 안의 `impl Agent for …`입니다. 예제 둘 다 `reqwest`와
`serde` struct 두 개로 진짜 model과 대화합니다. 그것이 증명입니다.

renderer도 없습니다. `ag-ui-a2ui`는 A2UI를 만들고 검증하고 실어 나릅니다. 그것을
그리려면 widget toolkit과 event loop과 반응형 data model이 필요합니다. 그것은 다른
program입니다.

## 다음

- [시작하기](/ag-ui-rust/ko/start/) — 의존성 선언, 그리고 돌아가는 agent 하나.
- [Agent trait](/ag-ui-rust/ko/server/agent/) — `ag-ui-server`가 요구하는 것.
- [session](/ag-ui-rust/ko/client/session/) — `ag-ui-client`가 내어 주는 것.
- [API 문서](/ag-ui-rust/api/ag_ui_core/index.html) — 다섯 crate 전부의 rustdoc.
