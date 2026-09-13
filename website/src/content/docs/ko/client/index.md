---
title: Rust에서 Agent 호출하기
description: HTTP 연결부터 대화 기록과 업데이트 처리까지 Rust client를 만듭니다.
---

AG-UI 서버에 연결해 응답을 출력하고 대화 기록을 유지하는 Rust client를 만듭니다.
먼저 [서버 구축 가이드](/ag-ui-rust/ko/server/)의 서버를 실행합니다. 이미 AG-UI endpoint가 있다면 URL을 바꿔 사용합니다.
Rust 1.85 이상과 HTTP client의 TLS 의존성을 빌드할 C toolchain이 필요합니다.

## 1. 프로젝트와 의존성

```sh
cargo new agent-client
cd agent-client
```

```toml
# Cargo.toml
[dependencies]
ag-ui = { git = "https://github.com/KimSoungRyoul/ag-ui-rust", features = ["http"] }
futures-util = "0.3"
tokio = { version = "1", features = ["rt-multi-thread", "macros"] }
```

## 2. 연결하고 업데이트 소비하기

`src/main.rs`를 다음 코드로 바꿉니다. `HttpAgent`가 접속 설정을,
`Thread`가 대화 기록과 상태를, `Update`가 화면에 반영할 변경을 담당합니다.

```rust,no_run
use ag_ui::client::{HttpAgent, MessageChangeKind, RunEnd, Update};
use futures_util::StreamExt;
use std::io::Write;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let agent = HttpAgent::new("http://127.0.0.1:3000/agent")?;
    let mut thread = agent.thread("thread-1");
    {
        let mut run = thread.send("Hello")?;
        while let Some(update) = run.next().await {
            match update {
                Update::Message(message) => {
                    if let MessageChangeKind::Content { delta } = message.change {
                        print!("{delta}");
                        std::io::stdout().flush()?;
                    }
                }
                Update::Error(error) => eprintln!("{error}"),
                Update::Done(RunEnd::Success { .. }) => println!(),
                Update::Done(end) => eprintln!("{end:?}"),
                _ => {}
            }
        }
    }
    println!("{} messages in the thread", thread.messages().len());
    Ok(())
}
```

## 3. 실행 결과 확인하기

서버를 켜 둔 상태에서 `cargo run`을 실행합니다.
`Hello from Rust.`와 `2 messages in the thread`가 출력되면 연결이 완료된 것입니다.
같은 `Thread`로 다시 `send()`하면 앞선 대화를 포함해 다음 요청을 보냅니다.

## 컴포넌트별 구현 가이드

| 구현할 기능 | 사용할 컴포넌트 | 가이드 |
| --- | --- | --- |
| 서버 연결과 대화 유지 | `HttpAgent`, `Thread` | [연결과 대화 관리](/ag-ui-rust/ko/client/thread/) |
| 업데이트·진단·종료 처리 | `RunStream`, `Update`, `RunEnd` | [업데이트 처리](/ag-ui-rust/ko/client/updates/) |
| 메시지·도구·하위 agent 표시 | message ID, tool call ID, subagent registry | [렌더링](/ag-ui-rust/ko/client/rendering/) |
| 공유 상태를 UI에서 읽기 | `Update::State`, `state`, `raw_state` | [공유 상태 읽기](/ag-ui-rust/ko/client/state/) |
| 승인·거절과 재개 | `resume_many`, `decline`, snapshot | [승인 응답과 복원](/ag-ui-rust/ko/client/interrupts/) |
| HTTP 교체 또는 테스트 | `Transport`, `ReplayTransport` | [Transport 선택](/ag-ui-rust/ko/client/transports/) |

[Client 도구와 결과 전달](/ag-ui-rust/ko/client/tools/)에서 기존 client 함수를 AG-UI 호출에 연결합니다.

## Client 애플리케이션이 맡을 일

SDK는 event를 메시지와 상태로 조립합니다. 애플리케이션은 `Update`를 UI 컴포넌트에 연결하고,
인증 정보와 snapshot 저장 위치를 정합니다. 같은 thread ID로 새 객체를 만들어도 서버 기록을 자동으로 불러오지는 않습니다.

이 가이드는 **Rust로 client를 만드는 경우**를 다룹니다. 브라우저의 TypeScript client를 쓴다면
[공식 TypeScript SDK](https://docs.ag-ui.com/sdk/js/client/overview)와
[review-desk 예제](/ag-ui-rust/ko/examples/review-desk/)를 함께 참고합니다.
Rust CLI 구현은 [board-watch](/ag-ui-rust/ko/examples/board-watch/)에서 확인합니다.
