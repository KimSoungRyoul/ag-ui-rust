---
title: shared state
description: agent의 상태를 client에 내보내는 법. 그리고 snapshot과 JSON Patch delta 중 무엇을 고르는지.
---

AG-UI run에는 client가 함께 비추어 보는 shared state가 실려 다닙니다. agent가 편집하는 보드,
채워 넣는 폼, 초안을 잡는 문서 같은 것들입니다. client는 자기 사본을 `RunAgentInput.state`로
보냅니다. agent가 그것을 바꿉니다. 모든 변경은 `STATE_SNAPSHOT`이나 `STATE_DELTA`가 되어
되돌아 나갑니다.

server 쪽에서 그 상태는 타입이 붙은 값입니다. `Agent::State`, 즉 여러분의 구조체입니다. 어느
event로 나갈지는 알아서 정해집니다.

## 읽고 쓰기

```rust
use ag_ui::{Event, EventType, RunAgentInput};
use ag_ui::serve::RunContext;
use serde::{Deserialize, Serialize};

#[derive(Default, Serialize, Deserialize)]
struct Doc {
    step: u32,
    notes: Vec<String>,
}

fn main() -> ag_ui::serve::Result<()> {
    let (mut ctx, mut events) = RunContext::<Doc>::new(RunAgentInput::new("t", "r"))?;

    // 한 번의 호출로 바꾸고 내보냅니다.
    ctx.update_state(|doc| {
        doc.step = 1;
        doc.notes.push("the document the user is editing".repeat(8));
    })?;

    // 또는 지금 바꾸어 두고, 준비되었을 때 내보냅니다.
    ctx.state_mut().step = 2;
    ctx.publish_state()?;

    // 할 말이 없으면 아무것도 emit하지 않습니다.
    ctx.publish_state()?;

    let types: Vec<EventType> = events.drain().iter().map(Event::event_type).collect();
    assert_eq!(types, [EventType::StateSnapshot, EventType::StateDelta]);
    assert_eq!(ctx.state().step, 2);
    Ok(())
}
```

메서드는 다섯 개입니다. 그중 대부분을 가르는 것은 event가 언제 나가느냐뿐입니다.

| 메서드 | 하는 일 |
| --- | --- |
| `state()` | 마지막으로 내보낸 시점 기준의, 타입이 붙은 상태 |
| `state_mut()` | 타입이 붙은 상태, 가변으로. 아무것도 emit하지 않습니다 |
| `publish_state()` | `state_mut`이 남긴 것을 내보냅니다. 바뀐 것이 없으면 아무 일도 하지 않습니다 |
| `update_state(\|s\| …)` | 한 번의 호출로 바꾸고 내보냅니다 |
| `set_state(&s)` | 값 전체를 갈아 끼우고 내보냅니다 |

## snapshot이냐 delta냐

필드 하나가 늘어났을 뿐인 큰 문서를 매번 통째로 보내는 것은 낭비입니다. 통째로 바뀐 작은
문서에 patch를 보내는 것도 낭비입니다. 내보낼 때마다 정합니다.

1. run의 첫 발행은 언제나 `STATE_SNAPSHOT`입니다. client의 사본은 이미 어긋나 있을 수 있고,
   알 수 없는 기준점에 대한 patch는 적용할 수 없습니다.
