---
title: 시작하기
description: Cargo project에 crate를 추가합니다. 첫 agent를 serving하고, Rust client로 말을 걸어 봅니다.
---

AG-UI는 사용자와 맞닿은 application과 agent backend 사이의 protocol입니다. run input을
담은 POST 하나를 보내면 type이 붙은 event stream이 답합니다. 이 SDK는 그 양쪽 끝을
Rust로 줍니다. 직접 띄우는 agent, 그리고 그 agent를 소비하는 client입니다.

이 page가 끝나면 agent가 `http://127.0.0.1:3000/agent`에서 응답합니다. 두 번째
program이 그 agent와 대화하고 있습니다.

:::note
이 site의 Rust block은 전부 workspace의 test suite가 compile합니다. `README`의
quickstart와 같습니다. 낡은 code는 붙여넣어 봐야 아는 것이 아니라 빨간 build로
드러납니다.
:::

## 준비물

Rust **1.85 이상**. workspace가 `rust-version = "1.85"`와 `edition = "2024"`를
설정합니다. 그 edition을 아는 첫 compiler가 1.85입니다.

목록은 이것이 전부입니다. protobuf compiler도 code 생성 단계도 없습니다.
`ag-ui`는 `serde`와 `serde_json`에만 의존합니다. 그 위에 쌓이는 crate는
runtime이 아니라 `futures` primitive를 더합니다. tokio는 `ag_ui::axum`을 쓸 때만
들어옵니다.

Rust 밖으로 나가는 것은 TLS 하나뿐입니다. `ag_ui::client`의 기본 `http` feature가
`rustls`를 쓰는 `reqwest`를 끌어옵니다. 그 crypto backend가 C를 compile합니다. 그래서
client build에는 C toolchain이 필요합니다. agent 쪽에는 그런 것이 없습니다.

## crate 추가하기

:::caution[여기서 말하는 `ag-ui`는 community crate가 아닙니다]
crates.io의 `ag-ui-core`, `ag-ui-server`, `ag-ui-client` 이름은 이전의 무관한 community
SDK의 것이고 이 project가 아닙니다. 이 project는 `ag-ui` crate 하나, 그리고
`ag-ui-a2ui`입니다.
:::

crate는 하나이고, protocol의 어느 쪽을 쓸지는 feature로 정합니다. agent라면
`axum`입니다. `server`를 함의합니다. 여기에 web server 하나:

```toml
# Cargo.toml
[dependencies]
ag-ui = { version = "0.3", features = ["axum"] }
axum = "0.8"
tokio = { version = "1", features = ["rt-multi-thread", "macros", "net"] }
```

client라면 `http`입니다:

```toml
# Cargo.toml
[dependencies]
ag-ui = { version = "0.3", features = ["http"] }
futures-util = "0.3"
tokio = { version = "1", features = ["rt-multi-thread", "macros"] }
```

`Tool`, `Message`, `Event` 같은 protocol type은 어느 쪽이든 crate root에 있습니다.
text 출력 이상을 하는 순간 바로 쓰게 됩니다.

`reqwest` transport를 함께 가져오는 것이 `http`입니다. 대신 `client`를 켜면 crate가
executor를 가리지 않습니다. 아래에 직접 만든 transport를 두고
`wasm32-unknown-unknown`으로 build됩니다.

어느 feature가 무엇을 하는지, 그리고 왜 crate 다섯이 아니라 하나인지는
[crate 구성](/ag-ui-rust/ko/start/crates/)에 있습니다.

## 첫 agent

agent는 trait 구현 하나입니다. `Agent::run`은 run context를 건네받습니다. 그것으로
event를 emit하고, run이 어떻게 끝났는지를 반환합니다.

```rust,no_run
// src/main.rs
use ag_ui::axum::RouterExt;
use ag_ui::RunOutcome;
use ag_ui::server::{Agent, Result, RunContext};
use axum::Router;

struct Greeter;

impl Agent for Greeter {
    type State = ();

    async fn run(&self, ctx: &mut RunContext<()>) -> Result<RunOutcome> {
        // TEXT_MESSAGE_START / _CONTENT / _END로 나갑니다.
        let mut message = ctx.assistant_message()?;
        message.delta("Hello from Rust.")?;
        message.end()?;

        Ok(RunOutcome::Success)
    }
}

#[tokio::main]
async fn main() {
    let app: Router = Router::new().route_agui("/agent", Greeter);

    let listener = tokio::net::TcpListener::bind("127.0.0.1:3000").await.unwrap();
    println!("agent on http://127.0.0.1:3000/agent");
    axum::serve(listener, app).await.unwrap();
}
```

