---
title: 설계 원칙
description: 이 SDK가 스스로에게 지우는 약속, 그 근거, 그리고 각각의 대가.
---

이 페이지는 이 SDK를 바깥에서 본 논거입니다. 이 위에 무언가를 만들지 고민
중이라면, 여기 적힌 것이 이 SDK의 약속이고 그 약속의 값입니다.

하나하나가 품질에 대한 주장이고, 그 각각은 이 page가 단언해서가 아니라 build를
빨갛게 만드는 무언가가 강제합니다. 약속에 대가가 따르는 곳에서는 그 대가도 함께
적었습니다.

## 네 가지 약속

**잘못 쓸 수 없는 emitter.** AG-UI의 streaming 구성물은 모두 짝으로 묶여
있습니다. `TEXT_MESSAGE_START` … `TEXT_MESSAGE_END`,
`TOOL_CALL_START` … `TOOL_CALL_END`, `STEP_STARTED` … `STEP_FINISHED`입니다.
구성물마다 raw emit 호출 세 개를 agent에게 넘긴다면, agent가 연 것을 순서대로
닫으리라고 믿는 셈입니다. 이른 return을 포함한 모든 경로에서 말입니다. 이 SDK는
대신 RAII handle을 넘깁니다. handle을 만들면 여는 event가 emit됩니다. handle은
run context를 mutable로 빌립니다. 그래서 겹치는 두 번째 handle은 **borrow check
error**입니다. `Drop`이 종료 event를 emit합니다. 그래서 `end()`를 잊어도, message
중간에서 `?`로 `Err`를 반환해도, stream은 올바른 형태로 남습니다.

**server에서 검증하는 ordering.** protocol이 금지하는 것을 type system으로 다
표현할 수는 없습니다. borrow checker가 못 잡는 것은 runtime state machine이
잡습니다. 앞선 `START` 없는 `TEXT_MESSAGE_CONTENT`는 원인이 생긴 자리에서
보고됩니다. network를 세 번 건너간 곳에서 혼란에 빠진 frontend로 드러나지
않습니다. TypeScript SDK는 client에서 검증하고, .NET SDK는 아예 검증하지
않습니다. server 쪽에서 ordering을 검사하는 것은 어느 쪽도 아닙니다.

**exhaustive한 `Event`.** protocol에 event가 추가되면 consumer 쪽에서 compile
error가 납니다. `_` 갈래가 삼켜 버리지 않습니다.

**CI의 drift check.** 이 port는 upstream의 TypeScript schema를 보고 손으로
썼습니다. 둘을 잇는 것이 compiler에는 없습니다.
`cargo run -p xtask -- drift-check`가 그 연결입니다. upstream의 event 집합이
움직이면 build가 실패합니다. 그래서 port가 조용히 뒤처질 수 없습니다.

[검증 체계](/ag-ui-rust/ko/design/verification/)는 두 번째와 네 번째를 자세히
다룹니다. [테스트](/ag-ui-rust/ko/design/testing/)는 네 가지를 정직하게 유지하는
방법입니다. 이 페이지의 나머지는 그 약속들이 딛고 선 결정의 근거입니다.

## 진실의 원천은 TypeScript Zod schema입니다

protobuf 정의가 아닙니다. upstream `events.proto`의 `Event` message는 36개 event
type 중 21개만 담는 `oneof`입니다. reasoning도, activity도, thinking도,
`tool_call_result`도 없습니다. binary transport는 protocol의 손실 있는
부분집합입니다. port의 대상이 될 수 없습니다. upstream에는 생성에 쓸 JSON Schema
export도 없습니다.

그래서 port는 `core/src/events.ts`를 보고 손으로 썼습니다. drift check가 그것을
정직하게 유지합니다. 생성이 아니라 탐지입니다. upstream의 `EventType` enum과 Zod
객체 key를 parse해서, Rust 쪽과 어긋나면 build를 실패시킵니다. 완전한 code 생성은
Zod-to-Rust compiler를 만들어 유지한다는 뜻입니다. 아직 그럴 값어치는 없습니다.

