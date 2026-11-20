---
title: event reference
description: AG-UI 1.0의 31개 event type과 수신 호환용 THINKING variant 5개의 Rust 표현.
---

AG-UI run은 event의 나열입니다. wire에서는 각각이 JSON 객체입니다. `type` field에
SCREAMING_SNAKE_CASE 이름이 들어갑니다. Rust에서는 각각이
[`Event`](/ag-ui-rust/api/ag_ui/event/enum.Event.html)의 variant입니다.
[`EventType`](/ag-ui-rust/api/ag_ui/event/enum.EventType.html)은 그
discriminator만 따로 뗀 것입니다.

AG-UI 1.0이 정의하는 event type은 **31개**입니다. Rust의 `EventType::ALL`에는
과거 stream을 읽기 위한 `THINKING_*` variant 5개도 있어 **36개**가 됩니다.
`cargo run -p xtask -- drift-check`는 현재의 31개를 검토된 1.0 schema baseline과
비교합니다. 자세한 것은 [검증 체계](/ag-ui-rust/ko/v1.0.0/design/verification/)를 보십시오.

두 enum 모두 일부러 exhaustive합니다. 그래서 protocol에 무언가 추가되면 match하는
자리에서 compile error가 납니다. `_` 갈래가 삼켜 버리지 않습니다.
[설계 원칙](/ag-ui-rust/ko/v1.0.0/design/commitments/)이 그 이유와 대가를 설명합니다.

## event 목록

variant마다 자기 이름을 딴 payload struct를 감쌉니다. `Event::TextMessageStart`는
`TextMessageStartEvent`를 싣습니다. 아래로 쭉 내려가며 전부 그렇습니다. payload의
field는 `type` 옆에 나란히 직렬화됩니다. 어떤 key 아래에 중첩되지 않습니다. 모든
payload는 `BaseEvent`의 optional field인 `timestamp`, `rawEvent`, `metadata`도 같은
객체 안에 평평하게 함께 싣습니다. `metadata`는 key로 열린 객체입니다. token 사용량,
trace id, application이 실어야 하는 무엇이든 담습니다. 없거나 객체이고, `null`은
아닙니다. consumer는 event마다의 metadata를 그 event가 만드는 message에 key별로
merge합니다. 마지막 쓰기가 이깁니다.
[`ag_ui::metadata`](/ag-ui-rust/api/ag_ui/metadata/index.html)에 규칙과 예약된 key
하나가 있습니다.

아래는 `EventType::ALL`의 순서입니다. 과거 `THINKING_*` variant 5개는 tool event 다음에 둡니다.

