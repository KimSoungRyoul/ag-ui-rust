---
title: event reference
description: 프로토콜의 33개 event 타입 전부, 각각을 담는 Rust variant, 그리고 이들이 속한 family.
---

AG-UI run은 event의 나열입니다. wire에서는 각각이 JSON 객체입니다. `type` field에
SCREAMING_SNAKE_CASE 이름이 들어갑니다. Rust에서는 각각이
[`Event`](/ag-ui-rust/api/ag_ui_core/event/enum.Event.html)의 variant입니다.
[`EventType`](/ag-ui-rust/api/ag_ui_core/event/enum.EventType.html)은 그
discriminator만 따로 뗀 것입니다.

모두 **33개**입니다. 그 숫자는 `EventType::ALL.len()`입니다.
`cargo run -p xtask -- drift-check`가 모든 pull request에서 upstream TypeScript
schema의 snapshot과 맞대어 보는 것도 그 숫자입니다.
[검증 체계](/ag-ui-rust/ko/design/verification/)를 보십시오.

두 enum 모두 일부러 exhaustive합니다. 그래서 프로토콜에 무언가 추가되면 match하는
자리에서 compile 오류가 납니다. `_` 갈래가 삼켜 버리지 않습니다.
[설계 원칙](/ag-ui-rust/ko/design/commitments/)이 그 이유와 대가를 설명합니다.

## event 목록

variant마다 자기 이름을 딴 payload struct를 감쌉니다. `Event::TextMessageStart`는
`TextMessageStartEvent`를 싣습니다. 아래로 쭉 내려가며 전부 그렇습니다. payload의
field는 `type` 옆에 나란히 직렬화됩니다. 어떤 key 아래에 중첩되지 않습니다. 모든
payload는 `BaseEvent`의 optional field인 `timestamp`와 `rawEvent`도 같은 객체 안에
평평하게 함께 싣습니다.

아래 순서는 `EventType::ALL`의 순서이고, 그것이 upstream의 순서입니다.

