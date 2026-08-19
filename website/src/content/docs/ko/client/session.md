---
title: session
description: 원격 agent에 session을 열고, 한 턴을 보내고, session이 쌓아 두는 message와 state를 읽는 방법.
---

thread와 run은 protocol의 단어입니다. `Session`은 이 SDK의 단어입니다.
wire에 실리는 것은 `threadId`와 `runId`뿐입니다. session도 thread
object도 없고, id 하나뿐입니다. `Session`은 그 id *위에* 놓여
transport, 지금까지의 대화, typed state, tool을 더합니다. 이름을
`Thread`로 하지 않은 것은 일부러입니다. 없는 protocol entity가 있는
것처럼 읽히기 때문입니다.

AG-UI run은 delta로 도착합니다. message가 열립니다. text는 조각 단위로
옵니다. tool argument는 부분 JSON으로 쌓입니다. state는 RFC 6902
patch로 움직입니다. run이 사람에게 무언가를 물으려고 멈추기도 합니다.

`Session`은 event가 도착하는 대로 이 전부를 다시 하나의 대화로 접어
넣습니다.

`Session`은 상위 계층입니다. 그 아래에 `RemoteAgent`가 있습니다.
`RemoteAgent`는 agent가 보낸 event를 조립하지 않고 그대로 건네줍니다.
proxy, recorder, 다른 protocol로 잇는 bridge에는 그쪽이 맞습니다. 이
페이지가 다루는 것은 user interface가 원하는 계층입니다.

## session 열기

session에는 transport와 thread id가 필요합니다. 나머지는 session이
스스로 만듭니다. 지금까지의 대화, 지금까지의 state, 새 run id가
그렇습니다.

```rust
// src/main.rs
use ag_ui::client::{RunEnd, Session, Update, transport::ReplayTransport};
use ag_ui::{Event, TextMessageRole};
use futures_util::StreamExt;

#[tokio::main]
async fn main() {
    // script로 짜 둔 agent입니다. server도 network도 없이 돌아갑니다.
    let transport = ReplayTransport::new([
        Event::run_started("thread-1", "run-1"),
        Event::text_message_start("msg-1", TextMessageRole::Assistant),
        Event::text_message_content("msg-1", "It is "),
        Event::text_message_content("msg-1", "sunny."),
        Event::text_message_end("msg-1"),
        Event::run_finished_success("thread-1", "run-1"),
    ]);

    let mut session = Session::<_>::new(transport, "thread-1");

    let mut ended = None;
    let mut run = session.send("what is the weather?");
    while let Some(update) = run.next().await {
        match update {
            Update::Message(message) => println!("{}: {:?}", message.id, message.change),
            Update::Done(end) => ended = Some(end),
            _ => {}
        }
    }
    drop(run);

    assert!(matches!(ended, Some(RunEnd::Success { .. })));
    // user의 턴과 agent의 답이 모두 thread에 들어갔습니다.
    // 다음 `send`가 둘을 함께 실어 갑니다.
    assert_eq!(session.messages().len(), 2);
}
```

`send`는 `RunStream`을 돌려줍니다. 이 stream은 run을 poll하는 동안
session을 mutable로 borrow합니다. 그 borrow 덕분에 run이 끝나는 순간
`session.messages()`가 정확해집니다. stream을 drop하면 session을 다시
읽을 수 있습니다.

실제 agent를 상대할 때 달라지는 것은 transport뿐입니다.

```rust,no_run
// src/main.rs
use ag_ui::client::{Session, Update, transport::HttpTransport};
use futures_util::StreamExt;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let transport = HttpTransport::builder("http://localhost:3000/agent")
        .header("authorization", "Bearer …")
        .connect_timeout(std::time::Duration::from_secs(5))
        .build()?;

    let mut session = Session::<_>::new(transport, "thread-1");

    let mut run = session.send("hello");
    while let Some(update) = run.next().await {
        if let Update::Message(message) = update {
            println!("{:?}", message.change);
        }
    }
    Ok(())
}
```

