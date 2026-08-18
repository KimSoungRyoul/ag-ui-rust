---
title: tool call
description: agent에서 tool call을 emit하는 법, run 안에서 직접 답하는 법, 그리고 client가 제공한 tool 목록을 읽는 법.
---

tool call은 message와 마찬가지로 앞뒤가 감싸입니다. `TOOL_CALL_START`, 여러 개의
`TOOL_CALL_ARGS`, 그리고 `TOOL_CALL_END`가 모두 같은 call id를 싣습니다.
`ctx.tool_call(name)`은 시작을 emit하고 handle을 돌려줍니다. handle은 `Drop`에서 끝을
emit합니다. [message handle](/ag-ui-rust/ko/server/text/)과 똑같습니다.

다른 것은 끝나는 방식입니다. agent가 tool을 직접 실행하고 결과를 보고할 수도 있습니다. 아니면
열어 둔 채로 client에게 넘길 수도 있습니다. client가 실행하고 다음 요청에 결과를 실어
보냅니다.

## agent가 직접 답하는 call

```rust
use ag_ui_core::{Event, EventType, RunAgentInput};
use ag_ui_server::RunContext;
use serde_json::json;

fn main() -> ag_ui_server::Result<()> {
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

`result`는 `TOOL_CALL_END`를 emit한 다음 `TOOL_CALL_RESULT`를 emit합니다. 그리고 결과를 실어
나르는 tool message의 id를 돌려줍니다. 대화 기록이 그 결과에 쓸 id입니다.

그 id는 결과를 emit할 때가 아니라 handle이 만들어질 때 배정됩니다. 덕분에 handle은 run
context에 다시 손을 뻗지 않고도 call을 마무리할 수 있습니다. 미리 알아야 하면
`result_message_id()`가 읽어 줍니다.

`result_json`은 직렬화까지 해 줍니다. `result`는 이미 손에 있는 `String`을 받습니다.

## client가 실행하는 call

frontend tool은 `end()`로 닫고 끝입니다. client가 실행할 수 있어서 client가 제공한 tool
말입니다. 여기서 보고할 결과는 없습니다. 결과는 다음 요청에 tool message로 도착합니다.

```rust
use ag_ui_core::{Event, EventType, RunAgentInput};
use ag_ui_server::RunContext;
use serde_json::json;

fn main() -> ag_ui_server::Result<()> {
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

## 인자는 텍스트로 streaming됩니다

`args`는 값이 아니라 조각을 받습니다. provider가 인자를 그렇게 내보내기 때문입니다. 부분
delta는 대개 올바른 JSON이 아닙니다. 프로토콜이 `TOOL_CALL_ARGS`를 파싱하지 않은 채 두는
이유도 그것입니다.

handle은 자기가 emit한 것을 모두 간직합니다. 그래서 provider가 다 보내고 나면 `parse_args`가
실행에 쓸 완성된 구조체를 건네줍니다.

```rust
use ag_ui_core::RunAgentInput;
use ag_ui_server::RunContext;
use serde::Deserialize;

#[derive(Deserialize)]
struct Query {
    city: String,
}

fn main() -> ag_ui_server::Result<()> {
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

인자가 아직 부분일 때 `parse_args`는 실패합니다. 그것이 요점입니다. 조각마다 부르지 말고
stream이 끝난 다음에 한 번 부르십시오. `raw_args`는 같은 버퍼를 파싱하지 않은 채로 줍니다.

## 제공된 tool 목록은 능력 목록입니다

`RunAgentInput.tools`는 **client**가 무엇을 실행할 수 있는지를 말합니다. agent가 무엇을
호출해도 되는지는 말하지 않습니다. 이 SDK의 어떤 부분도 그것을 허용 목록으로 취급하지
않습니다. 그 목록에 없는 이름으로 `TOOL_CALL_START`를 emit해도 온전한 stream입니다.
[ordering verifier](/ag-ui-rust/ko/server/errors/)는 그에 대해 아무 말도 하지 않습니다.

이 문제를 결론짓는 사례는 agent가 직접 답하는 tool입니다. A2UI agent는 surface를 frontend로
실어 보내려고 `render_a2ui`를 emit합니다. 그리는 쪽은 frontend입니다. client가 실행할 것이
없으니 어떤 client도 그 tool을 "제공"한 적이 없습니다. server 쪽 tool도 같은 모양입니다. run
안에서 agent가 결과를 계산해 버리는 경우입니다. agent가 무엇을 했는지 기록에 남기려고 emit하는
call도 마찬가지입니다.

알지 못하는 call을 받았을 때 무엇을 할지는 client가 정합니다. 무시하든, activity로 그리든,
보고하든 말입니다. 프로토콜이 제약하는 것은 *ordering*입니다. 시작 없는 인자, 끝나기 전에 온
결과 같은 것들입니다. 검사되는 것도 그것뿐입니다.

더 엄격한 규칙을 원하는 agent는 그렇게 할 수 있습니다. `ctx.tool(name)`이 제공되지 않은 이름에
`None`을 돌려주기 때문입니다.

```rust
use ag_ui_core::{RunAgentInput, RunOutcome, Tool};
use ag_ui_server::{Agent, Error, Result, RunContext, ToolCallHandle};
use serde_json::json;

/// client가 제공한 tool일 때만 call을 엽니다. client가 실행해 주기를 기대하는
/// tool에 대해 이 agent가 스스로 채택한 규칙입니다. 프로토콜이 강제하는
/// 규칙이 아닙니다.
fn offered<'a>(ctx: &'a mut RunContext<()>, name: &str) -> Result<ToolCallHandle<'a, ()>> {
    if ctx.tool(name).is_none() {
        return Err(Error::agent(format!("the client offered no {name} tool")));
    }
    ctx.tool_call(name)
}

