---
title: 연결과 대화 관리
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

구현 방법은 [전용 컴포넌트 가이드](/ag-ui-rust/ko/v1.0.0/client/state/)에서 확인합니다.

## 승인과 거절

구현 방법은 [전용 컴포넌트 가이드](/ag-ui-rust/ko/v1.0.0/client/interrupts/)에서 확인합니다.

## 관찰·중지·복원

구현 방법은 [전용 컴포넌트 가이드](/ag-ui-rust/ko/v1.0.0/client/interrupts/)에서 확인합니다.

## 같은 대화에서 요청 두 개 실행하기

한 `Thread`에서는 요청 하나가 stream이 끝날 때까지 thread를 빌립니다. 동시
작업이 필요하면 같은 대화 ID와 공통 snapshot으로 두 번째 `Thread`를 만듭니다.
복원 후에는 tools, context, forwarded properties를 다시 설정합니다. 진행 중에는
`merge_messages`로 화면에 보여 줄 목록을 만들고, 두 stream을 소비한 뒤
`Thread::merge_messages_from`으로 새 메시지를 합칩니다.

```rust
use ag_ui::client::{RunEnd, Thread, merge_messages, transport::ReplayTransport};
# use ag_ui::{Event, TextMessageRole};
use futures_util::StreamExt;
# fn reply(id: &str, text: &str) -> Vec<Event> {
#     vec![
#         Event::run_started("thread-1", "run"),
#         Event::text_message_start(id, TextMessageRole::Assistant),
#         Event::text_message_content(id, text),
#         Event::text_message_end(id),
#         Event::run_finished_success("thread-1", "run"),
#     ]
# }
# let transport = ReplayTransport::with_runs([
#     reply("report", "Here is the report."),
#     reply("time", "It is noon."),
#     reply("welcome", "You are welcome."),
# ])
# .matching_requests();
# let rt = tokio::runtime::Builder::new_current_thread().build().unwrap();
# rt.block_on(async {
let mut main = Thread::new(transport.clone(), "thread-1");
// A second thread on the same conversation, holding the same messages.
let mut side = Thread::<_>::restore(transport.clone(), main.snapshot())?;

// The long run starts, and borrows `main` until its stream ends ...
let mut long = main.send("Write the report")?;
long.next().await;
// ... so the quick question goes to `side`, and is answered meanwhile.
let quick = side.send("What time is it?")?.collect_report().await;
assert_eq!(quick.end, RunEnd::Success { result: None });
// Both can be drawn as one list at any moment.
let on_screen = merge_messages(long.thread().messages(), side.messages());
assert_eq!(on_screen.len(), 4);
long.collect_report().await;

// Both streams have ended: `main` takes the messages it lacks ...
let merged = main.merge_messages_from(&side)?;
assert_eq!(merged.added.len(), 2);
assert!(merged.differing.is_empty());
// ... and its next request carries both runs.
main.send("Thanks")?.collect_report().await;
assert_eq!(transport.last_request().unwrap().messages.len(), 5);
# Ok::<(), ag_ui::client::Error>(())
# })
# .unwrap();
```

병합은 메시지 ID를 기준으로 하고, 서로 다른 내용은 `MergeReport::differing`에
보고합니다. 앱은 공유 state를 별도로 조정합니다. 응답하지 않은 tool call,
pending interrupt, 미확정 resume 제출은 각 thread에서 먼저 정리합니다. 실행 중에
복제한 thread는 열린 call을 `abandon_tool_call`로 정리할 수 있습니다. 원본에 실제
결과가 도착했다면 복제본을 원본에 병합할 때 임시 결과는 제외되며
`MergeReport::superseded`에 나옵니다.
