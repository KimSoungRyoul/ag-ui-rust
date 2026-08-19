---
title: run rendering
description: 도착 순서가 run이 가진 유일한 중첩인 이유, entity 단위 buffering이 치르는 값, 그리고 board-watch가 함께 내놓는 두 가지 rendering.
---

update stream은 entity 단위가 아닙니다. *event* 단위입니다. delta 마흔
개로 흘러 들어오는 답변 하나는 같은 id 아래의 `Update::Message` 마흔
개입니다.

tool call 둘이 동시에 진행되기도 합니다. model이 두 가지를 한꺼번에
요청하면 언제나 그렇습니다. 두 call의 event는 서로 엇갈려 도착합니다.
그 둘을 갈라놓는 것은 id뿐입니다.

renderer는 이 사실 위에 세워집니다. 이것을 틀려도 아무 소리가 나지
않습니다. 무엇도 죽지 않습니다. 실제로 일어나지 않은 순서로 run이 그려질
뿐입니다. 이 페이지는 그 순서가 무엇을 실어 나르는지 다룹니다. 그리고
그것을 포기하면 무엇을 치르는지 다룹니다.

## 연달아 오는 update가 한 덩어리라는 보장은 없습니다

어느 쪽도 닫히기 전에 call 둘이 열립니다. 두 call의 argument 조각은
wire에서 번갈아 나타납니다. client는 순서를 바꾸지 않습니다. 그래서
update stream에서도 번갈아 나타납니다.

```rust
// src/main.rs
use ag_ui::client::{MessageChangeKind, Session, Update, transport::ReplayTransport};
use ag_ui::Event;
use futures_util::StreamExt;

#[tokio::main]
async fn main() {
    let transport = ReplayTransport::new([
        Event::run_started("thread-1", "run-1"),
        Event::tool_call_start("call-1", "add_task"),
        Event::tool_call_start("call-2", "add_task"),
        Event::tool_call_args("call-1", r#"{"title":"#),
        Event::tool_call_args("call-2", r#"{"title":"#),
        Event::tool_call_args("call-1", r#""write it down"}"#),
        Event::tool_call_args("call-2", r#""read it back"}"#),
        Event::tool_call_end("call-1"),
        Event::tool_call_end("call-2"),
        Event::run_finished_success("thread-1", "run-1"),
    ]);

    let mut session = Session::<_>::new(transport, "thread-1");
    let updates: Vec<_> = session.send("add two things").collect().await;

    let fragments: Vec<String> = updates
        .iter()
        .filter_map(|update| match update {
            Update::Message(message) => match &message.change {
                MessageChangeKind::ToolCallArgs { tool_call_id, .. } => {
                    Some(tool_call_id.to_string())
                }
                _ => None,
            },
            _ => None,
        })
        .collect();

    // 조각 넷, call 둘, 엄격하게 번갈아 옵니다. 각 delta를 "지금 진행
    // 중인 call"에 이어 붙이는 renderer는 뒤죽박죽인 한 줄을 씁니다.
    assert_eq!(fragments, ["call-1", "call-2", "call-1", "call-2"]);
}
```

그러니 각 change에 붙은 id는 장식이 아닙니다. 어떤 조각이 어느 call에
속하는지 말해 주는 유일한 것입니다.

## 도착 순서가 유일한 중첩입니다

protocol에는 포함 관계가 없습니다. 있는 것은 순서열뿐입니다. wire는
event가 그 순서열의 어디에 놓였는지만 말합니다. 그 event가 도착했을 때
무엇이 열려 있었는지도 거기서만 알 수 있습니다.

이를 손에 잡히게 만드는 사례가 있습니다. 어떤 agent는 tool이 할 일을
*call이 열려 있는 동안* 처리합니다. 그런 agent는 `TOOL_CALL_ARGS`와
`TOOL_CALL_END` 사이에서 state를 내보냅니다. `ag_ui::serve`의 handle이
이를 지원합니다. `STATE_*`에는 순서 제약이 없으니 protocol도
허용합니다. 그렇게 나온 `Update::State`에는 그 call에 대한 언급이 전혀
없습니다.

