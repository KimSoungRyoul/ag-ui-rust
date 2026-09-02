---
title: update stream
description: run이 진행되며 내어놓는 값들, run이 끝나는 세 가지 방법, 그리고 두 enum 중 하나는 왜 모든 경우를 나열하고 다른 하나는 왜 그러지 않는지.
---

run은 `Update`의 stream입니다. 하나하나가 view가 반응해야 할
무언가입니다. "이 message가 늘어났다", "state가 바뀌었고 type이 붙은
채로 여기 있다", "agent가 당신을 기다린다" 같은 것들입니다. chunk
normalization, protocol verification, delta 적용은 이미 끝난 뒤입니다.
`Update` 하나가 다시 그리기 한 번입니다.

`Update<S>`는 state type에 대해 generic입니다. `Session`의 두 번째
parameter가 나타나는 자리가 여기입니다. session에 더 나은 것을
요구하지 않았다면 `serde_json::Value`입니다.

## 모든 variant

```rust
// src/render.rs
use ag_ui::client::{RunEnd, Update};
use serde_json::Value;

fn render(update: Update<Value>) {
    match update {
        // message가 만들어졌거나, 이어 붙었거나, 끝났습니다. `index`는
        // `Session::messages` 안의 위치입니다. `message`는 지금 시점의
        // message 전체입니다. view는 한 행만 다시 그려도 되고, 전부
        // 다시 읽어도 됩니다.
        Update::Message(message) => {
            println!("message {} at {}: {:?}", message.id, message.index, message.change);
        }

        // MESSAGES_SNAPSHOT이 대화를 통째로 갈아 치웠습니다. message가
        // 사라졌을 수도 있으니 전부 다시 그리세요.
        Update::Messages(messages) => println!("{} messages replaced", messages.len()),

        // 호출자의 type으로 담은 application state. snapshot과 patch는
        // 같은 방식으로 도착하고, 여기서는 둘을 구분할 수 없습니다.
        Update::State(state) => println!("state is now {state}"),

        // reasoning text. 답변과는 분리해 둡니다.
        Update::Reasoning(reasoning) => println!("thinking: {}", reasoning.text),

        // run이 멈췄고 사람이 필요합니다. 대기 중인 interrupt 하나에
        // update 하나입니다.
        Update::Interrupt(interrupt) => println!("waiting on {}", interrupt.id),

        // 어긋난 stream, 적용되지 않는 patch, transport 실패,
        // RUN_ERROR. 반드시 치명적이지는 않습니다.
        Update::Error(error) => eprintln!("{error}"),

        // 빠져나가는 모든 경로에서, 언제나 run의 마지막 update.
        Update::Done(end) => println!("{}", ended(&end)),

        // `Update`는 `#[non_exhaustive]`입니다. wire type이 아니라 view
        // model이기 때문입니다. 다시 그릴 값어치가 있는 무언가가 새로
        // 생기는 것은 protocol 변경이 아닙니다.
        _ => {}
    }
}

/// run이 어떻게 끝났는지를 한 마디로. 갈래 셋에 `_`는 없습니다. 아래를 보세요.
fn ended(end: &RunEnd) -> String {
    match end {
        RunEnd::Success { .. } => "success".to_owned(),
        RunEnd::Interrupted { interrupts } => format!("interrupted on {}", interrupts.len()),
        RunEnd::Failed { message, .. } => format!("failed: {message}"),
    }
}

