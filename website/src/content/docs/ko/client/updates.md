---
title: 업데이트 처리
description: 변경 이벤트와 진단을 받고, 서버 완료와 로컬 중단을 구분합니다.
---

`Thread::send`, `run`, `resume_many`는 `Result<RunStream>`을 반환합니다.
먼저 승인 대기 등 요청 조건을 확인하고, 스트림을 소비하면 chunk 정규화·프로토콜 검증·메시지와 상태 반영을 수행합니다.
`Update` 하나는 변경 하나입니다. 전체 메시지나 별도 실행을 뜻하지 않습니다.

| Update | 화면에 전달하는 값 |
| --- | --- |
| `Message` | 메시지 ID·인덱스·변경 내용·현재 조립된 메시지 |
| `Messages` | 메시지 snapshot을 반영한 전체 대화 |
| `State(S)` | snapshot 또는 patch 반영 후의 상태 |
| `Reasoning` | 대화 본문과 분리된 reasoning 내용 |
| `Subagent` | 호출 이름·부모 관계·수명 상태 |
| `Interrupt` | 실행이 멈추며 전달한 승인 또는 질문 |
| `Error` | 진단. `Done`까지 계속 소비합니다 |
| `Done` | 실행 종료 결과 |

`Update`에는 앞으로 variant가 추가될 수 있습니다. `RunEnd`는 다음 네 가지를 명시적으로 처리합니다.

```rust
use ag_ui::client::RunEnd;

fn label(end: &RunEnd) -> &'static str {
    match end {
        RunEnd::Success { .. } => "완료",
        RunEnd::Interrupted { .. } => "입력 대기",
        RunEnd::Failed { .. } => "실패",
        RunEnd::Aborted => "로컬 중단",
    }
}

assert_eq!(label(&RunEnd::Aborted), "로컬 중단");
```

서버가 성공을 반환해도 클라이언트에서는 state patch 오류 등이 발생할 수 있습니다.
`collect_report()`는 일부 업데이트를 먼저 읽었더라도 **실행 전체**의 종료와 진단을 보존합니다.
`new_messages`는 실행 준비 시점에 없던 ID의 메시지입니다. 기존 메시지 수정 결과는 `thread.messages()`에서 읽습니다.
기본 진단 보관 한도는 100개이며 builder의 `diagnostic_limit` 또는 `set_diagnostic_limit`으로 바꿉니다.
한도를 넘은 개수는 `diagnostics_omitted`에 기록합니다. 개별 `Update::Error` 전달은 생략하지 않습니다.

```rust
use ag_ui::client::{Thread, RunEnd, transport::ReplayTransport};
use ag_ui::Event;

# #[tokio::main]
# async fn main() -> Result<(), ag_ui::client::Error> {
let transport = ReplayTransport::new([
    Event::run_started("t", "r"),
    Event::custom("progress", serde_json::json!({"percent": 50})),
    Event::run_finished_success("t", "r"),
]).matching_requests();
let mut thread = Thread::new(transport, "t");
thread.on_event(|event| {
    if let Event::Custom(progress) = event {
        println!("{}: {}", progress.name, progress.value);
    }
});
let report = thread.send("주문을 확인해줘")?.collect_report().await;
assert!(matches!(report.end, RunEnd::Success { .. }));
assert!(report.diagnostics.is_empty());
# Ok(())
# }
```

관찰 콜백은 decode한 원본 이벤트를 정규화·검증·상태 반영 전에 한 번씩 받습니다.
custom·raw·step 이벤트도 관찰할 수 있습니다. 동기·읽기 전용 콜백이므로 느린 작업은 앱의 큐로 전달합니다.
`on_event` 재호출은 교체이며 `clear_event_observer()`로 제거합니다.
native에서는 `Send + 'static`, wasm에서는 local callback을 허용합니다. decode 실패는 이벤트 대신 진단으로 전달합니다.

`run.abort_handle()`은 연결 또는 응답을 기다리는 poll을 깨워 로컬 수신을 중단합니다.
이미 terminal 이벤트가 반영되었다면 이후 abort로 결과가 바뀌지 않습니다.
그 전의 중단은 `RunEnd::Aborted`를 한 번 전달합니다. 첫 poll 전 drop은 요청과 메시지를 남기지 않습니다.
실행한 스트림의 drop은 thread에 중단을 기록하지만, 소비자가 없으므로 `Done` 전달은 보장하지 않습니다.
이 동작은 원격 작업이 멈췄음을 확인해 주지는 않습니다.

원격 JSON이 `S` 타입에 맞지 않으면 최신 JSON은 `raw_state()`에 보존하고 `state()`는 오류를 반환합니다.
이후 유효한 원격 갱신을 받으면 typed 조회가 복구됩니다. 로컬 `set_state()`는 검증 성공 후 두 표현을 함께 바꿉니다.
초기화·snapshot·승인 복구는 [Thread](/ag-ui-rust/ko/client/thread/)를 참고하세요.

Activity 메시지는 로컬 대화와 snapshot에 보존합니다. 서버로 보내는 `Thread` 요청의 메시지 목록에서는
공식 클라이언트처럼 제외합니다. 저수준 `RemoteAgent::run_events`는 호출자가 준 요청을 그대로 전달합니다.
