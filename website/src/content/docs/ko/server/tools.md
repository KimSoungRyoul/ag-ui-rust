---
title: 도구 호출·결과 전달
description: 이미 구현된 도구의 설명·호출·결과를 AG-UI로 전달합니다.
---

실행 가능한 tool은 애플리케이션이나 framework에 이미 구현되어 있다고 가정합니다.
이 SDK는 tool 선택이나 실행을 자동화하지 않습니다. `ctx.tool_call(name)`은 해당 이름의 호출 event를 열고,
`args_json()`은 인자를, `result_json()`은 애플리케이션이 얻은 결과를 전달합니다.

## Tool 설명과 실행 함수

`ag_ui::Tool`은 이름·설명·인자 JSON Schema입니다. 실행 함수나 인증 정보가 들어 있지 않습니다.
client가 제공하는 기능은 `RunAgentInput.tools`로 서버에 전달합니다.
서버는 `ctx.tools()`와 `ctx.tool(name)`으로 이 설명을 읽고, 필요하면 자기 모델의 tool 형식으로 변환합니다.
함수 등록·인자 검증·권한 확인·실제 실행은 실행 담당 애플리케이션에서 처리합니다.

예를 들어 날씨 도구가 이미 있다면, framework가 호출을 결정하고 날씨 API를 실행합니다.
AG-UI는 그 과정의 이름·인자·결과를 UI에 보냅니다. 아래 예제의 날씨 결과는 고정된 예시 데이터입니다.

## 서버에서 실행한 결과 전달

```rust
use ag_ui::{Event, EventType, RunAgentInput};
use ag_ui::server::RunContext;
use serde_json::json;

fn main() -> ag_ui::server::Result<()> {
    let (mut ctx, mut events) = RunContext::<()>::new(RunAgentInput::new("t", "r"))?;

    let mut call = ctx.tool_call("get_weather")?;
    call.args_json(&json!({"city": "Seoul"}))?;
    // tool이 실제로 하는 일이 여기에 들어갑니다.
    let result_id = call.result_json(&json!({"tempC": 21}))?;

    assert_eq!(result_id.as_str(), "r-msg-1");
    let types: Vec<EventType> = events.drain().iter().map(Event::event_type).collect();
    assert_eq!(
        types,
        [
            EventType::ToolCallStart,
            EventType::ToolCallArgs,
            EventType::ToolCallEnd,
            EventType::ToolCallResult,
        ]
    );
    Ok(())
}
```

`TOOL_CALL_START` → `TOOL_CALL_ARGS` → `TOOL_CALL_END` → `TOOL_CALL_RESULT` 순서로 전송합니다.
`TOOL_CALL_END`는 인자 전송의 끝이며, 업무 실행의 성공을 뜻하지 않습니다.
`result_json`은 end와 result event를 보내고 결과 message ID를 반환합니다.
`result()`는 이미 직렬화한 문자열을 받습니다.

## Client에서 실행할 호출 전달

```rust
use ag_ui::{Event, EventType, RunAgentInput};
use ag_ui::server::RunContext;
use serde_json::json;

fn main() -> ag_ui::server::Result<()> {
    let (mut ctx, mut events) = RunContext::<()>::new(RunAgentInput::new("t", "r"))?;

    let mut call = ctx.tool_call("open_settings_panel")?;
    call.args_json(&json!({"tab": "billing"}))?;
    call.end()?;

    let types: Vec<EventType> = events.drain().iter().map(Event::event_type).collect();
    assert_eq!(
        types,
        [
            EventType::ToolCallStart,
            EventType::ToolCallArgs,
            EventType::ToolCallEnd,
        ]
    );
    Ok(())
}
```

`end()`로 인자 전송을 마칩니다. Client 애플리케이션은 자신이 제공한 tool인지 확인하고 실행한 뒤,
같은 tool call ID의 결과 message를 다음 요청에 담습니다. SDK가 UI 함수나 외부 API를 대신 호출하지 않습니다.
서버가 이미 결과를 보낸 호출은 표시용일 수 있으므로 client가 다시 실행하면 안 됩니다.

`RunAgentInput.tools`는 실행 권한을 증명하지 않으며, 서버 도구 전체의 allow-list도 아닙니다.
서버는 client가 제공하지 않은 자기 도구의 호출과 결과도 보고할 수 있습니다.
`task-board`가 일부 이름을 요청 목록과 대조하는 것은 해당 예제의 정책입니다. 도구 실행 자체도 예제 코드가 맡습니다.