이는 field 하나가 채워 주기를 기다리는 누락이 아닙니다. 병렬 call에서는
call 둘이 동시에 열려 있습니다. 그 state가 어느 쪽 것인지 wire 자체가
말해 주지 않습니다. 그러니 어느 쪽에 붙이든 보고가 아니라 지어낸 것이
됩니다. **ordering이 곧 계약입니다.**

도착 순서대로 그리는 renderer는 실제로 일어난 일을 보여 줍니다. entity
단위로 buffering하는 renderer는 순서를 바꾸기로 선택한 것입니다.

## buffering이 치르는 값

call 하나를 한 줄로 그리려고 argument를 모아 두는 것은 바랄 만한
일입니다. 그편이 읽기 좋으니까요. 대신 치르는 값이 있습니다. call이
닫히기 전에는 그 줄을 쓸 수 없습니다. 그래서 call이 열려 있는 동안 도착한
것들이 그 줄보다 *먼저* 그려집니다.

아래는 하나의 update stream에 두 rendering을 모두 적용한 것입니다. 그러면
차이는 그리는 방식뿐입니다.

```rust
// src/render.rs
use ag_ui::client::{MessageChangeKind, Session, Update, transport::ReplayTransport};
use ag_ui::{Event, ToolCallId};
use futures_util::StreamExt;
use serde_json::json;

/// call id의 꼬리. 한 기록 안에서 둘을 구분하기에는 충분합니다.
fn short(id: &ToolCallId) -> &str {
    let id = id.as_str();
    id.rsplit('-').next().unwrap_or(id)
}

/// update 하나에 한 줄, 도착 순서대로. tool 줄에는 어느 call인지 적습니다.
fn in_order(update: &Update, out: &mut Vec<String>) {
    match update {
        Update::Message(message) => match &message.change {
            MessageChangeKind::ToolCallStarted { tool_call_id, name } => {
                out.push(format!("call {name} ({})", short(tool_call_id)));
            }
            // 이름을 붙입니다. 도착 순서에서는 두 call의 조각이 서로
            // 붙어 있고, 그것을 갈라놓는 것은 id뿐이기 때문입니다.
            MessageChangeKind::ToolCallArgs { tool_call_id, delta } => {
                out.push(format!("args ({}) {delta}", short(tool_call_id)));
            }
            // `ToolCallEnded`에는 id만 실려 옵니다. 이름은
            // `ToolCallStarted`에 왔습니다. 여기서 이름이 필요한
            // renderer는 map을 들고 있어야 합니다.
            MessageChangeKind::ToolCallEnded { tool_call_id } => {
                out.push(format!("end  ({})", short(tool_call_id)));
            }
            _ => {}
        },
        Update::State(state) => out.push(format!("state {state}")),
        _ => {}
    }
}

/// event가 몇 개였든 call 전체를 한 줄로. 그 말은 call이 *닫힐* 때
/// 비로소 줄을 쓴다는 뜻입니다.
#[derive(Default)]
struct Grouped {
    open: Vec<(ToolCallId, String, String)>,
}

impl Grouped {
    fn draw(&mut self, update: &Update, out: &mut Vec<String>) {
        match update {
            Update::Message(message) => match &message.change {
                MessageChangeKind::ToolCallStarted { tool_call_id, name } => {
                    self.open
                        .push((tool_call_id.clone(), name.clone(), String::new()));
                }
                MessageChangeKind::ToolCallArgs { tool_call_id, delta } => {
                    if let Some(call) = self.open.iter_mut().find(|call| &call.0 == tool_call_id) {
                        call.2.push_str(delta);
                    }
                }
                MessageChangeKind::ToolCallEnded { tool_call_id } => {
                    if let Some(at) = self.open.iter().position(|call| &call.0 == tool_call_id) {
                        let (_, name, args) = self.open.remove(at);
                        out.push(format!("call {name} {args}"));
                    }
                }
                _ => {}
            },
            Update::State(state) => out.push(format!("state {state}")),
            _ => {}
        }
    }
}

#[tokio::main]
async fn main() {
    // 자기 call 안에서 state를 내보내는 agent입니다.
    // `examples/task-board`가 그렇게 하고, protocol도 허용합니다.
    let transport = ReplayTransport::new([
        Event::run_started("thread-1", "run-1"),
        Event::tool_call_start("call-1", "add_task"),
        Event::tool_call_args("call-1", r#"{"title":"draft "#),
        Event::state_snapshot(json!({ "open": 1 })),
        Event::tool_call_args("call-1", r#"the agenda"}"#),
        Event::tool_call_end("call-1"),
        Event::run_finished_success("thread-1", "run-1"),
    ]);

    let mut session = Session::<_>::new(transport, "thread-1");
    let updates: Vec<_> = session.send("add one thing").collect().await;

    let mut ordered = Vec::new();
    let mut grouped = Vec::new();
    let mut state = Grouped::default();
    for update in &updates {
        in_order(update, &mut ordered);
        state.draw(update, &mut grouped);
    }

    // 도착 순서는 state를 call의 argument와 그 끝 사이에 둡니다.
    // wire가 그것을 둔 자리가 거기이기 때문입니다.
    assert_eq!(
        ordered,
        [
            r#"call add_task (1)"#,
            r#"args (1) {"title":"draft "#,
            r#"state {"open":1}"#,
            r#"args (1) the agenda"}"#,
            r#"end  (1)"#,
        ]
    );

    // 묶어 그리는 쪽은 call이 닫힐 때 줄을 씁니다. 그동안 일어난
    // state는 이미 화면에 올라가 있습니다.
    assert_eq!(
        grouped,
        [
            r#"state {"open":1}"#,
            r#"call add_task {"title":"draft the agenda"}"#,
        ]
    );
}
```