`HttpTransport`는 crate에 들어 있는 두 transport 중 하나입니다. 다른
하나는 script를 재생합니다. 세 번째를 직접 쓰는 데 필요한 것은 trait
method 하나입니다. [transport](/ag-ui-rust/ko/client/transports/)를
보세요.

## transport bound는 생성자에 있습니다

`Session<T, S>`에는 `T: Transport` bound가 없습니다. bound는
`Session::new`, `Session::builder`, `SessionBuilder::new`에 붙어
있습니다. 실제로 요청을 보내는 method도 마찬가지입니다.

당장의 이득은 실수가 저질러진 자리에서 잡힌다는 것입니다. URL은
transport를 *만드는 재료*입니다. 그래서 transport 자리에 URL을 넘기는
code는 그럴듯해 보입니다.

```rust,compile_fail,E0277
use ag_ui::client::Session;

// error[E0277]: the trait bound `str: Transport` is not satisfied
//   — 그리고 `help:` note가 이 trait을 구현한 type을 알려 줍니다.
//
// `&str`이 아니라 `str`인 것은 blanket `impl Transport for &T` 때문입니다.
let session = Session::<_>::new("http://localhost:3000/agent", "thread-1");
```

생성자에 bound가 없다면 이 error는 첫 `send`에서 납니다. 그 자리는
보통 다른 파일입니다.

두 번째 이득은 그 아래 모든 code가 가져갑니다. struct 정의에 붙은
bound는 전염됩니다. `Session<T, S>`에 `T: Transport`를 붙여 보세요. 그
type을 이름으로 쓰기만 하는 helper도 전부 같은 bound를 되풀이해야
합니다. `messages()`만 읽는 helper도 예외가 아닙니다. view 계층은
아무것도 보내지 않습니다. 그래서 transport를 이름으로 쓸 일이
없습니다.

```rust
// src/view.rs
use ag_ui::client::{Session, transport::ReplayTransport};
use ag_ui::Message;

/// status bar에 쓸 agent의 마지막 한 줄.
/// `T: Transport`는 없습니다. 읽기만 하니까요.
fn last_reply<T, S>(session: &Session<T, S>) -> Option<&str> {
    session.messages().iter().rev().find_map(|message| match message {
        Message::Assistant(assistant) => assistant.content.as_deref(),
        _ => None,
    })
}

fn main() {
    let session = Session::<_>::builder(ReplayTransport::new([]), "thread-1")
        .messages(vec![Message::assistant("a-1", "Two open tasks.")])
        .build();

    assert_eq!(last_reply(&session), Some("Two open tasks."));
}
```

양쪽 절반 모두 test로 고정되어 있습니다. 첫 번째는 `Session::new`에
붙은 `compile_fail,E0277` doctest입니다. 두 번째는
`crates/ag-ui/tests/client_bounds.rs`입니다. application이 helper를
쓰듯 작성한 test입니다. bound가 다시 type 쪽으로 옮겨 가면 이 test는
컴파일에 실패합니다.

## builder

`Session::builder`는 이미 무언가를 담은 채 시작하는 session을 위한
것입니다. 저장소에서 불러온 이력, client가 소유한 state 문서, agent가
호출할 수 있는 tool 집합이 그렇습니다.

```rust
// src/main.rs
use ag_ui::client::{Session, transport::ReplayTransport};
use ag_ui::{Message, Tool};
use serde_json::json;

fn main() {
    let session = Session::<_>::builder(ReplayTransport::new([]), "thread-1")
        .messages(vec![
            Message::user("u-1", "what is on the board?"),
            Message::assistant("a-1", "Two open tasks."),
        ])
        .state(json!({ "open": 2, "done": 0 }))
        .tools(vec![Tool::new(
            "add_task",
            "Add a task to the board.",
            json!({
                "type": "object",
                "properties": { "title": { "type": "string" } },
                "required": ["title"],
            }),
        )])
        .build();

    assert_eq!(session.messages().len(), 2);
    assert_eq!(session.raw_state()["open"], 2);
}
```