fn main() {
    render(Update::State(Value::Bool(true)));
    assert_eq!(ended(&RunEnd::Success { result: None }), "success");
}
```

`MessageUpdate`는 `index`, `id`, `change`와 조립된 `message`를 실어
나릅니다. `change`는 `MessageChangeKind`입니다. renderer가 대부분의
시간을 쓰는 곳이 여기입니다. `Started`, `Content { delta }`, `Ended`,
`ToolCallStarted`, `ToolCallArgs`, `ToolCallEnded`, `ToolResult`,
`Activity`, `EncryptedValue`가 있습니다. 이들을 어떻게 다룰지는 [run
rendering](/ag-ui-rust/ko/client/rendering/)에서 다룹니다.

:::note
`Update`는 entity 단위가 아니라 *event* 단위입니다. text delta 마흔
개는 같은 id 아래의 `Update::Message` 마흔 개입니다. 동시에 진행 중인
tool call 둘은 서로의 event를 사이사이 끼워 넣습니다. 그래서 연달아 오는
update가 같은 call에 속한다는 보장이 없습니다. 중첩을 알려 주는 신호는
도착 순서뿐입니다. 그것을 포기하면 renderer는 무엇을 치를까요. [run
rendering](/ag-ui-rust/ko/client/rendering/) 한 페이지가 그 이야기입니다.
:::

## run이 끝나는 세 가지 방법

모든 run은 정확히 하나의 `Update::Done`으로 끝납니다. stream도 거기서
끝납니다. 빠져나가는 모든 경로가 그렇습니다. agent가 끝냈든, agent가
실패했든, transport가 말 도중에 죽었든 마찬가지입니다.

```rust
// src/render.rs
use ag_ui::client::RunEnd;

/// 입력창을 다시 살릴지 여부. `RunEnd`가 존재하는 이유인 그 결정.
fn prompt_again(end: &RunEnd) -> bool {
    match end {
        // agent가 끝냈습니다.
        RunEnd::Success { .. } => true,
        // agent가 기다리고 있습니다. 타이핑 대신 interrupt에 답하세요.
        RunEnd::Interrupted { .. } => false,
        // run이 실패했거나, 끝나기 전에 transport가 멈췄습니다.
        RunEnd::Failed { .. } => true,
    }
}

fn main() {
    assert!(prompt_again(&RunEnd::Success { result: None }));
    assert!(!prompt_again(&RunEnd::Interrupted { interrupts: Vec::new() }));
}
```

| variant | field |
| --- | --- |
| `Success` | `result: Option<Value>` — agent가 반환값을 보냈다면 그 값. |
| `Interrupted` | `interrupts: Vec<Interrupt>` — `Update::Interrupt`로 하나씩 도착했던 그 interrupt들. 다음 run 전까지 `Session::interrupts`에 남아 있습니다. |
| `Failed` | `message: String`, `code: Option<String>` — 무엇이 잘못됐는지. 그리고 agent가 보냈다면 기계가 읽을 수 있는 code. |

갈래는 셋이고 `_`는 없습니다. `RunEnd`가 **exhaustive**하기
때문입니다. 의도한 것입니다. 이 workspace의 모든 error type이 하는 것과
정반대입니다. run이 세 가지로 끝나는 것은 protocol이 그렇게 정했기
때문입니다. 성공 outcome을 담은 `RUN_FINISHED`, interrupt outcome을 담은
`RUN_FINISHED`, 그리고 `RUN_ERROR`입니다. 잘려 나간 stream도 `RUN_ERROR`로
알립니다.

네 번째가 생긴다면 wire 계약이 바뀐 것입니다. 그리고 이 match야말로
frontend가 compiler의 도움을 가장 받고 싶어 하는 match입니다. 그
갈래들이 입력창을 다시 열지, 답해야 할 것이 남았는지, 무언가 실패했는지를
결정하기 때문입니다. 거기에 `_` 갈래를 두면 "run이 끝나는 새로운 방법"이
아무 진단도 없는 일이 됩니다.

`Update`가 `#[non_exhaustive]`를 유지하는 것은 거울에 비친 이유
때문입니다. wire type이 아니라 view model이니까요. 이 논증의 일반형은
[설계 원칙](/ag-ui-rust/ko/design/commitments/)에 있습니다. protocol의
`Event` enum이 그 대표 사례입니다.

## `Success`는 agent가 그렇게 말했다는 뜻입니다

아무 문제도 없었다는 뜻은 아닙니다. 둘은 갈라집니다. 그 틈은 전부
client *자신의* 진단으로 채워져 있습니다. verifier가 잡아낸 protocol
위반이나, 적용되지 않는 state patch가 그렇습니다. 이들은
`Update::Error`로 도착합니다. run은 그대로 이어져 성공으로 끝납니다.
agent는 이를 듣지도, 묻지도 않기 때문입니다.