| wire 이름 | Rust variant | family | 의미 |
| --- | --- | --- | --- |
| `TEXT_MESSAGE_START` | `TextMessageStart` | Text | `messageId` 아래에 text message를 엽니다. `role`을 생략하면 `assistant`가 되며, 명시적인 JSON `null`은 거절합니다. |
| `TEXT_MESSAGE_CONTENT` | `TextMessageContent` | Text | 열린 message에 `delta`를 덧붙입니다. |
| `TEXT_MESSAGE_END` | `TextMessageEnd` | Text | message를 닫습니다. |
| `TEXT_MESSAGE_CHUNK` | `TextMessageChunk` | Text | text message를 열거나 이어 갑니다. |
| `TOOL_CALL_START` | `ToolCallStart` | Tool | call을 엽니다. tool 이름과, 뒤의 모든 것을 묶는 `toolCallId`를 답니다. |
| `TOOL_CALL_ARGS` | `ToolCallArgs` | Tool | 인자 JSON의 조각을 덧붙입니다. 조각은 이어 붙습니다. 하나만 떼면 대개 올바른 JSON이 아닙니다. |
| `TOOL_CALL_END` | `ToolCallEnd` | Tool | call을 닫습니다. 인자가 완성되었습니다. |
| `TOOL_CALL_CHUNK` | `ToolCallChunk` | Tool | tool call을 열거나 이어 갑니다. |
| `TOOL_CALL_RESULT` | `ToolCallResult` | Tool | 그 call의 result. thread에 덧붙는 `tool` message 형태입니다. |
| `THINKING_START` | `ThinkingStart` | Thinking (deprecated) | thinking block을 엽니다. 제목은 optional입니다. `REASONING_START`를 쓰십시오. |
| `THINKING_END` | `ThinkingEnd` | Thinking (deprecated) | thinking block을 닫습니다. `REASONING_END`를 쓰십시오. |
| `THINKING_TEXT_MESSAGE_START` | `ThinkingTextMessageStart` | Thinking (deprecated) | thinking message를 엽니다. `REASONING_MESSAGE_START`를 쓰십시오. |
| `THINKING_TEXT_MESSAGE_CONTENT` | `ThinkingTextMessageContent` | Thinking (deprecated) | thinking 텍스트를 덧붙입니다. message id를 싣지 않습니다. 그래서 block 하나가 동시에 가질 수 있는 message가 하나뿐이었고, 그것이 교체된 이유입니다. |
| `THINKING_TEXT_MESSAGE_END` | `ThinkingTextMessageEnd` | Thinking (deprecated) | thinking message를 닫습니다. `REASONING_MESSAGE_END`를 쓰십시오. |
| `STATE_SNAPSHOT` | `StateSnapshot` | State | shared state를 통째로 교체합니다. 자유 형식 JSON이고, protocol에는 불투명합니다. |
| `STATE_DELTA` | `StateDelta` | State | RFC 6902 연산으로 shared state를 patch합니다. 순서대로 적용됩니다. |
| `MESSAGES_SNAPSHOT` | `MessagesSnapshot` | State | message 이력을 교체합니다. 재연결 후, 또는 agent가 이력을 다시 쓸 때. |
| `ACTIVITY_SNAPSHOT` | `ActivitySnapshot` | Activity | client가 정의한 `activityType` 아래로 activity의 내용을 발행합니다. `replace`의 기본값은 `true`입니다. |
| `ACTIVITY_DELTA` | `ActivityDelta` | Activity | RFC 6902 연산으로 activity의 내용을 patch합니다. |
| `RAW` | `Raw` | Escape hatch | provider event를 그대로 전달합니다. `source`는 optional입니다. |
| `CUSTOM` | `Custom` | Escape hatch | 이름이 붙은, 애플리케이션이 정의한 event. protocol이 보증하는 것은 봉투뿐입니다. |
| `RUN_STARTED` | `RunStarted` | Lifecycle | 모든 run의 첫 event. `threadId`, `runId`, 그리고 optional로 부모 run과 그 run을 시작시킨 입력. |
| `RUN_FINISHED` | `RunFinished` | Lifecycle | `outcome`이 성공, interrupt, 취소를 구분합니다. |
| `RUN_ERROR` | `RunError` | Lifecycle | run이 실패했습니다. 새 run이 뒤따를 수 있습니다. |
| `STEP_STARTED` | `StepStarted` | Lifecycle | run 안에서 이름 붙은 step을 엽니다. |
| `STEP_FINISHED` | `StepFinished` | Lifecycle | 그 step을 닫습니다. |
| `REASONING_START` | `ReasoningStart` | Reasoning | 어느 message id에 대한 reasoning block을 엽니다. |
| `REASONING_MESSAGE_START` | `ReasoningMessageStart` | Reasoning | reasoning message를 엽니다. `TEXT_MESSAGE_START`와 달리 `role`이 필수이고, 언제나 `reasoning`입니다. |
| `REASONING_MESSAGE_CONTENT` | `ReasoningMessageContent` | Reasoning | reasoning 텍스트를 덧붙입니다. |
| `REASONING_MESSAGE_END` | `ReasoningMessageEnd` | Reasoning | reasoning message를 닫습니다. |
| `REASONING_MESSAGE_CHUNK` | `ReasoningMessageChunk` | Reasoning | reasoning message를 열거나 이어 갑니다. |
| `REASONING_END` | `ReasoningEnd` | Reasoning | reasoning block을 닫습니다. |
| `REASONING_ENCRYPTED_VALUE` | `ReasoningEncryptedValue` | Reasoning | provider의 불투명한 reasoning blob. zero-data-retention 모드를 위한 것입니다. `subtype`이 `entityId`가 `tool-call`을 가리키는지 `message`를 가리키는지 말합니다. |
| `SUBAGENT_STARTED` | `SubagentStarted` | Subagent | `subagentRunId` 아래로 subagent 호출을 announce합니다. 표시용 `name`을 답니다. optional로 `description`, 바깥 `parentSubagentRunId`, 그리고 이 호출을 낳은 `parentToolCallId` / `parentMessageId`. |
| `SUBAGENT_FINISHED` | `SubagentFinished` | Subagent | 호출을 닫습니다. `outcome`은 `success` 또는 `suspended`입니다. 후자는 subagent가 소유한 `interruptIds`를 댑니다. 없으면 success로 읽습니다. `result`는 `RUN_FINISHED.result`에 대응합니다. |
| `SUBAGENT_ERROR` | `SubagentError` | Subagent | 호출이 실패했습니다. 사람을 위한 `message`와 optional인 기계 판독용 `code`. |