| wire 이름 | Rust variant | family | 의미 |
| --- | --- | --- | --- |
| `TEXT_MESSAGE_START` | `TextMessageStart` | Text | `messageId` 아래에 텍스트 메시지를 엽니다. `role`의 기본값은 `assistant`이고, JSON `null`은 생략으로 읽힙니다. |
| `TEXT_MESSAGE_CONTENT` | `TextMessageContent` | Text | 열린 메시지에 `delta`를 덧붙입니다. |
| `TEXT_MESSAGE_END` | `TextMessageEnd` | Text | 메시지를 닫습니다. |
| `TEXT_MESSAGE_CHUNK` | `TextMessageChunk` | Text | start와 content와 end를 그 자체로 완결된 event 하나로 접은 것. |
| `TOOL_CALL_START` | `ToolCallStart` | Tool | call을 엽니다. tool 이름과, 뒤의 모든 것을 묶는 `toolCallId`를 답니다. |
| `TOOL_CALL_ARGS` | `ToolCallArgs` | Tool | 인자 JSON의 조각을 덧붙입니다. 조각은 이어 붙습니다. 하나만 떼면 대개 올바른 JSON이 아닙니다. |
| `TOOL_CALL_END` | `ToolCallEnd` | Tool | call을 닫습니다. 인자가 완성되었습니다. |
| `TOOL_CALL_CHUNK` | `ToolCallChunk` | Tool | start와 args와 end를 그 자체로 완결된 event 하나로 접은 것. |
| `TOOL_CALL_RESULT` | `ToolCallResult` | Tool | 그 call의 result. thread에 덧붙는 `tool` 메시지 형태입니다. |
| `THINKING_START` | `ThinkingStart` | Thinking (deprecated) | thinking block을 엽니다. 제목은 optional입니다. `REASONING_START`를 쓰십시오. |
| `THINKING_END` | `ThinkingEnd` | Thinking (deprecated) | thinking block을 닫습니다. `REASONING_END`를 쓰십시오. |
| `THINKING_TEXT_MESSAGE_START` | `ThinkingTextMessageStart` | Thinking (deprecated) | thinking 메시지를 엽니다. `REASONING_MESSAGE_START`를 쓰십시오. |
| `THINKING_TEXT_MESSAGE_CONTENT` | `ThinkingTextMessageContent` | Thinking (deprecated) | thinking 텍스트를 덧붙입니다. message id를 싣지 않습니다. 그래서 block 하나가 동시에 가질 수 있는 메시지가 하나뿐이었고, 그것이 교체된 이유입니다. |
| `THINKING_TEXT_MESSAGE_END` | `ThinkingTextMessageEnd` | Thinking (deprecated) | thinking 메시지를 닫습니다. `REASONING_MESSAGE_END`를 쓰십시오. |
| `STATE_SNAPSHOT` | `StateSnapshot` | State | shared state를 통째로 교체합니다. 자유 형식 JSON이고, 프로토콜에는 불투명합니다. |
| `STATE_DELTA` | `StateDelta` | State | RFC 6902 연산으로 shared state를 patch합니다. 순서대로 적용됩니다. |
| `MESSAGES_SNAPSHOT` | `MessagesSnapshot` | State | 메시지 이력을 교체합니다. 재연결 후, 또는 agent가 이력을 다시 쓸 때. |
| `ACTIVITY_SNAPSHOT` | `ActivitySnapshot` | Activity | client가 정의한 `activityType` 아래로 activity의 내용을 발행합니다. `replace`의 기본값은 `true`입니다. |
| `ACTIVITY_DELTA` | `ActivityDelta` | Activity | RFC 6902 연산으로 activity의 내용을 patch합니다. |
| `RAW` | `Raw` | Escape hatch | provider event를 그대로 전달합니다. `source`는 optional입니다. |
| `CUSTOM` | `Custom` | Escape hatch | 이름이 붙은, 애플리케이션이 정의한 event. 프로토콜이 보증하는 것은 봉투뿐입니다. |
| `RUN_STARTED` | `RunStarted` | Lifecycle | 모든 run의 첫 event. `threadId`, `runId`, 그리고 optional로 부모 run과 그 run을 시작시킨 입력. |
| `RUN_FINISHED` | `RunFinished` | Lifecycle | run이 실패 없이 끝났습니다. `outcome`이 성공과 interrupt를 구분합니다. interrupt는 사람의 입력을 기다리며 멈춘 run입니다. |
| `RUN_ERROR` | `RunError` | Lifecycle | run이 실패했습니다. 뒤따르는 것은 없습니다. |
| `STEP_STARTED` | `StepStarted` | Lifecycle | run 안에서 이름 붙은 step을 엽니다. |
| `STEP_FINISHED` | `StepFinished` | Lifecycle | 그 step을 닫습니다. |
| `REASONING_START` | `ReasoningStart` | Reasoning | 어느 message id에 대한 reasoning block을 엽니다. |
| `REASONING_MESSAGE_START` | `ReasoningMessageStart` | Reasoning | reasoning 메시지를 엽니다. `TEXT_MESSAGE_START`와 달리 `role`이 필수이고, 언제나 `reasoning`입니다. |
| `REASONING_MESSAGE_CONTENT` | `ReasoningMessageContent` | Reasoning | reasoning 텍스트를 덧붙입니다. |
| `REASONING_MESSAGE_END` | `ReasoningMessageEnd` | Reasoning | reasoning 메시지를 닫습니다. |
| `REASONING_MESSAGE_CHUNK` | `ReasoningMessageChunk` | Reasoning | start와 content와 end를 그 자체로 완결된 event 하나로 접은 것. |
| `REASONING_END` | `ReasoningEnd` | Reasoning | reasoning block을 닫습니다. |
| `REASONING_ENCRYPTED_VALUE` | `ReasoningEncryptedValue` | Reasoning | provider의 불투명한 reasoning blob. zero-data-retention 모드를 위한 것입니다. `subtype`이 `entityId`가 `tool-call`을 가리키는지 `message`를 가리키는지 말합니다. |

Text 4개, Tool 5개, deprecated된 Thinking 5개, State 3개, Activity 2개, Escape
hatch 2개, Lifecycle 5개, Reasoning 7개입니다.

## wire에서

`type`이 tag이고, payload는 그 옆에 평평하게 놓입니다.

