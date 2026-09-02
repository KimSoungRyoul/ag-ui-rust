---
title: AG-UI 동작 방식
description: AG-UI 교환의 모양. run input을 담은 POST 하나에 type이 붙은 event stream이 답합니다.
---

AG-UI는 작습니다. request 하나가 run을 시작합니다. 답은 type이 붙은 event stream입니다.
run이 끝날 때까지 agent가 하는 모든 일을 서술합니다. 두 번째 endpoint도 없고, polling
channel도 없고, 협상 단계도 없습니다.

이 page는 wire 이야기이지 이 SDK의 API 이야기가 아닙니다. 여기 나오는 type은
`ag-ui`에 삽니다. 이 crate는 일부러 어휘일 뿐입니다. runtime도 I/O도 async도
없습니다.

## request 하나, run 하나

request body는 `RunAgentInput`입니다. agent가 대화에 대해 알아도 되는 모든 것이 여기
담겨 옵니다. agent가 무언가를 기억한다고 가정하지 않기 때문입니다:

| field | 무엇을 담는가 |
| --- | --- |
| `threadId` | 이 run이 속한 대화. |
| `runId` | 이 run의 id. 모든 lifecycle event에 되울립니다. |
| `parentRunId` | 이 run을 낳은 run. 중첩되거나 위임된 agent를 위한 것. |
| `state` | 공유 application state. 자유 형식 JSON이고 protocol에는 불투명합니다. |
| `messages` | 대화 이력. 오래된 것부터. |
| `tools` | *client*가 이 run에 제안하는 tool. |
| `context` | 주변 context 항목. |
| `forwardedProps` | 임의의 통과 data. 이것도 protocol에는 불투명합니다. |
| `resume` | 이전 run이 멈춰 서서 물은 것에 대한 답. 재개할 때만 있습니다. |

```rust
use ag_ui::RunAgentInput;

let body = r#"{
    "threadId": "thread-1",
    "runId": "run-1",
    "state": { "tasks": [] },
    "messages": [{ "id": "m1", "role": "user", "content": "add a task" }],
    "tools": [],
    "context": []
}"#;

let input: RunAgentInput = serde_json::from_str(body).unwrap();

assert_eq!(input.thread_id.as_str(), "thread-1");
assert_eq!(input.messages.len(), 1);
assert_eq!(input.state["tasks"], serde_json::json!([]));
assert!(!input.is_resume());
```

사람들이 놀라는 지점 둘을 분명히 해 둡니다.

**thread는 client에 삽니다.** `threadId`는 대화의 이름입니다. 대화를 가져오지 않습니다.
server가 thread를 저장한다는 말은 protocol 어디에도 없습니다. 이력을 하나도 갖지 않은
agent도 규격을 지킵니다. 대화를 남길지는 application이 정합니다.

**tool 목록은 client의 제안이지 agent의 메뉴가 아닙니다.** AG-UI에는 tool discovery가
없습니다. agent는 받지 않은 tool을 달라고 할 수 없습니다. 그렇다고 allow-list도
아닙니다. `tools`에 없는 이름으로 call을 emit해도 잘 형성된 stream입니다. agent가 스스로
한 일을 보고하는 방법이 그것입니다. `docs/DESIGN.md`가 그 논거를 길게 폅니다. agent를
쓰는 입장에서 무슨 뜻인지는 [tool call](/ag-ui-rust/ko/server/tools/)이 다룹니다.

## 답은 event stream입니다

응답은 `text/event-stream`입니다. SSE `data:` frame 하나에 JSON object 하나가 실립니다.
agent가 만든 순서 그대로입니다. 각 object는 SCREAMING_SNAKE_CASE 이름을 담은 `type`
판별자를 갖습니다. payload의 field는 어떤 key 아래 중첩되지 않고 그 옆에 나란히
놓입니다:

```rust
use ag_ui::{Event, EventStreamFormatter, SseFormatter, TextMessageRole};

let formatter = SseFormatter::new();
let run = [
    Event::run_started("thread-1", "run-1"),
    Event::text_message_start("msg-1", TextMessageRole::Assistant),
    Event::text_message_content("msg-1", "It is "),
    Event::text_message_content("msg-1", "sunny."),
    Event::text_message_end("msg-1"),
    Event::run_finished_success("thread-1", "run-1"),
];

let body: String = run
    .iter()
    .map(|event| formatter.encode_to_string(event).unwrap())
    .collect();

assert_eq!(
    body.lines().next(),
    Some(r#"data: {"type":"RUN_STARTED","threadId":"thread-1","runId":"run-1"}"#),
);
// frame 하나는 `data:` 줄 하나와 빈 줄 하나입니다. 직렬화한 JSON에는 날
// 줄바꿈이 없습니다. event가 frame에 걸쳐 쪼개지지 않습니다.
assert_eq!(body.matches("\n\n").count(), 6);
```

SSE는 상호운용의 기본값입니다. 이 SDK가 온전히 구현하는 유일한 transport이기도 합니다.
protocol은 binary media type `application/vnd.ag-ui.event+proto`도 정의하고
`ag-ui`가 그것을 협상합니다. 다만 upstream의 `events.proto`는 36개 event type 중
21개만 다룹니다. 그쪽으로 encode하면 event를 조용히 떨어뜨립니다. 그래서 여기에
encoder는 없습니다. 자세한 것은 [feature flag](/ag-ui-rust/ko/reference/features/)에
있습니다.

content negotiation은 `Accept`로 합니다. header가 없거나 비면 `*/*`로 읽고 SSE로
답합니다. `406`을 받아야 할 경우는 그 build가 내보낼 수 있는 모든 것을 배제하는
header입니다.

## run lifecycle

모든 run은 `RUN_STARTED`로 열립니다. `RUN_FINISHED`와 `RUN_ERROR` 중 하나로만
닫힙니다. 그 뒤로는 아무것도 오지 않습니다.

```text
RUN_STARTED
  …everything the agent did…
RUN_FINISHED   or   RUN_ERROR
```

`RUN_FINISHED`는 `outcome`을 싣습니다. "끝났다"에는 *멈춰 섰다*도 들어갑니다:

- `{"type":"success"}` — run이 완료되었습니다.
- `{"type":"interrupt","interrupts":[…]}` — agent가 사람을 기다립니다. client가 답을
  모아 *다음* request의 `resume`에 실어 보냅니다. run은 거기서 이어집니다.
  [human in the loop](/ag-ui-rust/ko/server/interrupts/)를 보세요.

이 field는 선택입니다. interrupt protocol보다 앞선 producer는 생략합니다. 소비자는
그것을 성공으로 읽어야 합니다.

`RUN_ERROR`는 message와, 선택인 기계 판독용 `code`를 싣습니다. 잘 형성된 `200` 응답
안에 담겨 옵니다. agent가 실패할 수 있는 시점이면 status line은 이미 나갔기
때문입니다. client가 agent 실패와 죽은 socket을 구별하는 근거가 바로 그것입니다.

run 안에서 `STEP_STARTED` / `STEP_FINISHED`는 이름 붙은 단계를 감쌉니다. 선택이고
순전히 서술적입니다. 다른 무엇도 여기 기대지 않습니다.

## delta, 그리고 그것을 감싸는 세 짝

agent가 만드는 것은 거의 다 조각으로 옵니다. 그래서 stream은 대부분 *delta*입니다.
논리적으로 하나인 것은 `START`와 여러 개의 내용 event와 `END`입니다. message, tool
call, reasoning block이 그렇습니다. 셋 다 같은 id를 답니다:

```text
TEXT_MESSAGE_START     messageId=msg-1  role=assistant
TEXT_MESSAGE_CONTENT   messageId=msg-1  delta="It is "
TEXT_MESSAGE_CONTENT   messageId=msg-1  delta="sunny."
TEXT_MESSAGE_END       messageId=msg-1
```

그 셋을 하나로 묶는 것이 id입니다. *뒤섞임*을 읽을 수 있게 하는 것도 id입니다. tool
둘을 한 번에 요청하는 model은 열린 call 둘을 만듭니다. 두 call의 event가 번갈아
나옵니다. 어느 fragment가 어느 call의 것인지는 id만이 말합니다.

```text
TOOL_CALL_START   toolCallId=call-1  toolCallName=add_task
TOOL_CALL_START   toolCallId=call-2  toolCallName=add_task
TOOL_CALL_ARGS    toolCallId=call-1  delta="{\"title\":"
TOOL_CALL_ARGS    toolCallId=call-2  delta="{\"title\":"
TOOL_CALL_ARGS    toolCallId=call-1  delta="\"write it down\"}"
TOOL_CALL_END     toolCallId=call-1
TOOL_CALL_RESULT  toolCallId=call-1  content="{\"id\":1}"
```