어느 쪽이 더 옳지는 않습니다. 묶어 그리는 쪽은 읽기 좋습니다. 대신
call 안에서 일어난 일의 순서를 바꿉니다. 충실한 쪽은 시끄럽습니다.
대신 그것을 보여 줄 수 있습니다. 가질 수 없는 것은 하나입니다. call을 한
줄로 그리면서 **동시에** 순서를 지키는 것. call이 닫히기 전에는 그 줄을
쓸 수 없기 때문입니다.

:::tip
병렬 call에서 읽을 수 있게 만들어 주는 것은 buffering이 아닙니다. 각
줄에 call id를 붙이는 일입니다. 그 꼬리표가 없으면 call 둘이 열려 있을
때 충실한 rendering은 읽을 수 없습니다. 붙이면 괜찮습니다. 이 저장소에 이
이야기를 처음 적었을 때는 결론이 반대였습니다. 그 정정은
`examples/board-watch/tests/client.rs`의 test로 고정되어 있습니다.
:::

## board-watch가 내놓는 두 가지 rendering

`examples/board-watch`는 어떤 AG-UI agent에도 붙일 수 있는 terminal
client입니다. 두 rendering을 모두 내놓습니다. `task-board`는 call 안에서
state를 내보냅니다. 그것을 상대로 기본값인 묶어 그리는 view는 이렇게
그립니다.

```text
  state  1 open · 0 done
  call   add_task {"title":"draft the agenda"}
  result {"id":1,"title":"draft the agenda"}
```

그리고 `--in-order`는 이렇게 그립니다.

```text
  call   add_task (1)
  args   (1) {"title":"draft the agenda"}
  state  1 open · 0 done
  end    add_task (1)
  result {"id":1,"title":"draft the agenda"}
```

