---
title: Agent에 AG-UI 연결하기
description: Agent 구현부터 HTTP endpoint까지 만들고 컴포넌트별 기능을 추가합니다.
---

이 페이지는 기존 agent의 입출력을 AG-UI에 연결하는 방법을 다룹니다.
모델 loop, 실행 가능한 tool 등록, graph routing과 subagent 실행은 애플리케이션이나 framework에서 구현합니다.
아래 `Greeter`는 연결 방법을 보여 주는 최소 예제입니다.

이 가이드에서는 `http://127.0.0.1:3000/agent`에서 텍스트를 streaming하는 서버를 만듭니다.
먼저 작은 `Agent`를 실행하고, 필요한 컴포넌트를 하나씩 추가합니다. Rust 1.85 이상이 필요합니다.

## 1. 프로젝트와 의존성

```sh
cargo new agent-server
cd agent-server
```

```toml
# Cargo.toml
[dependencies]
ag-ui = { git = "https://github.com/KimSoungRyoul/ag-ui-rust", features = ["axum"] }
axum = "0.8"
tokio = { version = "1", features = ["rt-multi-thread", "macros", "net"] }
```

## 2. Agent 구현과 HTTP 연결

`src/main.rs`를 다음 코드로 바꿉니다. `Agent`에는 업무 로직을,
`RunContext`에는 client로 보낼 출력을 작성합니다. `route_agui`가 이를 HTTP endpoint에 연결합니다.

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

## 3. 실행하고 응답 확인하기

`cargo run`으로 서버를 띄운 뒤, 다른 터미널에서 요청합니다.

```sh
curl -N -X POST http://127.0.0.1:3000/agent \
  -H 'content-type: application/json' \
  -d '{"threadId":"thread-1","runId":"run-1","messages":[],"tools":[],"context":[]}'
```

`RUN_STARTED` → `TEXT_MESSAGE_START` → `TEXT_MESSAGE_CONTENT` → `TEXT_MESSAGE_END`
→ `RUN_FINISHED` 순서로 도착하면 성공입니다. 텍스트 내용은 `Hello from Rust.`입니다.
run의 시작과 종료 event는 SDK가 생성합니다.

## 컴포넌트별 구현 가이드

| 구현할 기능 | 사용할 컴포넌트 | 가이드 |
| --- | --- | --- |
| 모델·업무 로직 연결 | `Agent`, `RunContext`, `RunOutcome` | [Agent와 실행 컨텍스트](/ag-ui-rust/ko/server/agent/) |
| HTTP 공개와 요청 처리 | `RouterExt`, `AgentEndpoint`, `AgUiInput` | [HTTP endpoint](/ag-ui-rust/ko/server/axum/) |
| 응답을 조금씩 전송 | message handle, `delta`, `end` | [텍스트 스트리밍](/ag-ui-rust/ko/server/text/) |
| 도구 호출과 결과 전달 | tool call handle | [도구 호출](/ag-ui-rust/ko/server/tools/) |
| 화면과 공유할 상태 변경 | `State`, snapshot, delta | [공유 상태](/ag-ui-rust/ko/server/state/) |
| 사용자 결정을 기다리고 재개 | `Interrupt`, resume 입력 | [승인 요청과 재개](/ag-ui-rust/ko/server/interrupts/) |
| 하위 agent의 출력 구분 | subagent handle, invocation ID | [하위 agent 표시](/ag-ui-rust/ko/server/subagents/) |
| 실패·연결 해제 처리 | server error, cancellation token | [오류와 실행 중지](/ag-ui-rust/ko/server/errors/) |

## 애플리케이션에 연결하기

예를 들어 task-board 서버는 업무 상태를 읽고, 변경 내용을 state event로 보내고,
사용자 승인이 필요하면 interrupt를 반환합니다. 실제 DB 변경과 권한 확인은 애플리케이션 코드에서 수행합니다.
`Agent` 구현에 기존 서비스나 모델 client를 주입하고, 그 결과를 `RunContext`로 내보내면 됩니다.

[task-board 전체 예제](/ag-ui-rust/ko/examples/task-board/)에서 이 흐름을 확인합니다.
HTTP 없이 검증하려면 [agent 테스트](/ag-ui-rust/ko/design/testing/)를 사용합니다.
이제 [Rust client를 연결](/ag-ui-rust/ko/client/)해 응답을 화면에 표시할 수 있습니다.