`context`와 `forwarded_props`는 protocol이 해석하지 않는 두 passthrough
field를 설정합니다. `verify`는 [client 측 protocol
verification](/ag-ui-rust/ko/design/verification/)을 끕니다. 기본값은
켜짐입니다. 끄는 쪽은 별난 구석을 감수하기로 한 producer를 상대할 때
씁니다. `applier`는 어느 쪽이든 관대합니다. 잃는 것은 진단이지 대화가
아닙니다.

:::caution[tool은 client에서, 매 요청마다 함께 갑니다]
AG-UI에는 tool discovery도 협상도 없습니다. agent는 받지 못한 tool을
달라고 할 수 없습니다. 그래서 tool이 필요한 agent에게 아무것도 주지
않아도 이 crate는 tool이 없다는 error를 내지 않습니다. 대신 *agent
자신의* error가 옵니다. "the client offered no add_task tool" 같은, 그
agent가 하는 말입니다. 평범한 실패 run으로 도착합니다. agent의
버그처럼 읽히지만 버그가 아닙니다. 특정 agent를 겨냥하지 않은 client는
URL을 설정하듯 tool 집합도 설정해야 합니다. `SessionBuilder::tools`를
쓰세요. 다음 run부터 적용되는 `Session::set_tools`도 있습니다. 반대편
이야기는 [tool call](/ag-ui-rust/ko/server/tools/)에 있습니다.
:::

## typed state

`Session<T, S = Value>`에는 두 번째 parameter가 있습니다. application
state를 담을 type입니다. 이 type은 `Update::State`로 무엇을 하는지에서
추론됩니다. `Session<T, Board>`를 받는 함수에 session을 넘겨 보세요.
turbofish는 필요 없습니다. match 갈래에서 state를 typed local에 담을
때도 마찬가지입니다. 아무것도 그 type을 지목하지 않을 때만 직접
적으세요.

```rust
// src/main.rs
use ag_ui::client::{Session, Update, transport::ReplayTransport};
use ag_ui::{Event, PatchOperation};
use futures_util::StreamExt;
use serde::Deserialize;
use serde_json::json;

/// agent의 state를 client 자신의 type으로.
#[derive(Clone, Debug, Deserialize, PartialEq)]
struct Board {
    open: u32,
    done: u32,
}

#[tokio::main]
async fn main() {
    let transport = ReplayTransport::new([
        Event::run_started("thread-1", "run-1"),
        Event::state_snapshot(json!({ "open": 2, "done": 0 })),
        Event::state_delta(vec![PatchOperation::replace("/done", 1)]),
        Event::run_finished_success("thread-1", "run-1"),
    ]);

    let mut session = Session::<_, Board>::new(transport, "thread-1");

    let mut latest = None;
    let mut run = session.send("mark one done");
    while let Some(update) = run.next().await {
        if let Update::State(board) = update {
            latest = Some(board);
        }
    }
    drop(run);

    // snapshot과 patch는 같은 종류의 update로 도착합니다.
    // 여기서는 둘을 구분할 수 없고, 구분할 필요도 없습니다.
    assert_eq!(latest, Some(Board { open: 2, done: 1 }));
    assert_eq!(session.state(), Some(&Board { open: 2, done: 1 }));
    // type에 맞든 아니든 원본 JSON은 언제나 함께 있습니다.
    assert_eq!(session.raw_state()["done"], 1);
}
```

update를 stream으로 받으려면 `S`가 `Deserialize + Clone + Unpin`이어야
합니다. `Update::State`는 state를 값으로 실어 나릅니다. 그래서 run이
지나간 뒤에도 view가 들고 있을 수 있습니다. 평범한 struct라면
`#[derive(Clone, Deserialize)]` 하나면 됩니다.

