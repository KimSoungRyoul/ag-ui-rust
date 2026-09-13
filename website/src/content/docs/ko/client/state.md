---
title: 공유 상태 읽기
description: 서버 상태를 typed view로 읽고 잘못된 상태 업데이트를 처리합니다.
---

서버에서 snapshot이나 delta가 도착하면 `Thread`가 상태를 반영합니다.
UI는 `Update::State`로 변경을 감지하고 최신 값을 읽습니다.

## Typed state

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

## 상태를 화면에 반영하기

`thread.raw_state()`는 JSON, `thread.state()?`는 애플리케이션 타입을 반환합니다.
subagent 출처가 붙은 event도 같은 공유 상태를 갱신합니다.
잘못된 JSON Patch는 이전 raw·typed 상태를 유지합니다.

[서버에서 상태 보내기](/ag-ui-rust/ko/server/state/) · [렌더링 가이드](/ag-ui-rust/ko/client/rendering/)
