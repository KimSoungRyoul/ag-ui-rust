---
title: Client 도구와 결과 전달
description: Client 기능을 설명하고 애플리케이션이 실행한 결과를 전달합니다.
---

Client의 실행 가능한 함수는 애플리케이션에 이미 등록되어 있어야 합니다.
`Tool`은 그 기능의 설명을 서버에 알리는 데이터입니다. 아래 코드는 함수를 등록하거나 실행하지 않습니다.

## 사용할 수 있는 기능 알리기

```rust
use ag_ui::{RunAgentInput, Tool};
use serde_json::json;

let tool = Tool::new("open_settings_panel", "Open application settings", json!({
    "type": "object",
    "properties": { "tab": { "type": "string" } },
    "required": ["tab"],
    "additionalProperties": false
}));
let mut input = RunAgentInput::new("conversation-1", "run-1");
input.tools = vec![tool];
assert_eq!(input.tools[0].name, "open_settings_panel");
```

고수준 API에서는 `thread.set_tools(vec![tool])` 또는 `ThreadBuilder::tools`로 목록을 설정합니다.
서버는 이름·설명·인자 schema를 읽지만 client의 함수 구현은 받지 않습니다.
서버가 자체적으로 실행하는 tool은 client가 제공할 필요가 없습니다.

## 호출을 받아 결과 보내기

1. `RunStream`의 `Update::Message`에서 호출 이름·ID와 인자를 수집합니다. 인자는 `ToolCallEnded`까지 모읍니다.
2. 해당 호출이 client 실행용인지, 이미 서버 결과가 도착했는지 확인합니다. 일반 출력용 tool call을 자동 실행하지 않습니다.
3. 애플리케이션의 등록된 함수와 연결하고 인자 schema·실행 권한을 검사한 뒤 실행합니다.
4. 현재 run을 끝까지 소비하고 drop한 다음, 같은 tool call ID의 결과 message를 `Thread`에 추가합니다.
5. 승인 대기가 없다면 `thread.run()`으로 다음 요청을 보냅니다. 승인 대기가 있으면 [승인 응답](/ag-ui-rust/ko/client/interrupts/) 흐름을 따릅니다.

```rust
use ag_ui::Message;
use ag_ui::client::{Result, Thread, transport::Transport};
use serde_json::Value;

fn record_result<T: Transport>(
    thread: &mut Thread<T>,
    message_id: &str,
    tool_call_id: &str,
    output: &Value,
) -> Result<()> {
    thread.push_message(Message::tool(
        message_id, tool_call_id, serde_json::to_string(output)?,
    ))
}
```

`message_id`에는 새 고유 ID를, `tool_call_id`에는 수신한 호출 ID를 넣습니다.
이 함수는 이미 얻은 결과를 대화에 기록할 뿐입니다. 함수 선택·실행·재시도와 결과의 성공·실패 표현은 애플리케이션이 정합니다.
`Thread`는 tool executor나 자동 model loop를 제공하지 않습니다.

[서버에서 호출 보내기](/ag-ui-rust/ko/server/tools/) · [대화 관리](/ag-ui-rust/ko/client/thread/)