type에 맞지 않는 state가 와도 run을 잃지는 않습니다. `raw_state`는
계속 갱신되고 정확합니다. 뒤처지는 것은 typed 쪽뿐입니다. 그 사실은
`Update::Error`로 알려집니다.

:::note
`session.state()`가 `None`이라는 것은 state가 비었다는 뜻이 아닙니다.
`STATE_*` event가 하나도 오지 않았다는 뜻입니다. 둘은 다른 이야기이고,
대개는 agent가 고장 났다는 신호입니다. `board-watch`가 이 둘을 일부러
다르게 그리는 이유입니다.
:::

## run 시작하기

들어가는 길은 셋입니다. 모두 같은 `RunStream`을 돌려줍니다.

| 호출 | 무엇을 보내는가 |
| --- | --- |
| `send(text)` | user message를 덧붙인 뒤 run합니다. |
| `send_message(message)` | 역할이 무엇이든 message를 덧붙인 뒤 run합니다. |
| `run()` | 대화를 지금 그대로 두고 run합니다. |

`send`는 요청이 나가기 *전에* user의 턴을 덧붙입니다. 그래서 run이
어떻게 되든 그 턴은 `session.messages()`에 남습니다. `run()`은 client가
혼자 한 일 뒤를 이어 갈 때 씁니다. 로컬에서 계산해 `push_message`로
넣은 tool 결과가 그렇습니다. client가 `set_state`로 지정한 state도
마찬가지입니다.

run id는 `{thread}-run-{n}` 형태로 생성됩니다. `set_next_run_id`는 다음
id를 직접 지정합니다. 재개를 run id로 식별하는 server에는 필요하지만,
대부분의 server에는 필요 없습니다.

## session이 쌓아 두는 것

session을 읽는 데는 진행 중인 run도, transport bound도 필요 없습니다.

| 접근자 | 무엇이 담기는가 |
| --- | --- |
| `messages()` | 모든 run에 걸쳐 조립된 대화. 오래된 것부터. |
| `state()` | deserialize되는 state가 한 번이라도 왔다면, `S` type의 application state. |
| `raw_state()` | 같은 state를 JSON으로. 언제나 최신입니다. |
| `reasoning()` | 대화 기록과 분리해 둔 reasoning message. |
| `interrupts()` | 마지막 run이 멈췄다면, agent가 기다리는 것. |
| `thread_id()` | 이 session이 속한 대화. |
| `applier()` | 그 아래에서 도는 state machine. 조립된 원본 형태를 보려는 view를 위한 것. |
| `agent()` | 한 계층 아래로 내려가기 위한 `RemoteAgent`. |

## 멈춤에 답하기

run은 성공하거나 실패하기만 하지 않습니다. 멈출 수도 있습니다. agent는
사람이 결정할 것들을 나열한 interrupt outcome으로 run을 끝냅니다.
client가 그 답을 돌려보내면 대화가 이어집니다.

```rust
// src/main.rs
use ag_ui::client::{RunEnd, Session, Update, transport::ReplayTransport};
use ag_ui::{Event, Interrupt};
use futures_util::StreamExt;
use serde_json::json;

#[tokio::main]
async fn main() {
    let transport = ReplayTransport::with_runs([
        vec![
            Event::run_started("thread-1", "run-1"),
            Event::run_finished_interrupt(
                "thread-1",
                "run-1",
                vec![Interrupt::new("i-1", "tool_approval")],
            ),
        ],
        vec![
            Event::run_started("thread-1", "run-2"),
            Event::run_finished_success("thread-1", "run-2"),
        ],
    ]);

    let mut session = Session::<_>::new(transport, "thread-1");

    let mut paused = Vec::new();
    let mut run = session.send("delete the staging database");
    while let Some(update) = run.next().await {
        if let Update::Interrupt(interrupt) = update {
            paused.push(interrupt);
        }
    }
    drop(run);

    // 같은 interrupt가 다음 run이 시작될 때까지 session에 남아 있습니다.
    assert_eq!(session.interrupts().len(), 1);

    // 사람에게 묻고 agent에 답합니다. 나머지 절반은
    // `session.cancel(&interrupt)`입니다. 사람이 거절한 경우죠.
    let mut ended = None;
    let mut resumed = session.resume(&paused[0], json!({ "approved": true }));
    while let Some(update) = resumed.next().await {
        if let Update::Done(end) = update {
            ended = Some(end);
        }
    }
    drop(resumed);

    assert!(matches!(ended, Some(RunEnd::Success { .. })));
}
```