renderer를 물어뜯는 세부가 둘 있습니다. 둘 다
[board-watch 예제](/ag-ui-rust/ko/examples/board-watch/)가 보여 줍니다. 인자 fragment는
임의의 byte 위치에서 잘린 JSON입니다. `\`와 그것이 escape하는 `n`이 서로 다른 event로
올 수 있습니다. fragment 하나만으로는 parse되지 않습니다. text fragment는 하나하나가
유효한 UTF-8입니다. Rust `String`이 그럴 수밖에 없습니다. 그래도 *grapheme*은 fragment에
걸쳐 쪼개집니다. zero-width joiner로 만든 emoji는 여러 조각으로 옵니다.

### chunk event

자기 출력을 감싸지 못하는 producer가 있습니다. provider adapter는 다음 message가
시작되기 전까지 이전 message가 끝난 줄 모르는 일이 흔합니다. 그래서 protocol은
`TEXT_MESSAGE_CHUNK`, `TOOL_CALL_CHUNK`, `REASONING_MESSAGE_CHUNK`도 정의합니다.
chunk는 연속된 것 중 **첫 번째에만** id를 싣습니다. 뒤의 것은 그 id를 물려받습니다.
chunk event 다섯 개가 message 하나일 수 있습니다.

소비자는 다른 무엇이 stream을 보기 전에 chunk를 명시적인 start/content/end 세 짝으로
되돌립니다. `ag_ui::client`는 그것을 `chunks` 단계에서 합니다. 그 결과 view가 무엇을
보는지는 [update stream](/ag-ui-rust/ko/client/updates/)이 보여 줍니다.

## event 계열

event type은 **36개**입니다. `ag-ui`는 그것을 빠짐없는 `Event` enum 하나와
`EventType` 판별자로 표현합니다:

```rust
use ag_ui::{Event, EventType};

let event = Event::text_message_content("msg-1", "Hello");

assert_eq!(event.event_type(), EventType::TextMessageContent);
assert_eq!(EventType::TextMessageContent.as_str(), "TEXT_MESSAGE_CONTENT");
assert_eq!(EventType::ALL.len(), 36);
```

여덟 계열로 묶입니다:

| 계열 | event | 무엇을 위한 것인가 |
| --- | --- | --- |
| text message | `TEXT_MESSAGE_START` / `_CONTENT` / `_END` / `_CHUNK` | 사용자가 읽는 답변. |
| tool call | `TOOL_CALL_START` / `_ARGS` / `_END` / `_CHUNK` / `_RESULT` | call, 그 인자 JSON, 그리고 결과. |
| reasoning | `REASONING_START` / `_END`, `REASONING_MESSAGE_START` / `_CONTENT` / `_END` / `_CHUNK`, `REASONING_ENCRYPTED_VALUE` | 답변과 떼어 둔 사고. block 하나가 message 하나 이상을 감쌉니다. |
| thinking | `THINKING_START` / `_END`, `THINKING_TEXT_MESSAGE_START` / `_CONTENT` / `_END` | reasoning 계열의 폐기 예정 선행자. 아직 wire에 있고, 아직 type으로 남아 있습니다. |
| state | `STATE_SNAPSHOT`, `STATE_DELTA`, `MESSAGES_SNAPSHOT` | shared state, 그리고 이력의 통째 교체. |
| activity | `ACTIVITY_SNAPSHOT`, `ACTIVITY_DELTA` | agent가 지금 *하는 일*. 검색, 읽기, 대기. client가 그릴 수 있는 모양으로. |
| run과 step | `RUN_STARTED`, `RUN_FINISHED`, `RUN_ERROR`, `STEP_STARTED`, `STEP_FINISHED` | 위의 lifecycle. |
| 탈출구 | `RAW`, `CUSTOM` | provider event를 그대로 전달한 것, 그리고 application이 정의한 것. |

field 단위 판본은 [event reference](/ag-ui-rust/ko/reference/events/)에 있습니다.

`Event`가 `#[non_exhaustive]`가 아니라 빠짐없는 enum인 것은 값을 치르는 결정입니다. 새
protocol event가 생기면 event를 match하는 모든 사람에게 compile 오류가 납니다. 이 SDK의
major version도 올라갑니다. 그 이유는
[설계 원칙](/ag-ui-rust/ko/design/commitments/)에 있습니다. 빠뜨린 부분은 시끄러워야
하는데, `_` arm이 바로 그것을 조용하게 만듭니다.

