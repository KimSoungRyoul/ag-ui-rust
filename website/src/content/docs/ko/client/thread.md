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

구현 방법은 [전용 컴포넌트 가이드](/ag-ui-rust/ko/client/state/)에서 확인합니다.

## 승인과 거절

구현 방법은 [전용 컴포넌트 가이드](/ag-ui-rust/ko/client/interrupts/)에서 확인합니다.

## 관찰·중지·복원

구현 방법은 [전용 컴포넌트 가이드](/ag-ui-rust/ko/client/interrupts/)에서 확인합니다.