현재 type 31개는 Text 4개, Tool 5개, State 3개, Activity 2개, Escape hatch
2개, Lifecycle 5개, Reasoning 7개, Subagent 3개입니다. 과거 Thinking variant 5개를
더하면 Rust에는 36개가 있습니다.

### attribution

현재 31개 type 중 **24개**가 자신을 만든 subagent를 가리키는 optional
`subagentRunId`를 싣습니다. text, tool, state, activity, reasoning, step family와
`RAW`, `CUSTOM`이 여기에 속합니다. 이 field가 없으면 부모 agent의 event입니다.
optional 귀속 정보를 싣지 않는 현재 type 7개는 `RUN_STARTED`, `RUN_FINISHED`,
`RUN_ERROR`, `MESSAGES_SNAPSHOT` 및 대상 subagent의 ID를 싣는 `SUBAGENT_*` 수명주기
event 3개입니다. 과거 `THINKING_*` variant 5개에도 이 field가 없습니다.
`EventType::is_attributable`은 type별로 이를 알려 주고,
`Event::subagent_run_id`는 event의 tag를 읽습니다. 자세한 것은
[subagent](/ag-ui-rust/ko/v1.0.0/server/subagents/)를 보십시오.

## wire에서

`type`이 tag이고, payload는 그 옆에 평평하게 놓입니다.

```rust
use ag_ui::{Event, EventType};

fn main() {
    // 현재 protocol event와 수신 호환용 과거 variant 5개.
    assert_eq!(EventType::ALL.len(), 36);

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

이 build가 모르는 event type은 deserialize에 실패합니다. 의도된 것입니다. 더 새로운
agent와 이야기하는 frontend는 모르는 type의 이름을 대며 error로 멈춥니다. 대화의
4분의 3만 조용히 그리지 않습니다.

## `THINKING_*` family는 deprecated입니다

AG-UI 1.0은 이 다섯 type을 현재 event 집합에서 제외합니다. client는 과거 stream을
위해 계속 parse하지만, 현재 producer는 이들을 거절하고 `REASONING_*`를 내보냅니다.
`THINKING_TEXT_MESSAGE_CONTENT`에는 message id가 없어 thinking block 하나에서
동시에 다룰 수 있는 message가 하나뿐이었습니다.

Rust variant와 payload struct에는 `#[deprecated]`가 붙습니다. `ag-ui` 자신의
event module은 `#![allow(deprecated)]`를 답니다. 이 module은 union에서도,
`event_type()`에서도, factory에서도 이 type들의 이름을 대야 합니다. 과거 입력을 지원한다고 자기 자신에게 경고하는 것은 도움이 되지 않습니다. 이 억제는
그 module 안에서만 유효합니다. 그래서 이들 중 하나를 쓰는 consumer는 자기 사용
지점에서 경고를 받습니다. 계속 쓸지 정하는 자리가 거기입니다.