[event reference](/ag-ui-rust/ko/reference/events/)가 binary transport가 싣는
18개와 싣지 못하는 15개를 짚어 줍니다.

## `Event`는 일부러 exhaustive하고, error는 아닙니다

이 workspace의 error enum은 모두 `#[non_exhaustive]`입니다.
[`Event`](/ag-ui-rust/api/ag_ui/event/enum.Event.html)와
[`EventType`](/ag-ui-rust/api/ag_ui/event/enum.EventType.html)은 아닙니다.
이 비대칭은 의도입니다. protocol은 지난 1년 사이 두 번 자랐습니다. `REASONING_*`와
`ACTIVITY_*`입니다. 그러니 이것은 가정이 아니라 실제로 시험받는 문제입니다.

이 SDK가 바로잡으려는 실패는 조용한 coverage 누락입니다. `#[non_exhaustive]`는
그것을 제도화합니다. 모든 consumer에게 `_` 갈래를 쓰게 만듭니다. 그리고 `_`
갈래야말로 "34번 event가 도착했다"를 아무 진단도 없는 상태로 바꿉니다. 새 event를
처리하는 일은 그대로 남습니다. 그런 일이 있다는 통지만 사라집니다.

그러므로 새 protocol event는 Rust consumer에게 compile error여야 *합니다*.
`serde_json::Value` 대신 type이 붙은 SDK를 쓰는 이유가 그것입니다. drift checker가
이야기를 완성합니다. upstream이 event를 추가하면 이 저장소의 build가 실패합니다.
이 crate가 variant를 추가하면, 하위의 모든 match가 compile되지 않습니다. 누군가
그 event가 자기에게 무엇을 뜻하는지 정해야 풀립니다. 고리 세 개가 하나같이
시끄럽습니다.

**대가는 정직하게 받아들입니다. event 하나를 추가하는 것은 이 SDK의 major
version입니다.** 그래야 합니다. wire 계약이 바뀌었으니까요. `Event`를 직접
match한다면 그 비용을 예산에 넣으십시오. 그러고 싶지 않다면 상위의
[`Update`](/ag-ui-rust/api/ag_ui/client/session/enum.Update.html) stream을
match하십시오. 이쪽은 그 attribute를 답니다.

error는 논리가 뒤집힙니다. 그래서 attribute를 답니다. 실패 모드를 exhaustive하게
match하고 싶어 하는 사람은 없습니다. 호출자는 몇 개의 variant로 분기하고 나머지는
흘려보냅니다. 새 실패 모드는 protocol 변경이 아닙니다.

### `RunEnd`와 `Update`는 어디에 서는가

두 client type은 그 선의 반대편에 각각 서 있습니다. 그 갈림이 규칙을 보여 줍니다.

[`RunEnd`](/ag-ui-rust/api/ag_ui/client/session/enum.RunEnd.html)는 `Event`
쪽입니다. exhaustive합니다. run은 protocol이 정의한 세 가지 방식으로만 끝납니다.
frontend가 가장 검사받고 싶어 하는 match도 그것입니다. 입력을 다시 살릴지
결정하니까요. 그리고 run이 끝나는 네 번째 방식은 wire 계약 변경이 맞습니다.

```rust
use ag_ui::client::RunEnd;

fn on_end(end: &RunEnd) -> String {
    // `_` 갈래가 없습니다. run이 끝나는 네 번째 방식이 생기면 이 code는
    // compile되지 않습니다. 그것이 요점입니다. protocol이 바뀌었고, 이 함수는
    // 결정을 내려야 합니다.
    match end {
        RunEnd::Success { .. } => "done".to_owned(),
        RunEnd::Interrupted { interrupts } => {
            format!("waiting on {} interrupt(s)", interrupts.len())
        }
        RunEnd::Failed { message, .. } => format!("failed: {message}"),
    }
}

fn main() {
    let end = RunEnd::Failed {
        message: "the weather service is down".to_owned(),
        code: Some("AGENT_ERROR".to_owned()),
    };
    assert_eq!(on_end(&end), "failed: the weather service is down");
}
```

