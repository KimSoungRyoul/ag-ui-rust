---
title: Read shared state
description: Read server state through a typed view and handle invalid updates.
---

When the server sends a snapshot or delta, `Thread` applies it to shared state.
Use `Update::State` to refresh the view with the latest value.

## Typed state

`state()` returns `Result<&S, StateViewError>` for current raw state. Invalid local setters
leave both views unchanged. If remote JSON does not fit `S`, raw state advances and the typed
cache is cleared: an old value is never presented as current.

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

## Refresh the view

`thread.raw_state()` returns JSON; `thread.state()?` returns the application type.
Events attributed to subagents still update the same shared state.
An invalid JSON Patch preserves the previous raw and typed state.

[Send state from the server](/ag-ui-rust/server/state/) · [Rendering guide](/ag-ui-rust/client/rendering/)