`Event::is_deprecated`는 match 없이 runtime에 답합니다.

```rust
use ag_ui::Event;

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

`TEXT_MESSAGE_CHUNK`, `TOOL_CALL_CHUNK`, `REASONING_MESSAGE_CHUNK`는
대상을 열거나 이어 갑니다. 첫 chunk에 ID를 넣고, 시작할 때 보낸 metadata를
반복할 때는 같은 값을 사용합니다. stream이 열려 있는 동안은 이후 chunk의 ID를
생략할 수 있습니다. 부모 agent와 각 subagent는 별도의 lane을 사용합니다.

같은 lane에서 새 대상을 열면 이전 chunk stream이 닫힙니다. run 경계와
`MESSAGES_SNAPSHOT`은 모든 lane을 닫습니다. subagent의 종료 event는 해당
lane을 닫습니다. 아래 예제에서는 세 번째 chunk가 첫 메시지를 닫습니다.

```text
TEXT_MESSAGE_CHUNK { messageId: "msg-1", delta: "Hel" }
TEXT_MESSAGE_CHUNK { delta: "lo" }
TEXT_MESSAGE_CHUNK { messageId: "msg-2", delta: "Bye" }   <- msg-1이 방금 끝났습니다
```

소비하는 쪽에서 그 장부 정리는 `ag_ui::client::chunks`가 맡습니다. 연달아 이어진
chunk를 다른 무엇이 보기 전에 start/content/end 세 짝으로 되펼칩니다. emit하는
쪽에는 일부러 **handle이 없습니다**. `ag_ui::server`의 typestate emitter는 연 것이
닫히도록 보장하려고 있습니다. chunk에는 닫을 것이 없습니다. RAII handle로 감싸면
틀릴 방법만 하나 늘어납니다. 이들은 `ctx.emit`으로 emit하십시오. API를 기다리는
빈틈이 아니라 지원되는 경로입니다.

뒤섞인 병렬 tool call이 `ctx.emit`에 속하는 나머지 사례입니다. `ToolCallHandle` 두
개를 동시에 여는 것은 *설계상* borrow check error입니다. 그래서
`args(a) args(b) args(a) end(a) end(b)`를 흘리는 provider를 call당 handle 하나로
그대로 옮길 수 없습니다. 방법은 둘입니다. call마다 인자를 모아 두었다가 완성되면
통째로 emit하십시오. 두 call의 인자가 서로 섞여 들어갈 수 없는 유일한 매핑입니다.
아니면 뒤섞인 그대로 직접 emit하십시오. ordering verifier는 모든 것을 id로
색인하므로 뒤섞인 stream을 받아들입니다. 허락하지 않는 것은 열지 않은 call을 닫는
일입니다. [검증 체계](/ag-ui-rust/ko/v1.0.0/design/verification/)를 보십시오.

## protobuf 지원 범위

upstream은 현재 AG-UI 1.0 JSON Schema에서 `events.proto`를 생성합니다.
현재 event type 31개를 모두 표현하며, 퇴역한 `THINKING_*` event 5개는
제외합니다. 이 SDK의 `protobuf` feature는 media type과 formatter stub을
제공합니다. encoder는 `Error::UnsupportedTransport`를 반환하고 HTTP 협상은
protobuf를 선택하지 않습니다. 스트리밍에는 SSE를 사용하세요.
자세한 내용은 [protobuf rustdoc](/ag-ui-rust/api/ag_ui/encode/protobuf/index.html)과
[이슈 #46](https://github.com/KimSoungRyoul/ag-ui-rust/issues/46)을 참고하세요.