`Update`는 `#[non_exhaustive]`를 유지합니다. wire type이 아니라 view model입니다.
다시 그릴 만한 종류가 하나 늘어나는 것은 protocol 변경이 아닙니다.

runtime 쪽도 type 쪽과 같은 편입니다. 이 build가 모르는 event type은
deserialize에 실패합니다. session이 그것을 보고하고 run을 `RunEnd::Failed`로
끝냅니다. 더 새로운 agent와 이야기하는 frontend는 모르는 type의 이름을 대며
error로 멈춥니다. 대화의 4분의 3만 조용히 그리지 않습니다.

## LLM 추상화는 없습니다

.NET의 `AGUI.Server`는 `Microsoft.Extensions.AI`의 `IChatClient` 위에 있습니다.
.NET에는 축복받은 chat 추상화가 하나뿐이라 그것이 통합니다. Rust는 아닙니다.
최근 90일 다운로드는 `async-openai` 230만, `rig-core` 130만, `genai` 11만
3천쯤입니다. 가장 가까운 대응물인 `agent-framework-core`는 1천 언저리입니다.
하나를 고르는 것은 편을 드는 일입니다. 잘못 고르면 이 SDK를 쓰는 모두가 그
의존성을 지고 다닙니다.

그래서 **`trait Agent`가 곧 경계입니다.** 이 SDK는 어떤 LLM crate에도 의존하지
않습니다. client는 각자 가져오고, trait 하나만 구현하면 됩니다. framework 통합은
자기 crate 안의 `impl Agent for …`가 됩니다. 그것은 오로지 그쪽의 문제입니다.

이 주장은 주장으로 두지 않았습니다. workspace의 live smoke test는 평범한
`reqwest`로 실제 streaming model에 닿습니다. 구현하는 것은 `Agent`뿐입니다.
그래서 `e2e/Cargo.toml`에 LLM 의존성이 없다는 사실이 곧 증거입니다.
[테스트](/ag-ui-rust/ko/design/testing/)를 보십시오.

## web binding 아래는 executor에 의존하지 않습니다

`ag-ui`, `ag_ui::server`, `ag_ui::client`는 `futures` primitive를 씁니다. emit
경로에 `tokio::sync::mpsc`가 아니라 `futures::channel::mpsc`를 씁니다. tokio는
`ag_ui::axum`에만 있습니다. 그래야 wasm target과 tokio가 아닌 executor가 가능한
선택지로 남습니다.

CI는 이것을 두 가지 방법으로 강제합니다. 두 번째가 있는 이유는 첫 번째로 부족하기
때문입니다. 그 crate들을 `wasm32-unknown-unknown`으로 build하고, *또한* 의존성
graph에 tokio가 없음을 단언합니다. tokio의 `rt`, `sync`, `macros`, `io-util`,
`time` feature는 모두 wasm으로 compile됩니다. 그래서 `ag_ui::server`에 `tokio`를
추가해도 wasm 검사는 전부 통과합니다. 실제로 그렇게 해 보고 wasm build가 초록으로
남는 것을 확인했습니다. 보증을 지는 것은 의존성 graph입니다. 그래서 단언하는
대상도 그것입니다.

## 동기 emit, Rust에 async `Drop`이 없으므로

첫 번째 약속의 대가입니다. 분명히 짚어 둘 값어치가 있습니다.
`msg.delta(text)?`에는 `.await`가 붙지 않습니다.

`Drop`은 async일 수 없습니다. 그래서 handle이 종료 event를 emit하면서 `await`할
수 없습니다. emit 경로는 끝에서 끝까지 동기입니다. handle이 unbounded channel로
밀어 넣고, transport 계층이 비웁니다. 이 API의 첫 초안은 TypeScript와 .NET SDK를
따라 `msg.delta(t).await?`였습니다. 그것은 RAII 보증과 공존할 수 없습니다. 둘 중
하나는 사라져야 했습니다.

