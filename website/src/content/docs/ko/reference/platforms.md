---
title: 플랫폼과 Rust 버전
description: Native·wasm과 executor 의존성의 지원 범위를 확인합니다.
---

최소 Rust 버전은 **1.85**, edition은 **2024**입니다.
CI의 `msrv` job이 workspace의 모든 feature와 target을 해당 버전으로 검사합니다.

## 지원 범위

| 구성 | 실행 환경 | 확인 범위 |
| --- | --- | --- |
| Protocol type, `server`, `client` | native, custom executor | unit·integration·doctest |
| `axum` | Tokio 기반 native HTTP 서버 | 실제 HTTP 테스트 |
| `http` | reqwest 기반 native HTTP client | 실제 HTTP 테스트 |
| Protocol, `server`, `client`, A2UI | `wasm32-unknown-unknown` | 지정 feature 조합의 컴파일과 의존성 검사 |

Wasm 컴파일 성공은 브라우저에서 Rust SDK를 실행했다는 뜻은 아닙니다.
브라우저 transport는 제공하지 않으므로 직접 구현해야 합니다. Review Desk의 TypeScript renderer 검증은 별도 검사입니다.

## Executor와 transport 분리

`server`와 `client`만 활성화한 일반 의존성에는 Tokio가 없습니다.
`axum`이나 `http`를 켜면 Tokio가 포함됩니다. `http`는 opt-in입니다.

```rust
use ag_ui::client::transport::{Transport, TransportFuture, boxed_stream};
use ag_ui::client::Result;
use ag_ui::{Event, RunAgentInput};
use futures_util::stream;

/// 정해진 script를 재생합니다. `fetch`와 `EventSource` 위에 세운
/// browser transport도 같은 모양입니다.
struct Canned(Vec<Event>);

impl Transport for Canned {
    // 연결 실패는 future가 내는 error입니다. stream 도중의 실패는 stream 안의
    // error item입니다. 그 구분이 interface의 전부입니다.
    fn run(&self, _input: RunAgentInput) -> TransportFuture {
        let events: Vec<Result<Event>> = self.0.iter().cloned().map(Ok).collect();
        Box::pin(async move { Ok(boxed_stream(stream::iter(events))) })
    }
}
```

위 코드는 event를 재생하는 transport 예제입니다. 브라우저에서는 POST 요청과
streaming 응답을 처리하는 구현으로 교체합니다. `EventStream`과 `TransportFuture`는
native에서 `Send`를 요구하고 wasm에서는 그 bound를 제외합니다.

## 직접 확인하기

```sh
cargo check -p ag-ui --target wasm32-unknown-unknown --no-default-features --features client,sse
cargo check -p ag-ui-a2ui --target wasm32-unknown-unknown --all-features
cargo tree -p ag-ui --no-default-features --features server -e normal
cargo tree -p ag-ui --no-default-features --features client -e normal
```

먼저 `rustup target add wasm32-unknown-unknown`으로 target을 설치합니다.
CI는 컴파일 검사와 별개로 일반 의존성 그래프에 Tokio가 없는지도 확인합니다.
Dev dependency와 HTTP 구성을 이 보장에 포함하지 않습니다.
전체 구성은 [Feature 선택](/ag-ui-rust/ko/reference/features/)을 참고합니다.