```rust
use ag_ui_core::{Event, EventType};

fn main() {
    // 프로토콜이 정의하는 모든 event 타입, upstream 순서 그대로.
    assert_eq!(EventType::ALL.len(), 33);

    // discriminator는 양방향 모두 wire 이름입니다.
    assert_eq!(EventType::TextMessageContent.as_str(), "TEXT_MESSAGE_CONTENT");
    assert_eq!(
        "TEXT_MESSAGE_CONTENT".parse::<EventType>().unwrap(),
        EventType::TextMessageContent,
    );

    let event = Event::text_message_content("msg-1", "Hello");
    assert_eq!(event.event_type(), EventType::TextMessageContent);
    assert_eq!(
        serde_json::to_string(&event).unwrap(),
        r#"{"type":"TEXT_MESSAGE_CONTENT","messageId":"msg-1","delta":"Hello"}"#,
    );
}
```

이 build가 모르는 event 타입은 deserialize에 실패합니다. 의도된 것입니다. 더 새로운
agent와 이야기하는 frontend는 모르는 타입의 이름을 대며 오류로 멈춥니다. 대화의
4분의 3만 조용히 그리지 않습니다.

## `THINKING_*` family는 deprecated입니다

다섯 개 모두 여전히 프로토콜에 있고, 여전히 파싱되고, 그 변경보다 앞선 producer가
여전히 emit합니다. 그래서 여기에 있고, SDK도 이들을 싣습니다. `REASONING_*` event가
이들을 대체합니다. 대체본은 원본이 물러난 이유를 고칩니다.
`THINKING_TEXT_MESSAGE_CONTENT`는 message id를 싣지 않습니다. 그래서 thinking
block은 동시에 메시지 하나만 가질 수 있었습니다.

Rust variant와 payload struct에는 `#[deprecated]`가 붙습니다. `ag-ui-core` 자신의
event module은 `#![allow(deprecated)]`를 답니다. 이 module은 union에서도,
`event_type()`에서도, factory에서도 이 타입들의 이름을 대야 합니다. spec을 쓰인
대로 구현했다고 자기 자신에게 경고하는 것은 아무에게도 도움이 안 됩니다. 이 억제는
그 module 안에서만 유효합니다. 그래서 이들 중 하나를 쓰는 consumer는 자기 사용
지점에서 경고를 받습니다. 계속 쓸지 정하는 자리가 거기입니다.

`Event::is_deprecated`는 match 없이 runtime에 답합니다.

```rust
use ag_ui_core::Event;

fn main() {
    let event: Event = serde_json::from_str(r#"{"type":"THINKING_END"}"#).unwrap();

    assert_eq!(event.event_type().as_str(), "THINKING_END");
    assert!(event.is_deprecated());

    let current = Event::reasoning_end("msg-1");
    assert!(!current.is_deprecated());
}
```

:::note
`#[deprecated]` 표시에는 예외가 하나 있습니다. `utoipa` feature를 켜면 payload
struct에서는 이 attribute가 억제됩니다. utoipa 5.5의 derive가 `#[serde(flatten)]`
struct에 쓰는 `AllOf` builder에 `.deprecated()` 호출을 냅니다. 그 builder에는 그런
method가 없어서 crate가 compile되지 않습니다. `Event::thinking_*` 생성자에서는
deprecation이 조건 없이 유지됩니다. utoipa는 그것을 보지 않습니다.
:::

## `*_CHUNK` event

event 세 개가 start와 그 content와 end를 그 자체로 완결된 event 하나로 접습니다.
`TEXT_MESSAGE_CHUNK`, `TOOL_CALL_CHUNK`, `REASONING_MESSAGE_CHUNK`입니다. 출력을
짝으로 묶을 수 없는 producer를 위해 존재합니다. 대부분의 provider adapter가
그렇습니다. upstream API가 메시지의 끝을 다음 메시지가 시작되기 전에는 알려 주지
않기 때문입니다.

id와 이름은 **첫 chunk에만** 실립니다. 그래서 한 stream의 끝은 다음 stream의
시작에서, 아니면 run의 끝에서만 알 수 있습니다.