## state는 snapshot이나 patch로 움직입니다

application state는 양쪽이 함께 비추는 자유 형식 JSON입니다. agent는 그것을 두 가지로
다시 알립니다:

- `STATE_SNAPSHOT`은 값 전체를 싣고, client가 든 것을 대체합니다.
- `STATE_DELTA`는 RFC 6902 JSON Patch를 싣고, 그것에 적용됩니다.

어느 쪽을 보낼지는 protocol 규칙이 아니라 크기 판단입니다. client는 둘 다 처리해야
합니다. `ag_ui::server`는 publish할 때마다 정합니다. 첫 번째는 언제나 snapshot입니다.
뒤의 것은 patch가 그것이 서술하는 state보다 작아지지 않는 한 delta입니다. state가
작으면 그런 일이 자주 벌어집니다. [shared state](/ag-ui-rust/ko/server/state/)가 그것을
짚어 나갑니다.

`STATE_*` event는 다른 모든 것에 대해 **순서가 없습니다**. message나 tool call이 열려
있는 동안에도 옵니다. 위반이 아닙니다. agent가 call을 끝낸 뒤에야 보고하는 대신 call이
자리 잡는 모습을 보여 주는 방법입니다. 뒤집어 말하면 state event는 도착 당시 무엇이
열려 있었는지와 아무 연관이 없습니다. wire도 그 연관을 싣지 않기 때문입니다.

## ordering 규칙은 실제로 무엇인가

protocol의 규칙은 감싸는 짝과 id에 관한 것입니다. 몇 개 안 됩니다:

| 규칙 | 무엇을 금지하는가 |
| --- | --- |
| `run-ended` | `RUN_FINISHED`나 `RUN_ERROR` 뒤의 모든 event. |
| `duplicate-run-started` | 두 번째 `RUN_STARTED`. |
| `duplicate-start` | 같은 id를 두 번 여는 것. |
| `not-open` | 짝이 되는 시작 없이 오는 내용이나 종결 event. |
| `unknown-id` | stream이 소개한 적 없는 id를 참조하는 것. |
| `open-at-finish` | message나 call이나 step이 열려 있는데 오는 `RUN_FINISHED`. |
| `out-of-order` | 적법한 event가 부적법한 자리에 오는 것. `TOOL_CALL_END`보다 앞선 tool 결과. |

저 이름이 이 SDK가 보고하는 이름입니다. client뿐 아니라 **server**에서도 검사하고,
release build에서도 기본으로 켜져 있습니다. 그래서 `START` 없이
`TEXT_MESSAGE_CONTENT`를 emit하면 network 세 hop 아래의 frontend가 혼란에 빠지는
대신, bug가 있는 자리에서 진단이 나옵니다.
[verification](/ag-ui-rust/ko/design/verification/)이 그 state machine과 그 비용을
다룹니다.

저 목록에 *없는* 것도 보세요. agent가 어떤 tool을 불러도 되는지에 대한 것이 없습니다.
state event가 어디에 나와도 되는지에 대한 것도 없습니다. run이 message를 몇 개 만들어야
하는지도 없습니다.

## type은 어디에 있나

이식은 upstream TypeScript Zod schema를 보고 손으로 했습니다. 손실이 있는 부분집합인
protobuf 정의를 보고 한 것이 아닙니다. compiler 안에서 그 둘을 잇는 것은 없습니다.
`cargo run -p xtask -- drift-check`가 그 일을 합니다. upstream event 표면을 vendor해 둔
snapshot을 Rust type과 대조하고, 둘이 갈라지면 build를 실패시킵니다.

- [event reference](/ag-ui-rust/ko/reference/events/) — 모든 event와 그 field.
- [crate 구성](/ag-ui-rust/ko/start/crates/) — 이 type이 어디에 살고 그 위에 무엇이
  쌓이는지.
- [ag_ui](/ag-ui-rust/api/ag_ui/index.html) — rustdoc.