struct Board;

impl Agent for Board {
    type State = ();

    async fn run(&self, ctx: &mut RunContext<()>) -> Result<RunOutcome> {
        let mut call = offered(ctx, "add_task")?;
        call.args_json(&json!({"title": "ship it"}))?;
        call.result_json(&json!({"ok": true}))?;

        Ok(RunOutcome::Success)
    }
}

fn main() -> ag_ui_server::Result<()> {
    let mut input = RunAgentInput::new("t", "r");
    input.tools = vec![Tool::new("add_task", "Add a task to the board.", json!({}))];
    let (mut ctx, _events) = RunContext::<()>::new(input)?;

    assert!(offered(&mut ctx, "add_task").is_ok());
    assert!(offered(&mut ctx, "delete_everything").is_err());
    Ok(())
}
```

`examples/task-board`가 바로 이렇게 합니다. client를 대신해 보드를 움직이는 네 개의 tool에
대해서 말입니다. `render_a2ui`에 대해서는 *하지 않습니다*.

## call이 열려 있는 동안 작업하기

handle은 run context가 아니라 그 run의 event sink와 상태를 빌립니다. 그래서 tool이 실제로 하는
일은 인자와 결과 *사이에* 놓입니다.

```rust
use ag_ui_core::RunAgentInput;
use ag_ui_server::RunContext;
use serde::{Deserialize, Serialize};
use serde_json::json;

#[derive(Default, Serialize, Deserialize)]
struct Board {
    tasks: Vec<String>,
}

fn main() -> ag_ui_server::Result<()> {
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

call이 진행 중이고, 상태가 바뀌고, 결과가 그것을 닫습니다. 이 순서야말로 call을 streaming하는
이유입니다. 이미 끝난 call을 한 번에 알리는 대신 말입니다. 프로토콜이 왜 이를 허용하는지,
그리고 순서를 왜 신경 쓸 만한지는 [shared state](/ag-ui-rust/ko/server/state/)에서 다룹니다.

## 병렬 call

`ToolCallHandle` 두 개를 동시에 여는 것은 borrow check 오류입니다. 의도된 것이고, message 두
개가 그런 것과 같은 이유입니다. 그래서 `args(a) args(b) args(a) end(a) end(b)`처럼 흘려보내는
provider는 call 하나에 handle 하나로 그대로 옮길 수 없습니다.

통하는 방식은 이렇습니다. call마다 인자를 모아 두었다가, 인자가 완성되면 통째로 emit합니다. 두
call의 인자를 서로 뒤섞지 않는 유일한 방식이기도 합니다.

```rust
use ag_ui_core::{Event, EventType, RunAgentInput};
use ag_ui_server::RunContext;
use std::collections::BTreeMap;

fn main() -> ag_ui_server::Result<()> {
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

`e2e/src/llm.rs`가 실제 provider의 stream을 이렇게 옮깁니다. wire에서의 엇갈림이 정말로
필요할 때도 있습니다. client가 두 call을 도착하는 대로 그려야 하는 경우입니다. 그럴 때는
`ctx.emit`으로 직접 emit하십시오. verifier는 모든 것을 id로 구분하므로 엇갈린 stream도
받아들입니다. 허락하지 않는 것은 연 적 없는 call을 닫는 일입니다.

## API

- [`RunContext::tool_call`](/ag-ui-rust/api/ag_ui_server/struct.RunContext.html#method.tool_call)과
  [`tool_call_with_id`](/ag-ui-rust/api/ag_ui_server/struct.RunContext.html#method.tool_call_with_id)
- [`RunContext::tools`](/ag-ui-rust/api/ag_ui_server/struct.RunContext.html#method.tools)와
  [`tool`](/ag-ui-rust/api/ag_ui_server/struct.RunContext.html#method.tool)
- [`ag_ui_server::ToolCallHandle`](/ag-ui-rust/api/ag_ui_server/struct.ToolCallHandle.html)
- [`ag_ui_core::Tool`](/ag-ui-rust/api/ag_ui_core/struct.Tool.html)과
  [`ToolCall`](/ag-ui-rust/api/ag_ui_core/struct.ToolCall.html)