```text
TEXT_MESSAGE_CHUNK { messageId: "msg-1", delta: "Hel" }
TEXT_MESSAGE_CHUNK { delta: "lo" }
TEXT_MESSAGE_CHUNK { messageId: "msg-2", delta: "Bye" }   <- msg-1이 방금 끝났습니다
```

소비하는 쪽에서 그 장부 정리는 `ag_ui_client::chunks`가 맡습니다. 연달아 이어진
chunk를 다른 무엇이 보기 전에 start/content/end 세 짝으로 되펼칩니다. emit하는
쪽에는 일부러 **handle이 없습니다**. `ag-ui-server`의 typestate emitter는 연 것이
닫히도록 보장하려고 있습니다. chunk에는 닫을 것이 없습니다. RAII handle로 감싸면
틀릴 방법만 하나 늘어납니다. 이들은 `ctx.emit`으로 emit하십시오. API를 기다리는
빈틈이 아니라 지원되는 경로입니다.

뒤섞인 병렬 tool call이 `ctx.emit`에 속하는 나머지 사례입니다. `ToolCallHandle` 두
개를 동시에 여는 것은 *설계상* borrow check 오류입니다. 그래서
`args(a) args(b) args(a) end(a) end(b)`를 흘리는 provider를 call당 handle 하나로
그대로 옮길 수 없습니다. 방법은 둘입니다. call마다 인자를 모아 두었다가 완성되면
통째로 emit하십시오. 두 call의 인자가 서로 섞여 들어갈 수 없는 유일한 매핑입니다.
아니면 뒤섞인 그대로 직접 emit하십시오. ordering verifier는 모든 것을 id로
색인하므로 뒤섞인 stream을 받아들입니다. 허락하지 않는 것은 열지 않은 call을 닫는
일입니다. [검증 체계](/ag-ui-rust/ko/design/verification/)를 보십시오.

## 바이너리 transport가 싣지 못하는 것

프로토콜은 protobuf 인코딩도 정의합니다. 그것은 손실 있는 부분집합입니다. upstream
`events.proto`의 `Event` 메시지는 33개 타입 중 **18개**만 담는 `oneof`입니다.

`TEXT_MESSAGE_START`, `TEXT_MESSAGE_CONTENT`, `TEXT_MESSAGE_END`,
`TEXT_MESSAGE_CHUNK`, `TOOL_CALL_START`, `TOOL_CALL_ARGS`, `TOOL_CALL_END`,
`TOOL_CALL_CHUNK`, `STATE_SNAPSHOT`, `STATE_DELTA`, `MESSAGES_SNAPSHOT`, `RAW`,
`CUSTOM`, `RUN_STARTED`, `RUN_FINISHED`, `RUN_ERROR`, `STEP_STARTED`,
`STEP_FINISHED`입니다.

나머지 15개는 바이너리 표현이 아예 없습니다. `REASONING_*` 일곱 개 전부,
`ACTIVITY_*` 두 개 모두, deprecated된 `THINKING_*` 다섯 개 전부, 그리고
`TOOL_CALL_RESULT`입니다. reasoning을 하거나, activity를 보고하거나, tool result를
돌려주는 agent는 자기 stream을 그 형식으로 표현할 수 없습니다. 대부분의 agent가
그렇습니다.

그래서 `ag-ui-core`는 그중 무엇도 encode하지 않습니다. `protobuf` feature는 build가
media type을 협상하고 그 이름을 댈 수 있도록 존재합니다. formatter의 `encode`는
언제나 `Error::UnsupportedTransport`로 실패합니다. 프로토콜의 절반 가까이를 조용히
버리는 것은 거절하는 것보다 나쁩니다. 33개를 모두 싣는 SSE를 쓰십시오.
[`encode::protobuf`](/ag-ui-rust/api/ag_ui_core/encode/protobuf/index.html)
module은 다뤄지는 집합을 `COVERED_EVENT_TYPES`로 나열하고 `is_covered`를
제공합니다. 그래서 주어진 stream이 바이너리 transport에서 살아남았을지 test로
단언할 수 있습니다.

port를 proto 정의가 아니라 TypeScript Zod schema를 보고 쓴 이유도 이것입니다. 33개
중 15개가 빠진 진실의 원천은 원천 노릇을 할 수 없습니다.