```rust
// src/main.rs
use ag_ui::client::{RunEnd, Session, Update, transport::ReplayTransport};
use ag_ui::{Event, PatchOperation};
use futures_util::StreamExt;
use serde_json::json;

#[tokio::main]
async fn main() {
    let transport = ReplayTransport::new([
        Event::run_started("thread-1", "run-1"),
        Event::state_snapshot(json!({ "count": 1 })),
        // 존재하지 않는 경로를 replace 합니다. RFC 6902 patch는 전부
        // 아니면 전무입니다. state는 있던 그대로 남습니다.
        Event::state_delta(vec![PatchOperation::replace("/missing/deeply", json!(2))]),
        Event::state_delta(vec![PatchOperation::replace("/count", json!(2))]),
        Event::run_finished_success("thread-1", "run-1"),
    ]);

    let mut session = Session::<_>::new(transport, "thread-1");
    let updates: Vec<_> = session.send("go").collect().await;

    let errors: Vec<String> = updates
        .iter()
        .filter_map(|update| match update {
            Update::Error(error) => Some(error.to_string()),
            _ => None,
        })
        .collect();

    assert_eq!(errors.len(), 1);
    assert!(errors[0].contains("state patch failed"));

    // run은 계속됐습니다. 뒤의 delta는 적용됐고 agent는 성공이라고
    // 했습니다. `Done`만 보고 분기하는 view는 이 run을 깨끗했다고 봅니다.
    assert!(matches!(updates.last(), Some(Update::Done(RunEnd::Success { .. }))));
    assert_eq!(session.raw_state(), &json!({ "count": 2 }));
}
```

그러므로 `Update::Error`는 종료 신호가 아닙니다. 정말로 치명적일 때는
짝이 되는 `Update::Done`이 뒤따릅니다. `RunEnd::Failed` 앞에는 언제나
그에 해당하는 `Update::Error`가 있습니다. transport가 그냥 멈춘
경로까지 포함해 모든 경로에서 그렇습니다.

이 차이가 중요하다면 error를 도착하는 대로 기록해 두세요.
`board-watch`는 error가 올 때마다 출력합니다. 그래서 그 기록에는
불평도 하고 성공도 한 run이 그대로 남습니다.

## 이 build가 모르는 event

protocol의 `Event` enum도 exhaustive합니다. 그 exhaustive함이
바로잡으려는 실패는 소리 없는 누락입니다. 커뮤니티판 `ag-ui
0.1.0`은 event variant를 24개만 선언했습니다. 당시 명세에는 32개가
있었습니다. 지금은 36개입니다. 아무도 알아채지 못했습니다. 모든 소비자
code에 있던 `_` 갈래가 그 침묵의 모습이기 때문입니다.

type 수준의 결과는 event 하나를 추가하는 일이 이 SDK의 major
version이 된다는 것입니다. runtime 수준의 결과는 여기 있습니다. wire에
알아볼 수 없는 `type`이 오면 deserialize가 실패하고 run이 멈춥니다. 아래
transport는 그것을 보일 수 있는 가장 작은 transport입니다. 그 모양은
[transport](/ag-ui-rust/ko/client/transports/)에서 설명합니다.

