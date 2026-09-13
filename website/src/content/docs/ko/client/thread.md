---
title: 대화 스레드
description: 원격 에이전트와 대화하고 상태를 유지하며 승인 요청에 답합니다.
---

`HttpAgent`는 접속 설정, `Thread`는 대화 기록·상태·승인 대기를 소유합니다.
run은 한 번의 요청과 응답 스트림입니다. thread 생성은 로컬 작업이며 서버 기록을
자동으로 불러오지 않습니다. 같은 ID의 thread를 두 번 만들어도 두 로컬 객체가
자동 동기화되지는 않습니다.

## 접속하고 대화하기

```rust,no_run
use ag_ui::client::{HttpAgent, Update};
use futures_util::StreamExt;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let agent = HttpAgent::new("http://localhost:3000/agent")?;
    let mut thread = agent.thread("conversation-1");
    {
        let mut run = thread.send("Hello")?;
        while let Some(update) = run.next().await {
            match update {
                Update::Message(message) => println!("{:?}", message.change),
                Update::Error(error) => eprintln!("{error}"),
                Update::Done(end) => println!("{end:?}"),
                _ => {}
            }
        }
    }
    let report = thread.send("Continue")?.collect_report().await;
    println!("{:?}, diagnostics: {:?}", report.end, report.diagnostics);
    Ok(())
}
```

`send()`의 `?`는 승인 대기 등 요청 전 조건을 검사합니다. 첫 poll 전에는 HTTP 요청을
보내지 않습니다. run이 thread를 빌리는 동안 추가 정보는 `run.thread()`에서 읽습니다.
최종 결과만 필요하면 `collect_report()`를 사용합니다. 서버 성공과 로컬 진단은 별개입니다.

## 타입이 있는 상태

`state()`는 최신 raw state에 대한 `Result<&S, StateViewError>`입니다. 로컬 setter 실패는
이전 값을 유지합니다. 원격 JSON이 타입에 맞지 않으면 raw state는 갱신하고 typed cache는
지우므로 오래된 값을 현재 값으로 읽지 않습니다.

```rust
use ag_ui::client::{Thread, transport::ReplayTransport};
use ag_ui::Event;
use serde::Deserialize;
use serde_json::json;

#[derive(Clone, Debug, Deserialize)]
struct Board { open: u32 }

#[tokio::main]
async fn main() {
    let transport = ReplayTransport::new([
        Event::run_started("board", "r"),
        Event::state_snapshot(json!({"open": 3})),
        Event::run_finished_success("board", "r"),
    ]).matching_requests();
    let mut thread = Thread::<_, Board>::builder(transport, "board")
        .state(json!({"open": 0})).build().unwrap();
    assert_eq!(thread.state().unwrap().open, 0);
    thread.run().unwrap().collect_report().await;
    assert_eq!(thread.state().unwrap().open, 3);
    thread.set_state(json!({"open": 4})).unwrap();
    assert_eq!(thread.state().unwrap().open, 4);
    assert!(thread.set_state(json!({"open": "invalid"})).is_err());
    assert_eq!(thread.raw_state()["open"], 4);
}
```

## 승인과 거절

현재 대기 중인 결정에 모두 답해야 합니다. 일부 응답·중복·미지 ID는 요청 전에 거부합니다.
하나의 승인 거절에는 `decline()`을 사용합니다. 이것은 실행 중지와 다른 API입니다.

```rust
use ag_ui::client::{Thread, InterruptExt, RunEnd, transport::ReplayTransport};
use ag_ui::{Event, Interrupt};
use serde_json::json;

#[tokio::main]
async fn main() {
    let transport = ReplayTransport::with_runs([
        vec![Event::run_started("t", "r1"),
             Event::run_finished_interrupt("t", "r1", vec![
                 Interrupt::new("budget", "approval"),
                 Interrupt::new("date", "approval"),
             ])],
        vec![Event::run_started("t", "r2"), Event::run_finished_success("t", "r2")],
    ]).matching_requests();
    let mut thread = Thread::new(transport, "t");
    thread.send("Plan the trip").unwrap().collect_report().await;
    let pending = thread.interrupts().to_vec();
    assert!(thread.resume(&pending[0], json!(true)).is_err());
    assert_eq!(thread.interrupts().len(), 2);
    let report = thread.resume_many([
        pending[0].resolve(json!(true)), pending[1].cancel(),
    ]).unwrap().collect_report().await;
    assert!(matches!(report.end, RunEnd::Success { .. }));
}
```

승인 응답을 전송한 뒤 결과가 유실되면 제출을 `Unconfirmed`로 보존합니다. SDK는 자동
재전송하지 않습니다. 애플리케이션이 서버의 실제 상태를 확인하고 snapshot으로 복원해야 합니다.
응답 JSON Schema의 검증과 실행 권한 확인은 서버/애플리케이션 책임입니다.

## 관찰·중지·복원

`on_event`는 정규화 전 수신 이벤트를 읽는 동기 콜백입니다. 재호출하면 교체되고
`clear_event_observer()`로 제거합니다. `abort_handle()`은 해당 run만 중지하며 원격 업무의
취소 확인을 뜻하지 않습니다. snapshot에는 대화와 대기 상태가 담기고 transport·인증·observer는
담기지 않습니다. 복원한 뒤 tools/context 같은 요청 설정은 다시 지정합니다.

```rust,no_run
use ag_ui::{Event, Tool};
use ag_ui::client::HttpAgent;
use serde_json::json;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let agent = HttpAgent::builder("http://localhost:3000/agent")
        .header("authorization", "Bearer token").build()?;
    let mut thread = agent.thread("t");
    thread.set_tools(vec![Tool::new("search", "Search", json!({"type":"object"}))]);
    thread.on_event(|event| {
        if let Event::Custom(progress) = event { println!("{}", progress.value); }
    });
    let run = thread.send("Search")?;
    let abort = run.abort_handle();
    // An application's stop handler can call abort.abort().
    let report = run.collect_report().await;
    println!("{:?}", report.end);
    drop(abort);
    let saved = serde_json::to_vec(&thread.snapshot())?;
    let snapshot = serde_json::from_slice(&saved)?;
    let restored = agent.restore_thread(snapshot)?;
    assert_eq!(restored.thread_id(), thread.thread_id());
    Ok(())
}
```

[업데이트 종류](/ag-ui-rust/ko/client/updates/)와
[transport](/ag-ui-rust/ko/client/transports/)에서 저수준 API를 확인할 수 있습니다.