2. 그 뒤로는 마지막으로 보낸 snapshot과 상태를
   [RFC 6902](https://datatracker.ietf.org/doc/html/rfc6902)로 비교해 `STATE_DELTA`로
   보냅니다.
3. 단, 직렬화한 patch가 직렬화한 snapshot보다 작지 않으면 대신 snapshot을 보냅니다.

위의 run이 snapshot 하나와 delta 하나를 낸 이유가 이것입니다. 두 번째 변경은 작은 필드 하나만
건드렸습니다. `notes`가 크기를 좌우하는 값에서 말입니다.

`StateManager`는 이 논리를 따로 떼어 낸 것입니다. run 바깥에서 이 판단이 필요한 transport나
테스트를 위한 것입니다.

```rust
use ag_ui::PatchOperation;
use ag_ui::serve::{StateManager, StatePublish};
use serde_json::json;

fn main() -> ag_ui::serve::Result<()> {
    let mut states = StateManager::new();
    let notes = "the document the user is editing, at some length";

    // 첫 발행: 크기와 상관없이 snapshot.
    let first = states.publish(json!({"step": 1, "notes": notes}))?;
    assert!(matches!(first, StatePublish::Snapshot(_)));

    // 큰 문서의 필드 하나: 더 작으므로 patch.
    assert_eq!(
        states.publish(json!({"step": 2, "notes": notes}))?,
        StatePublish::Delta(vec![PatchOperation::replace("/step", 2)])
    );

    // 움직인 것이 없음: 보낼 것도 없음.
    assert_eq!(
        states.publish(json!({"step": 2, "notes": notes}))?,
        StatePublish::Unchanged
    );

    // 작은 문서가 통째로 바뀜: 다시 snapshot. patch가 그것이 설명하는
    // 상태보다 커지기 때문입니다.
    let mut small = StateManager::new();
    small.publish(json!({"a": 1}))?;
    assert_eq!(
        small.publish(json!({"b": 2}))?,
        StatePublish::Snapshot(json!({"b": 2}))
    );
    Ok(())
}
```

`reset()`은 마지막 발행을 잊습니다. 그래서 다음 발행이 다시 snapshot이 됩니다.
`STATE_SNAPSHOT`을 손으로 emit한 뒤에 필요합니다. client의 사본이 어떤 상태인지 더 이상 알 수
없는 재접속 뒤에도 그렇습니다.

## 무엇이 들어오고 무엇이 들어오지 않는가

agent가 들고 시작하는 상태는 `RunAgentInput.state`를 `S`로 역직렬화한 것입니다. `null`이나 빈
객체는 역직렬화 오류가 아닙니다. client가 "아직 상태 없음"으로 보내는 값이고, `S::default()`가
됩니다. 그래서 상태 없는 agent도 모든 client를 상대할 수 있습니다.

상태가 있기는 한데 `S`에 맞지 않으면 오류입니다. run driver가 context를 넘겨주기 *전에*
디코딩합니다. 그래서 이것은 panic이 아니라 `RUN_ERROR`로 client에 도착합니다.

```rust
use ag_ui::RunAgentInput;
use ag_ui::serve::{Error, RunContext};
use serde::{Deserialize, Serialize};
use serde_json::json;

#[derive(Debug, Default, Serialize, Deserialize)]
struct Counter {
    clicks: u32,
}

fn main() {
    // 아직 상태 없음: 실패가 아니라 기본값.
    let mut input = RunAgentInput::new("t", "r");
    input.state = json!({});
    let (ctx, _events) = RunContext::<Counter>::new(input).expect("an empty state is fine");
    assert_eq!(ctx.state().clicks, 0);

    // 맞지 않는 상태.
    let mut input = RunAgentInput::new("t", "r");
    input.state = json!({"clicks": "three"});
    let error = RunContext::<Counter>::new(input).expect_err("should not decode");
    assert!(matches!(error, Error::Json(_)));
    assert_eq!(error.code(), "SERIALIZATION");
}
```

## 무언가 열려 있는 동안 내보내기

message handle과 tool call handle은 run context 자체를 빌리지 않습니다. 그 *필드* 두 개를
빌립니다. event sink와 상태입니다. 그래서 call이 열려 있는 내내 상태에 손이 닿습니다. tool은
자기를 알리고, 일하고, 그런 다음에야 보고할 수 있습니다.

```rust
use ag_ui::{Event, EventType, RunAgentInput};
use ag_ui::serve::RunContext;
use serde::{Deserialize, Serialize};
use serde_json::json;

#[derive(Default, Serialize, Deserialize)]
struct Board {
    tasks: Vec<String>,
}

fn main() -> ag_ui::serve::Result<()> {
    let (mut ctx, mut events) = RunContext::<Board>::new(RunAgentInput::new("t", "r"))?;

    let mut call = ctx.tool_call("add_task")?;
    call.args_json(&json!({"title": "ship it"}))?;

    call.state_mut().tasks.push("ship it".to_owned());
    call.publish_state()?;

    call.result_json(&json!({"ok": true}))?;

    let types: Vec<EventType> = events.drain().iter().map(Event::event_type).collect();
    assert_eq!(
        types,
        [
            EventType::ToolCallStart,
            EventType::ToolCallArgs,
            // call이 감싼 구간 안에서 보드가 움직입니다.
            EventType::StateSnapshot,
            EventType::ToolCallEnd,
            EventType::ToolCallResult,
        ]
    );
    Ok(())
}
```

이것이 적법한 이유는 `STATE_*` 계열이 wire에서 **순서가 없기** 때문입니다. state event는 어느
구간에도 속하지 않고 run 중 어디에나 나타날 수 있습니다. server의 ordering verifier도 그렇게
보고, client의 applier도 그렇게 봅니다.

이 점을 굳이 고집하는 까닭이 있습니다. 대안이 같은 event를 더 나쁜 순서로 내놓기 때문입니다.
이 crate의 이전 초안은 handle에 event sink만 주었습니다. 그래서 무언가 열려 있는 동안에는
상태에 손이 닿지 않았습니다. 모든 agent는 call을 알리기 *전에* 그 call이 바꿀 상태를 먼저
바꾸어야 했습니다. 같은 다섯 event인데 순서가 뒤집힌 것입니다.

그리고 그 순서가 결정합니다. client가 call이 반영되는 과정을 지켜볼 수 있느냐, 아니면 이미
끝난 결과만 보느냐를 말입니다. 상태를 sink 옆에 함께 쥐여 주면 handle이 *닿을 수 있는* 범위만
넓어집니다. *열 수 있는* 범위는 넓어지지 않습니다. 그 뒤에는 여전히 두 번째 블록을 열 run
context가 없습니다.

:::note
`STATE_*`는 순서가 없습니다. 그래서 client는 위치만 보고 어떤 상태 변경이 *어느* tool call에
속하는지 알 수 없습니다. 그 연결이 UI에 중요하다면 끼어든 위치에 기대지 마십시오. call의
결과나 상태 자체에 담으십시오.
:::

## run마다 한 번이 아니라 변경마다 한 번

`examples/task-board`는 작업이 추가될 때마다 한 번씩 내보냅니다. 그래서 작업 두 개를 추가하는
message 하나에서 server는 인코딩을 두 번 고릅니다. 첫 발행은 snapshot입니다. 두 번째는 patch가
보드 전체보다 작게 나올 때만 delta입니다.

상태를 함께 비추어 보는 client는 둘 다 견뎌야 합니다. 예제의 `tests/flows.rs`가 둘 다 못 박아
둡니다. 모든 변경을 run 끝의 발행 하나로 몰아 담으면 event는 줄지만 경험은 나빠집니다. client는
run이 끝날 때까지 아무것도 보지 못합니다.

## API

- [`RunContext::state`](/ag-ui-rust/api/ag_ui/serve/struct.RunContext.html#method.state),
  [`state_mut`](/ag-ui-rust/api/ag_ui/serve/struct.RunContext.html#method.state_mut),
  [`publish_state`](/ag-ui-rust/api/ag_ui/serve/struct.RunContext.html#method.publish_state),
  [`update_state`](/ag-ui-rust/api/ag_ui/serve/struct.RunContext.html#method.update_state),
  [`set_state`](/ag-ui-rust/api/ag_ui/serve/struct.RunContext.html#method.set_state)
- [`ag_ui::serve::StateManager`](/ag-ui-rust/api/ag_ui/serve/struct.StateManager.html)와
  [`StatePublish`](/ag-ui-rust/api/ag_ui/serve/enum.StatePublish.html)
- [`ag_ui::PatchOperation`](/ag-ui-rust/api/ag_ui/enum.PatchOperation.html)
- 같은 이야기의 client 쪽: [session](/ag-ui-rust/ko/client/session/)