## 인자가 나누어 도착할 때

```rust
use ag_ui::RunAgentInput;
use ag_ui::server::RunContext;
use serde::Deserialize;

#[derive(Deserialize)]
struct Query {
    city: String,
}

fn main() -> ag_ui::server::Result<()> {
    let (mut ctx, _events) = RunContext::<()>::new(RunAgentInput::new("t", "r"))?;

    let mut call = ctx.tool_call("get_weather")?;
    call.args(r#"{"city":"#)?;       // provider가 흘려보내는 그대로
    call.args(r#""Seoul"}"#)?;

    assert_eq!(call.raw_args(), r#"{"city":"Seoul"}"#);
    let query: Query = call.parse_args()?;
    assert_eq!(query.city, "Seoul");

    call.result(r#"{"tempC":21}"#)?;
    Ok(())
}
```

`args()`는 JSON 문자열의 일부를 받습니다. 모든 조각을 받은 후 `parse_args()`로 해석합니다.
도중의 조각 하나만으로 JSON을 해석하거나 도구를 실행하지 않습니다.

## 실행 중 상태 전달

```rust
use ag_ui::RunAgentInput;
use ag_ui::server::RunContext;
use serde::{Deserialize, Serialize};
use serde_json::json;

#[derive(Default, Serialize, Deserialize)]
struct Board {
    tasks: Vec<String>,
}

fn main() -> ag_ui::server::Result<()> {
    let (mut ctx, mut events) = RunContext::<Board>::new(RunAgentInput::new("t", "r"))?;

    let mut call = ctx.tool_call("add_task")?;
    call.args_json(&json!({"title": "ship it"}))?;

    call.state_mut().tasks.push("ship it".to_owned());
    call.publish_state()?;               // call이 열린 채로 STATE_SNAPSHOT

    call.result_json(&json!({"ok": true}))?;

    assert_eq!(ctx.state().tasks, ["ship it"]);
    // START, ARGS, STATE_SNAPSHOT, END, RESULT.
    assert_eq!(events.drain().len(), 5);
    Ok(())
}
```

`state_mut()`와 `publish_state()`는 진행 상황을 공유하는 API입니다. DB 변경이나 tool 실행을 대신하지 않습니다.
state event의 위치만으로 어느 tool의 상태인지 추정하지 않습니다.

## 동시에 도착하는 호출

```rust
use ag_ui::{Event, EventType, RunAgentInput};
use ag_ui::server::RunContext;
use std::collections::BTreeMap;

fn main() -> ag_ui::server::Result<()> {
    let (mut ctx, mut events) = RunContext::<()>::new(RunAgentInput::new("t", "r"))?;

    let names = ["get_weather", "roll_dice"];
    // provider가 흘려보낸 것: 두 개의 call이 서로 엇갈려 있습니다.
    let streamed = [
        (0, r#"{"city":"#),
        (1, r#"{"sides":"#),
        (0, r#""Seoul"}"#),
        (1, "20}"),
    ];

    let mut buffered: BTreeMap<usize, String> = BTreeMap::new();
    for (call, fragment) in streamed {
        buffered.entry(call).or_default().push_str(fragment);
    }

    for (call, args) in buffered {
        let mut handle = ctx.tool_call(names[call])?;
        handle.args(&args)?;
        handle.end()?;
    }

    let types: Vec<EventType> = events.drain().iter().map(Event::event_type).collect();
    assert_eq!(types.len(), 6);
    assert_eq!(types[0], EventType::ToolCallStart);
    Ok(())
}
```

단일 context에서 tool handle 두 개를 동시에 유지할 수는 없습니다. 이 예제는 호출 ID별로 인자를 모아 순서대로 전달합니다.
실시간 interleaving이 필요하면 `ctx.emit`으로 ID가 명시된 event를 전달합니다. 서로 다른 ID의 호출은 겹칠 수 있습니다.
이 SDK가 병렬 tool 실행을 스케줄링하는 것은 아닙니다.

## 다음 단계

- [Client에서 도구 결과 보내기](/ag-ui-rust/ko/client/tools/)
- [공유 상태](/ag-ui-rust/ko/server/state/)와 [AG-UI 연결 개요](/ag-ui-rust/ko/server/)
- [`Tool`](/ag-ui-rust/api/ag_ui/tool/struct.Tool.html), [`ToolCallHandle`](/ag-ui-rust/api/ag_ui/server/emit/struct.ToolCallHandle.html)