권하는 바는 이렇습니다. 대화를 읽을 때는 묶어 그리는 view를 쓰세요.
대화를 debug할 때는 `--in-order`를 쓰세요. 둘 다 integration test가
돌립니다. binary가 실행하는 바로 그 함수에, 키보드 대신 script로 짠
`&[u8]`을, 화면 대신 `Vec<u8>`을 물린 test입니다. 그래서 README에 실린
기록은 예시가 아니라 단언입니다. test는 한쪽에서 순서가 바뀌었음을
단언합니다. 다른 쪽에서는 순서가 충실함을 단언합니다. 그래서 어느
renderer가 바뀌든 소리 없이 지나가지 않습니다.

나머지는 [board-watch](/ag-ui-rust/ko/examples/board-watch/)에
있습니다.

## 직접 다루지 않아도 되는 것들

rendering 문제처럼 보이는 것 가운데 일부는 update가 여기 닿기 전에 이미
처리되어 있습니다.

**chunk event.** 자기 출력을 여닫는 괄호로 감싸지 못하는 provider
adapter는 `*_CHUNK` event를 보냅니다. 이 event는 맨 처음 하나에만 id를
싣습니다. normalizer가 이것을 명시적인 `Started` / `Content` / `Ended`
세 짝으로 바꿉니다. 그래서 renderer는 언제나 괄호로 감싼 형태만 봅니다.
여러 event에 걸쳐 id를 기억할 일도 없습니다.

**끝나지 않은 message.** producer가 마지막 message를 끝내 닫지 않으면
stream의 끝이 대신 닫아 줍니다. normalizer가 아직 갚지 않은 종료
event를 run이 끝나기 전에 emit합니다. 그렇지 않았다면
`MessageChangeKind::Ended`에서 입력 중 표시를 감추는 view는 영영 돌기만
했을 것입니다.

**어긋난 stream.** ordering rule을 어긴 event는 `Update::Error`로
보고됩니다. 그리고 **적용되지 않습니다**. 예외는 run을 끝내는
event뿐입니다. 그것만은 어쨌든 적용합니다. 호출자를 기다리게 두지
않으려고요. 그래서 대화에는 깨진 stream으로 조립한 state가 들어가지
않습니다. verification을 끄면 그런 event도 그대로 적용됩니다. 그때
치르는 값은 진단이지 대화가 아닙니다.

**reasoning의 수명 주기.** protocol은 하나의 생각을 두 번 감쌉니다.
`REASONING_START`가 block을 엽니다. `REASONING_MESSAGE_START`가 그 안의
message를 엽니다. 둘은 같은 id를 씁니다. 그래도
`ReasoningChangeKind::Started`와 `Ended`는 id마다 **한 번씩만**
도착합니다. 그래서 끝난 생각을 출력하는 view는 한 번만 출력합니다. 중복
제거는 필요 없습니다.

## 다시 그릴 때의 지침

- `MessageUpdate::index`는 바뀐 행입니다. 그 한 행만 다시 그리세요.
- `Update::Messages`는 `MESSAGES_SNAPSHOT`이 대화를 통째로 갈아
  치웠다는 뜻입니다. message가 *사라졌을* 수도 있으니 전부 다시
  그리세요.
- `Update::State`는 새 state를 값으로 실어 나릅니다. run이 지나간
  뒤에도 view가 들고 있을 수 있습니다.
- `Update::Error`는 종료 신호가 아닙니다. 출력하고 계속 진행하세요.
  끝났다는 말은 run이 합니다.
- `Update::Done`은 빠져나가는 모든 경로에서 run의 마지막 update입니다.
  입력창을 다시 여는 자리가 여기입니다. [update
  stream](/ag-ui-rust/ko/client/updates/#run이-끝나는-세-가지-방법)을
  보세요.

## 다음

- [update stream](/ag-ui-rust/ko/client/updates/) — variant 그 자체.
- [session](/ag-ui-rust/ko/client/session/) — 그리는 동안 무엇이 쌓이고
  있는지.
- API 문서의
  [`MessageChangeKind`](/ag-ui-rust/api/ag_ui/client/apply/enum.MessageChangeKind.html).
