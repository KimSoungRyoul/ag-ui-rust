---
title: 실행 결과 렌더링
description: 섞여 도착하는 메시지·도구·subagent 변경과 중단 상태를 표시합니다.
---

여러 메시지와 도구 호출은 동시에 열릴 수 있습니다. 메시지 ID와 tool call ID로 변경 대상을 찾습니다.
도구 인자 사이에 도착한 state 이벤트가 그 도구의 전용 상태를 뜻하지는 않습니다.
subagent 출처가 있더라도 state는 실행의 공유 상태를 갱신합니다.

`MessageUpdate`는 현재 조립한 메시지와 변경 종류를 제공합니다.
`Started`, `Content`, `Ended`는 텍스트, `ToolCallStarted`, `ToolCallArgs`, `ToolCallEnded`, `ToolResult`는 도구,
`Activity`와 `EncryptedValue`는 그 밖의 메시지 변경입니다.
`Aborted`는 미완성 메시지 표시를 중단하며 성공 완료를 추정하지 않습니다.

```rust
use ag_ui::client::{MessageChangeKind, Update};
use serde_json::Value;

fn render(update: Update<Value>) {
    match update {
        Update::Message(message) => match message.change {
            MessageChangeKind::Content { delta } => print!("{delta}"),
            MessageChangeKind::Ended => println!(),
            MessageChangeKind::Aborted => println!(" [interrupted]"),
            MessageChangeKind::ToolCallArgs { tool_call_id, delta } => {
                println!("[{tool_call_id}] {delta}");
            }
            _ => {}
        },
        Update::State(state) => println!("state: {state}"),
        Update::Error(error) => eprintln!("{error}"),
        Update::Done(end) => println!("{end:?}"),
        _ => {}
    }
}
```

각 delta를 즉시 그리면 도착 순서가 유지됩니다. 도구 인자가 끝날 때까지 모아서 한 줄로 출력하면 읽기 쉽지만,
그 도구가 열려 있는 동안 도착한 이벤트보다 도구 행이 나중에 표시됩니다.
[board-watch](/ag-ui-rust/ko/examples/board-watch/)는 두 표시 방식을 제공합니다.

## Subagent 구분

`Update::Subagent`는 수명 상태만 담습니다. 하위 텍스트는 일반 `Update::Message`로 오며
`message.subagent_run_id()`로 출처를 찾습니다. 스트림을 소비하는 동안에는 `run.thread()`로 레지스트리를 읽습니다.

```rust
use ag_ui::client::{Thread, Update, transport::ReplayTransport};
use ag_ui::Event;
use futures_util::StreamExt;

# #[tokio::main]
# async fn main() -> Result<(), ag_ui::client::Error> {
let transport = ReplayTransport::new([
    Event::run_started("t", "r"),
    Event::subagent_started("child", "researcher"),
    Event::text_message_chunk(Some("reply".into()), Some("Three sources.".into()))
        .with_subagent_run_id("child"),
    Event::subagent_finished_success("child"),
    Event::run_finished_success("t", "r"),
]).matching_requests();
let mut thread = Thread::new(transport, "t");
let mut run = thread.send("Research this")?;
while let Some(update) = run.next().await {
    match update {
        Update::Message(message) => {
            let name = message.message.subagent_run_id()
                .and_then(|id| run.thread().subagent(id))
                .map_or("agent", |subagent| subagent.name.as_str());
            println!("[{name}] {:?}", message.change);
        }
        Update::Subagent(change) => println!("{}: {:?}", change.subagent.name, change.change),
        _ => {}
    }
}
# Ok(())
# }
```

레지스트리는 실행 중·완료·승인 대기·실패·로컬 중단을 구분합니다.
대기 중인 호출이 같은 ID로 다시 시작되면 `Resumed`로 표시합니다.
부모 `RunError`, 연결 단절, 로컬 abort에서는 아직 실행 중인 하위를 `Aborted`로 정리하며 업무 결과를 추정하지 않습니다.
시작 이벤트 없이 출처만 붙은 메시지도 유효하므로 찾을 수 없는 출처에는 기본 라벨을 표시합니다.

## 상태와 오류

현재 typed 상태는 `thread.state()?`, JSON은 `thread.raw_state()`로 읽습니다.
초기화와 로컬 setter는 즉시 검증합니다. 원격 값이 타입과 다르면 이전 typed 값을 제거하고 진단을 전달합니다.
잘못된 JSON Patch는 이전 raw·typed 상태를 유지합니다.

custom 진행률과 step 이벤트는 `thread.on_event`로 관찰합니다. SDK가 메시지와 상태를 계속 반영하므로
별도 reducer가 필요하지 않습니다. `Update::Error` 후에도 `Done`까지 소비합니다.
`collect_report()`는 앞서 소비한 부분의 진단도 함께 보존합니다.