```rust
// src/main.rs
use ag_ui::client::transport::{Transport, TransportFuture, boxed_stream, decode_events};
use ag_ui::client::{RunEnd, Session, Update};
use ag_ui::encode::sse::frame;
use ag_ui::{Event, RunAgentInput, SseFormatter, TextMessageRole};
use futures_util::StreamExt;

/// 모든 run에 같은 녹화 response body로 답하는 transport.
struct Recorded(String);

impl Transport for Recorded {
    fn run(&self, _input: RunAgentInput) -> TransportFuture {
        let body = self.0.clone();
        Box::pin(async move {
            let chunks = futures_util::stream::iter([Ok::<_, std::io::Error>(body)]);
            Ok(boxed_stream(decode_events(chunks)))
        })
    }
}

#[tokio::main]
async fn main() {
    let sse = SseFormatter::new();
    let mut body = String::new();
    for event in [
        Event::run_started("thread-1", "run-1"),
        Event::text_message_start("msg-1", TextMessageRole::Assistant),
        Event::text_message_content("msg-1", "Half a conversation."),
        Event::text_message_end("msg-1"),
    ] {
        body.push_str(&sse.encode_to_string(&event).expect("encodes"));
    }
    // 더 새로운 agent가 보낸 event입니다. 이 build에는 그것을 encode할
    // variant가 없어서 frame을 손으로 만듭니다.
    body.push_str(&frame(r#"{"type":"TELEPATHY_START","messageId":"msg-2"}"#));

    let mut session = Session::<_>::new(Recorded(body), "thread-1");
    let updates: Vec<_> = session.send("go").collect().await;

    let Some(Update::Done(RunEnd::Failed { message, .. })) = updates.last() else {
        panic!("an unknown event must still end the run: {updates:?}");
    };
    // error는 알아보지 못한 type을 이름으로 알려 줍니다.
    assert!(message.contains("TELEPATHY_START"), "unhelpful: {message}");

    // 그 앞에 도착한 것은 그대로 남습니다. 실패는 시끄러울 뿐,
    // 잃어버리지 않습니다.
    assert_eq!(session.messages().len(), 2);
}
```

더 새로운 agent와 이야기하는 frontend는 대화의 4분의 3을 조용히 그리지
않습니다. 알아보지 못한 type을 이름으로 알려 주는 error와 함께
멈춥니다. 말 도중에 죽는 transport도 마찬가지입니다. 잘려 나갔다는
사실이 `Update::Error`로 보고되고 run은 `Failed`로 끝납니다. `Done`을
보고 입력창을 다시 여는 view가 끊어진 연결 때문에 영영 기다려서는 안
되기 때문입니다. verification을 끄면 잘림을 얼마나 정확하게 설명하는지가
달라집니다. 보고할지 말지는 달라지지 않습니다.

## error

`Update::Error`는 `ag_ui::client::Error`를 실어 나릅니다. 이 type은
`#[non_exhaustive]`입니다. 새 transport와 새 validation rule이 호환성을 깨는
release 없이 variant를 추가하리라 보기 때문입니다. 분기해 볼 값어치가
있는 variant는 다음과 같습니다.

| variant | 무슨 일이 있었는가 |
| --- | --- |
| `Protocol` | stream은 parse됐지만 ordering rule을 어겼습니다. 문제의 event는 적용되지 않았습니다. |
| `Patch` | RFC 6902 patch를 적용할 수 없었습니다. 대상 문서는 그대로입니다. |
| `State` | state가 `S`로 deserialize되지 않았습니다. `raw_state`는 여전히 정확합니다. |
| `Run` | agent가 `RUN_ERROR`를 보냈습니다. |
| `Json` / `Decode` | byte가 올바른 event가 아니었거나, 형식에 맞는 `text/event-stream`이 아니었습니다. |
| `Http` / `Transport` / `Config` | 요청이 stream이 되지 못했거나, stream이기를 그만뒀습니다. |

나머지는 그냥 흘려보내면 됩니다. 그것이 이 attribute의 목적입니다.
실패 양상을 남김없이 나열한 match를 원하는 사람은 없습니다. 새로운 실패
양상은 protocol 변경이 아닙니다.

## 다음

- [run rendering](/ag-ui-rust/ko/client/rendering/) — tool call 둘이 동시에
  열려 있을 때 `Update::Message`를 어떻게 다룰지.
- [transport](/ag-ui-rust/ko/client/transports/) — event가 어디서
  왔는지.
- API 문서의
  [`Update`](/ag-ui-rust/api/ag_ui/client/session/enum.Update.html)와
  [`RunEnd`](/ag-ui-rust/api/ag_ui/client/session/enum.RunEnd.html).