borrow가 금지하는 것은 두 번째로 열린 block뿐입니다. protocol의 규칙이 그것이고,
그 밖에는 없습니다. 그래서 handle은 run context 자체가 아니라 그 *field* 두 개를
쥡니다. event sink와 state입니다. 덕분에 call이 열려 있는 동안에도
`call.state_mut()`과 `call.publish_state()`가 동작합니다. tool이 자기 일을 하는
자리가 거기입니다. `STATE_*`는 wire에서 ordering이 없으므로 verifier도
동의합니다.

이전 초안은 sink만 쥐었습니다. 그래서 무언가 열려 있는 동안 state에 닿을 수
없었습니다. 모든 agent가 call을 알리기 *전에* 그 call을 위한 변경을 먼저 해야
했습니다. 같은 event, 다른 순서입니다. 그리고 client가 call이 도착하는 것을
지켜볼 수 있는지, 아니면 이미 끝난 것만 보는지를 정하는 것이 그 순서입니다. sink
곁에 state를 함께 쥐면 handle이 닿는 범위만 넓어집니다. 열 수 있는 범위는
그대로입니다. 그 뒤에는 두 번째 block을 열 run context가 여전히 없습니다.

## ID는 문자열입니다

spec은 `threadId`, `runId`, `messageId`를 문자열로 규정합니다. 기존 커뮤니티
crate 하나는 이들을 UUID로 parse합니다. LangGraph 앞에서 곧바로 깨집니다.
LangGraph는 `"thread-abc"` 같은 thread id와 평범한 정수인 run id를 보냅니다. 자체
id 체계를 쓰는 다른 무엇 앞에서도 마찬가지입니다(upstream 이슈 #2195, #2196).
`String` 위의 newtype은 type 구분을 지키면서, protocol에 없는 제약을 만들지
않습니다.

```rust
use ag_ui::{RunId, ThreadId};

fn main() {
    // producer가 보낸 값이 무엇이든 byte 단위 그대로 왕복합니다.
    let thread = ThreadId::new("thread-abc");
    let run = RunId::new("42");

    assert_eq!(thread.as_str(), "thread-abc");
    assert_eq!(run.as_str(), "42");

    // 서로 다른 type이라 한쪽을 다른 쪽 자리에 넘길 수 없습니다.
    assert_eq!(serde_json::to_string(&thread).unwrap(), r#""thread-abc""#);
}
```

그런 것이 필요하면 UUID를 만들어 문자열로 넘기십시오. 이 SDK는 `uuid` 의존성이
없고, 이에 대한 의견도 없습니다.

## crate는 일곱이 아니라 둘

첫 초안은 .NET의 assembly 분할을 일대일로 흉내 냈습니다. 잘못된 직관입니다.
.NET에서 assembly는 배포와 version 관리의 단위라 쪼개는 것이 싸고 자연스럽습니다.
Rust에서는 **feature가 일차 도구**입니다. crate를 쪼개는 일은 의존성 격리나
독립적인 version 관리로 정당화되어야 합니다.

그 규칙이 일곱을 다섯으로 접었고, 그 결론을 자기 자신에게 적용하자 다섯이 둘이
되었습니다. crate 다섯이라는 구성은 자기 시험을 통과하지 못했습니다. feature
gate는 분할과 똑같이 의존성을 격리합니다. `--no-default-features`는 axum도
tokio도 reqwest도 compile하지 않고, CI가 그것을 feature 단위로 단언합니다. 남은
정당화는 독립 version 관리인데, 이 workspace는 그것을 하지 않습니다.
`workspace.version` 하나로 전부 함께 release합니다.

그래서 `ag-ui`가 crate 하나입니다. protocol type은 언제나 compile되고, `server`와
`client`와 `axum`이 같은 이름의 feature 뒤에 있습니다. runtime마다 자기 `Error`를
자기 module에 둡니다. `ag_ui::Error`는 protocol 오류이고 `ag_ui::server::Error`는
hosting 오류입니다.

`ag-ui-a2ui`는 feature가 할 수 없는 격리 논거로 따로 남습니다. A2UI는 별개
protocol이고 AG-UI 없이 A2A나 MCP 위에서 구동되며, 그 사용자가 `ag-ui`라는 이름의
crate에 의존할 이유가 없습니다. 앞서 접힌 둘은 접힌 이유 그대로 남습니다. SSE
encoder는 `ag_ui::encode`이고, 추가 의존성 없는 수백 줄입니다. A2UI toolkit은
`ag-ui-a2ui`의 feature입니다.

비용은 분할이 사 주고 feature가 못 사 주는 그 하나입니다. cargo는 dependency
graph 전체에서 feature를 합칩니다. 한쪽이 `server`를, 다른 쪽이 `client`를
요청하는 build는 둘 다 compile합니다. 섞인 graph에서의 compile 시간이지 runtime
이나 정확성 비용은 아닙니다. [crate 구성](/ag-ui-rust/ko/start/crates/)이 그
안내입니다.

## 확장 지점은 둘이 아니라 하나

초기 초안은 두 가지를 함께 들고 있었습니다. 하나는 `map_content` / `map_call` /
`map_result` / `map_interrupt` closure를 받는 `StreamOptions` builder입니다. .NET
`AGUIStreamOptions`를 그대로 옮긴 것입니다. 다른 하나는 middleware chain입니다.
같은 일을 하는 두 가지 방법이고, closure 쪽은 Rust에서 `Box<dyn Fn>` 더미가
됩니다.

모든 것은 `StreamTransformer`로 조합됩니다. 예전 hook들은 내장 transformer로
제공됩니다. transformer는 추가된 순서대로 돌며 앞의 것이 만든 결과를 봅니다.
그리고 그 전부가 ordering verifier보다 앞섭니다. event를 버리는 일이 안전한 이유가
그것입니다. verifier는 제거된 tool call의 나머지 반쪽을 아예 보지 못합니다.

## 제공된 tool 목록은 allow-list가 아니라 capability list입니다

`RunAgentInput.tools`는 *client*가 무엇을 실행할 수 있는지를 말합니다. agent가
무엇을 호출해도 되는지는 말하지 않습니다. 여기 있는 어떤 것도 그것을 allow-list로
다루지 않습니다. 그 목록에 없는 이름으로 `TOOL_CALL_START`를 emit해도 stream은
올바른 형태입니다. ordering verifier는 그에 대해 아무 말도 하지 않습니다.

이를 결정짓는 사례는 agent가 스스로 답하는 tool입니다. A2UI agent는 surface를
frontend로 실어 나르려고 `render_a2ui`를 emit합니다. frontend가 그것을 그립니다.
어떤 client도 그것을 "제공"한 적이 없습니다. client가 실행할 것이 애초에 없기
때문입니다. 결과를 agent가 run 안에서 직접 계산해 보고하는 server 측 tool도 같은
모양입니다. agent가 무엇을 했는지 transcript에 남기려고 emit하는 call도
마찬가지입니다.

알아보지 못하는 call을 client가 어떻게 다룰지는 client의 결정입니다. 무시하든,
activity로 그리든, 보고하든 말입니다. protocol이 제약하는 것은 *ordering*입니다.
`START` 없는 `TOOL_CALL_ARGS`, call이 끝나기 전에 온 result가 그것입니다. 검사받는
것도 그것입니다.

더 엄격한 규칙을 원하는 agent는 한 줄이면 됩니다. `RunContext::tool`이 제공되지
않은 이름에 `None`을 돌려주기 때문입니다. `task-board` 예제가 그렇게 합니다. 단,
client가 실행하리라고 진짜로 기대하는 tool에 대해서만 그렇게 합니다.

## A2UI는 v0.9에 고정됩니다

A2UI spec은 v1.0입니다. 하지만 출시된 toolkit은 TypeScript도 .NET도 Python도
여전히 `v0.9`를 찍습니다. .NET의 상수 파일은 이 값들을 "cross-language wire
contract"(언어 간 wire 계약)이라고, 그리고 "must not diverge"(어긋나서는 안
된다)라고 표시합니다. 오늘 v1.0 wire 값을 구현하면 그중 어느 것과도 상호 운용하지
못합니다. toolkit들이 움직이면 v1.0은 feature 뒤로 들어갑니다.
[A2UI](/ag-ui-rust/ko/a2ui/)를 보십시오.