`cargo run`. 이것으로 동작하는 AG-UI endpoint입니다.

짚어 둘 것이 넷입니다.

`type State = ()`는 이 agent가 client와 application state를 공유하지 않는다는 뜻입니다.
대신 `serde` type을 주면 client가 그것을 그대로 비춥니다.
[shared state](/ag-ui-rust/ko/server/state/)를 보세요.

`ctx.assistant_message()`는 context를 가변으로 borrow하는 *handle*을 반환합니다. 이
handle이 살아 있는 동안에는 두 번째 message를 열 수 없습니다. 그래서 둘을 뒤섞으면
frontend가 버텨 내야 할 stream이 아니라 borrow check 오류가 납니다. handle은 drop될 때
`TEXT_MESSAGE_END`도 emit합니다. 위의 `end()`는 편의이지 의무가 아닙니다.
[text streaming](/ag-ui-rust/ko/server/text/)이 둘 다 짚습니다.

`message.delta(…)?`에는 `.await`가 없습니다. handle은 `Drop`에서 종결 event를
emit하는데, `Drop`은 async일 수 없습니다. 그래서 emit 경로 전체가 동기입니다. handle이
channel에 밀어 넣고 transport가 그것을 비웁니다. 이 거래는
[설계 원칙](/ag-ui-rust/ko/design/commitments/)이 설명합니다.

`route_agui`는 `route(path, post(handler))`일 뿐입니다. 돌려주는 router는 평범한 axum
`Router`입니다. 직접 만든 route와 layer와 state를 여느 때처럼 올리면 됩니다.
[HTTP로 serving](/ag-ui-rust/ko/server/axum/)을 보세요.

## 돌아오는 것

```sh
curl -N -X POST http://127.0.0.1:3000/agent \
  -H 'content-type: application/json' \
  -d '{"threadId":"thread-1","runId":"run-1","messages":[],"tools":[],"context":[]}'
```

```text
data: {"type":"RUN_STARTED","threadId":"thread-1","runId":"run-1"}

data: {"type":"TEXT_MESSAGE_START","messageId":"run-1-msg-1","role":"assistant"}

data: {"type":"TEXT_MESSAGE_CONTENT","messageId":"run-1-msg-1","delta":"Hello from Rust."}

data: {"type":"TEXT_MESSAGE_END","messageId":"run-1-msg-1"}

data: {"type":"RUN_FINISHED","threadId":"thread-1","runId":"run-1","outcome":{"type":"success"}}
```

run은 언제나 `RUN_STARTED`로 열립니다. `RUN_FINISHED`와 `RUN_ERROR` 중 하나로만
닫힙니다. 그 사이는 전부 delta입니다. message가 열리고, text가 fragment 단위로
도착하고, message가 닫힙니다. message id는 UUID가 아니라 run id에서 파생했습니다.
기록한 stream을 diff할 수 있는 이유가 그것입니다.

*실패한* run도 `200`입니다. agent가 실패할 수 있는 시점이면 status line은 이미
나갔습니다. 그래서 실패는 잘 형성된 stream 안의 `RUN_ERROR` event로 도착합니다.
client가 "agent가 오류를 냈다"와 "network가 죽었다"를 구별하는 근거입니다.

[AG-UI 동작 방식](/ag-ui-rust/ko/start/protocol/)이 request body와 event 계열과
framing을 제대로 다룹니다.

## port 없이, 같은 run

`ag_ui::axum`은 wrapper입니다. 그 아래에서 `ag_ui::server::run`이 agent를 event
`Stream`으로 바꿉니다. 그 event가 누구에게 어떻게 닿는지에는 관여하지 않습니다. 그래서
agent를 순수한 stream으로 test할 수 있습니다. server도 port도 client도 없이:

```rust
// tests/greeter.rs
use ag_ui::{Event, EventStreamFormatter, EventType, RunAgentInput, RunOutcome, SseFormatter};
use ag_ui::server::{Agent, Result, RunContext, run};
use futures_util::StreamExt;

struct Greeter;

impl Agent for Greeter {
    type State = ();

    async fn run(&self, ctx: &mut RunContext<()>) -> Result<RunOutcome> {
        let mut message = ctx.assistant_message()?;
        message.delta("Hello from Rust.")?;
        message.end()?;

        Ok(RunOutcome::Success)
    }
}

#[tokio::main]
async fn main() {
    let events: Vec<Event> = run(Greeter, RunAgentInput::new("thread-1", "run-1"))
        .map(|event| event.expect("the stream should not break"))
        .collect()
        .await;

    let types: Vec<EventType> = events.iter().map(Event::event_type).collect();
    assert_eq!(
        types,
        [
            EventType::RunStarted,
            EventType::TextMessageStart,
            EventType::TextMessageContent,
            EventType::TextMessageEnd,
            EventType::RunFinished,
        ]
    );

    // endpoint가 써 내보내는 body가 이것입니다. 같은 event를 SSE로 감쌌습니다.
    let formatter = SseFormatter::new();
    let body: String = events
        .iter()
        .map(|event| formatter.encode_to_string(event).unwrap())
        .collect();

    assert!(body.starts_with(r#"data: {"type":"RUN_STARTED","threadId":"thread-1","runId":"run-1"}"#));
    assert!(body.ends_with("\"outcome\":{\"type\":\"success\"}}\n\n"));
}
```

[testing](/ag-ui-rust/ko/design/testing/)가 그 모양으로 agent test를 쓰는 이야기입니다.

## Rust에서 말 걸기

SDK의 나머지 절반은 agent를 소비합니다. `Session`은 thread를 들고 있습니다. 그
message와 state입니다. delta stream을 도로 그 안으로 접어 넣습니다. 그래서 다루는 것은
"`TEXT_MESSAGE_CONTENT`가 도착했다"가 아니라 "이 message가 자랐다"입니다:

```rust,no_run
// src/main.rs
use std::io::Write;

use ag_ui::client::{MessageChangeKind, RunEnd, Session, Update, transport::HttpTransport};
use futures_util::StreamExt;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let transport = HttpTransport::new("http://127.0.0.1:3000/agent")?;
    let mut session = Session::<_>::new(transport, "thread-1");

    let mut run = session.send("hello");
    while let Some(update) = run.next().await {
        match update {
            Update::Message(message) => {
                if let MessageChangeKind::Content { delta } = message.change {
                    print!("{delta}");
                    std::io::stdout().flush()?;
                }
            }
            Update::Error(error) => eprintln!("\n{error}"),
            Update::Done(RunEnd::Success { .. }) => println!(),
            _ => {}
        }
    }
    drop(run);

    println!("{} messages in the thread", session.messages().len());
    Ok(())
}
```

한 terminal에서 agent를, 다른 terminal에서 이것을 실행하세요. `Hello from Rust.`를
fragment 단위로 출력한 뒤 `2 messages in the thread`를 찍습니다. 당신 것과 agent
것입니다.

놓치기 쉬운 것이 둘 있습니다. thread는 *client*에 삽니다. session이 대화와 state를 한
run에서 다음 run으로 나릅니다. agent는 매 request마다 둘 다 건네받습니다. 같은 thread
id로 합류한 두 번째 client가 빈 상태에서 시작하는 이유입니다.

`drop(run)`도 격식이 아닙니다. run은 stream을 흘리는 동안 session을 borrow합니다. 일찍
drop하는 것이 곧 취소입니다. byte를 끌어오는 일이 stream을 poll하는 일이기 때문입니다.

[session](/ag-ui-rust/ko/client/session/)과
[update stream](/ag-ui-rust/ko/client/updates/)이 여기서 이어받습니다.

## 다음으로

- [AG-UI 동작 방식](/ag-ui-rust/ko/start/protocol/) — wire. request body, run
  lifecycle, event 계열, SSE framing.
- [crate 구성](/ag-ui-rust/ko/start/crates/) — crate 둘과 그 feature가 각각 무엇을 위한 것이고
  어떤 일에 어느 것이 필요한지.
- [Agent trait](/ag-ui-rust/ko/server/agent/) — server 쪽 전부. tool call, shared
  state, human in the loop, error와 cancellation.
- [session](/ag-ui-rust/ko/client/session/) — client 쪽 전부. proxy나 recorder가
  원하는 한 단계 아래까지.
- [task-board](/ag-ui-rust/ko/examples/task-board/)와
  [board-watch](/ag-ui-rust/ko/examples/board-watch/) — 실제 port로 서로 대화하는
  agent와 client를 담은 예제 둘.
- [API 문서](/ag-ui-rust/api/ag_ui/index.html) — 모든 crate의 rustdoc.