run은 한 번에 둘 이상의 결정에서 멈출 수 있습니다. 그 답은 **한 번의**
요청으로 함께 보냅니다. 요청마다 하나씩 답하면 끝나지 않습니다. 재개된
run이 멈춰 있던 run을 대체하기 때문입니다. agent는 재개 요청이 실어 온
것만 봅니다. 답하지 않고 남긴 것은 버려집니다. `resume_many`는 답을
한꺼번에 받습니다. `ResumeBuilder`는 결정을 하나씩 모읍니다.

```rust
// src/main.rs
use ag_ui::client::interrupts::ResumeBuilder;
use ag_ui::{Interrupt, ResumeStatus};
use serde_json::json;

fn main() {
    let budget = Interrupt::new("approve-budget", "tool_approval");
    let date = Interrupt::new("confirm-date", "tool_approval");

    let entries = ResumeBuilder::new()
        .resolve(&budget, json!({ "approved": true }))
        .cancel(&date)
        .build();

    assert_eq!(entries[0].interrupt_id, "approve-budget");
    assert_eq!(entries[1].status, ResumeStatus::Cancelled);
    // `session.resume_many(entries)`는 두 답을 한 요청으로 보냅니다.
}
```

답의 모양은 agent가 정합니다. interrupt가 `responseSchema`를 실어
왔다면 payload는 그것을 만족해야 합니다. 이 crate가 아는 모양은
`resolve_with_edits` 하나뿐입니다. 이 method는 `editedArgs` key를 대신
써 줍니다. `approveWithEdits`를 내세우는 agent가 기대하는 key입니다.
호출자가 기억할 필요가 없습니다.

재개된 run은 새 run id를 받습니다. 멈춰 있던 run의 id가 아닙니다. 자기
몫의 `RUN_STARTED`를 emit하기 때문입니다. 끝난 run의 id를 다시 쓰면 한
thread 안의 두 run을 log에서 구분할 수 없습니다.

이 이야기의 agent 쪽 절반은 [human in the
loop](/ag-ui-rust/ko/server/interrupts/)에 있습니다.

## 중단하기

`Session::cancel`은 interrupt에 답합니다. run을 중단하지는 않습니다.
run을 중단하는 method는 없습니다. byte를 끌어오는 것이 stream poll이기
때문입니다. stream을 놓아 버리는 것이 client 쪽 취소의 전부입니다.

그 drop이 저쪽 끝까지 닿는지는 client에서 보이지 않습니다. 그래서
`board-watch`가 반대편에서 증명합니다. integration test는 run을 stream
도중에 drop합니다. 상대 agent는 자신이 취소됐는지를 future가 빠져나갈 때
보고합니다. test는 그 run이 단순히 drop된 것이 아니라 취소되었음을
단언합니다. 그 뒤에도 session은 계속 쓸 수 있습니다. 다음 run은 여느
run과 다르지 않습니다.

## 다음

- [update stream](/ag-ui-rust/ko/client/updates/) — 모든 `Update`
  variant, 그리고 run이 끝나는 세 가지 방법.
- [run rendering](/ag-ui-rust/ko/client/rendering/) — 도착 순서가 유일한
  중첩인 이유. 그것을 무시한 renderer가 무엇을 틀리는지.
- API 문서의
  [`Session`](/ag-ui-rust/api/ag_ui/client/session/struct.Session.html).
