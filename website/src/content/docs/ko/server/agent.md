---
title: Agent trait
description: 이 SDK의 유일한 경계인 trait를 구현하는 법. 그리고 run context가 구현체에 건네주는 것들.
---

`Agent`는 `ag-ui-server`의 확장 지점 전부입니다. trait는 하나뿐입니다. 연관 타입 하나와
메서드 하나를 가집니다. 이 문서의 나머지는 전부 run context가 그 메서드에 건네주는 것들입니다.

```rust
// crates/ag-ui-server/src/agent.rs — 문서 주석을 걷어낸 선언부.
use ag_ui_core::RunOutcome;
use ag_ui_server::{AgentState, Result, RunContext};
use std::future::Future;

pub trait Agent: Send + Sync {
    type State: AgentState;

    fn run(
        &self,
        ctx: &mut RunContext<Self::State>,
    ) -> impl Future<Output = Result<RunOutcome>> + Send;
}
```

이 trait는 모델도 프롬프트도 provider도 말하지 않습니다. 일부러 그렇습니다.

.NET SDK는 `Microsoft.Extensions.AI` 위에 섭니다. .NET에는 공인된 채팅 추상화가 하나 있기
때문입니다. Rust에는 없습니다. 생태계가 `async-openai`, `rig-core`, `genai`로 갈라져 있고
승자가 없습니다. 그중 하나에 묶으면 이 crate는 나머지 대부분에게 쓸모없어집니다.

그래서 `ag-ui-server`는 LLM crate에 전혀 의존하지 않습니다. 쓰던 client를 그대로
가져오십시오. `run` 안에서 호출하고, 나오는 것을 emit하면 됩니다. 프레임워크 연동은 별도
crate에 있는 `impl Agent for …` 하나입니다.

## 완전한 agent 하나

아래는 그대로 컴파일되고 실행됩니다. 이것이 프로그램 전부입니다.

```rust
// src/main.rs
use ag_ui_core::{Event, EventType, RunAgentInput, RunOutcome};
use ag_ui_server::{Agent, Result, RunContext, run};
use futures_util::StreamExt;

struct Greeter;

impl Agent for Greeter {
    type State = ();

    async fn run(&self, ctx: &mut RunContext<()>) -> Result<RunOutcome> {
        let mut message = ctx.assistant_message()?;
        message.delta("Hello from Rust.")?;
        message.end()?;

        Ok(RunOutcome::Success)
    }
}

#[tokio::main]
async fn main() {
    let input = RunAgentInput::new("thread-1", "run-1");

    let events: Vec<Event> = run(Greeter, input)
        .map(|event| event.expect("the stream should not break"))
        .collect()
        .await;

    let types: Vec<EventType> = events.iter().map(Event::event_type).collect();
    assert_eq!(
        types,
        [
            EventType::RunStarted,
            EventType::TextMessageStart,
            EventType::TextMessageContent,
            EventType::TextMessageEnd,
            EventType::RunFinished,
        ]
    );
}
```

다섯 event 중 둘은 agent code에 없습니다. `run()`이 driver이기 때문입니다. agent를 호출하기
전에 `RUN_STARTED`를 emit합니다. agent가 반환되면 `RUN_FINISHED` / `RUN_ERROR` 중 정확히
하나를 emit합니다. 본문이 아무 일도 하지 않았을 때도, `?`로 `Err`가 빠져나갔을 때도
마찬가지입니다.

driver는 자체 executor가 없습니다. agent의 future를 소유한 채 stream에서 polling합니다.
그래서 stream을 비우는 일이 곧 agent를 실행하는 일입니다. crate 어디에도 `spawn`이 없고,
설정할 것도 없습니다. 그 stream을 응답 본문으로 바꾸는 쪽은
[HTTP로 serving](/ag-ui-rust/ko/server/axum/)입니다.

## `type State`

`State`는 client가 함께 비추어 보는 shared state입니다. 들어올 때 `RunAgentInput.state`에서
역직렬화됩니다. 나갈 때는 `STATE_SNAPSHOT` / `STATE_DELTA` event로 나갑니다.

bound는 `AgentState`입니다. 직접 구현할 일은 없습니다. 자격을 갖춘 모든 타입을 blanket impl이
덮습니다.

```rust
use ag_ui_core::RunOutcome;
use ag_ui_server::{Agent, Result, RunContext};
use serde::{Deserialize, Serialize};

/// `Serialize + DeserializeOwned + Default + Send`가 bound의 전부입니다.
/// `#[derive(Default, Serialize, Deserialize)]`가 그것을 모두 충족합니다.
#[derive(Default, Serialize, Deserialize)]
struct Draft {
    revision: u32,
    title: String,
}

struct Editor;

impl Agent for Editor {
    type State = Draft;

    async fn run(&self, ctx: &mut RunContext<Draft>) -> Result<RunOutcome> {
        ctx.update_state(|draft| {
            draft.revision += 1;
            draft.title = "Q3 plan".into();
        })?;

        Ok(RunOutcome::Success)
    }
}
```

상태를 두지 않는 agent라면 `()`를 쓰십시오. 위의 `Greeter`가 그렇습니다. `state`가 `null`이거나
빈 객체인 요청은 실패하지 않습니다. `S::default()`로 디코딩됩니다. 그래서 상태 없는 agent도
모든 client를 상대할 수 있습니다. 내보내는 쪽은
[shared state](/ag-ui-rust/ko/server/state/)에서 다룹니다.

## 왜 `#[async_trait]`이 아니라 `async fn`인가

`Agent::run`은 네이티브 `-> impl Future + Send`, 즉 RPITIT로 쓰여 있습니다. 그래서 구현체는
평범한 `async fn`입니다. 매크로도 없고, 호출마다 붙는 `Box::pin`도 없고, run마다 생기는 할당도
없습니다.

치르는 비용은 실재하니 분명히 밝혀 둡니다. RPITIT 메서드를 가진 trait는 `dyn` 호환이 아닙니다.
그래서 `Box<dyn Agent>`가 없습니다. 그것이 필요할 때가 있습니다. endpoint 하나 뒤에 여러
agent를 등록해 두는 경우입니다. `DynAgent`가 그 박싱된 형태이고, 모든 `Agent`에 대해 구현되어
있습니다. `BoxAgent<S>`는 다시 `Agent`를 구현합니다. 그래서 driver는 그것을 다른 agent와
똑같이 받습니다.

```rust
use ag_ui_core::RunOutcome;
use ag_ui_server::{Agent, BoxAgent, Result, RunContext};

struct Fixed(&'static str);

impl Agent for Fixed {
    type State = ();

    async fn run(&self, ctx: &mut RunContext<()>) -> Result<RunOutcome> {
        ctx.say(self.0)?;
        Ok(RunOutcome::Success)
    }
}

let agents: Vec<BoxAgent<()>> = vec![Box::new(Fixed("a")), Box::new(Fixed("b"))];
assert_eq!(agents.len(), 2);
```

차이는 run마다 박싱된 future 하나입니다. `Agent`는 `&A`와 `Arc<A>`에 대해서도 구현되어
있습니다. agent 값 하나가 복제 없이 동시 요청 여럿을 받아내는 방법이 이것입니다.

## 왜 `RunContext`가 아니라 `&mut RunContext`인가

agent는 context를 빌릴 뿐입니다. 소유하지 않습니다. 그래야 driver가 `run`이 반환된 *뒤에*
최종 event를 emit할 수 있습니다. agent가 쓰던 것과 같은 transformer chain, 같은 ordering
verifier를 거쳐서 말입니다. context를 값으로 넘기면 둘 다 agent의 마지막 문장과 함께
사라집니다. 그러면 최종 event는 verification 없이 나갑니다.

## context가 건네주는 것

`RunContext<S>`는 요청과 상태와 event sink와 취소 플래그를 한 값에 담은 것입니다. 읽는 쪽에는
`&mut`이 필요 없습니다.

```rust
use ag_ui_core::{Message, RunAgentInput, Tool};
use ag_ui_server::RunContext;
use serde_json::json;

fn main() -> ag_ui_server::Result<()> {
    let mut input = RunAgentInput::new("thread-1", "run-1");
    input.messages = vec![Message::user("msg-1", "what is the weather in Seoul?")];
    input.tools = vec![Tool::new(
        "get_weather",
        "Look up the current weather for a city.",
        json!({"type": "object", "properties": {"city": {"type": "string"}}}),
    )];

    // `RunContext::new`는 단위 테스트용 harness입니다. context와 event
    // stream의 수신 쪽을 줍니다. driver가 없으니 RUN_STARTED도 없습니다.
    let (ctx, _events) = RunContext::<()>::new(input)?;

    assert_eq!(ctx.thread_id().as_str(), "thread-1");
    assert_eq!(ctx.run_id().as_str(), "run-1");
    assert_eq!(ctx.messages().len(), 1);
    assert_eq!(
        ctx.last_user_text().as_deref(),
        Some("what is the weather in Seoul?")
    );
    assert!(ctx.tool("get_weather").is_some());
    assert!(ctx.tool("send_email").is_none());
    assert!(!ctx.is_resume());

    Ok(())
}
```

| 접근자 | 무엇을 알려주는가 |
| --- | --- |
| `thread_id`, `run_id`, `parent_run_id` | 어느 대화인지, 어느 run인지, 어느 run이 이것을 낳았는지 |
| `messages` | 대화 기록. 오래된 것부터 |
| `last_user_text` | 거의 언제나 지금 답하고 있는 그 발화 |
| `tools`, `tool(name)` | client가 제공한 것 — [tool call](/ag-ui-rust/ko/server/tools/) 참고 |
| `context`, `forwarded_props` | 덧붙은 context 항목과 그대로 통과시키는 불투명한 값 |
| `resume`, `resume_for`, `is_resume` | 앞서 멈춘 지점에 대한 답 — [human in the loop](/ag-ui-rust/ko/server/interrupts/) 참고 |
| `input` | 나머지가 다루지 못하는 것을 위한 `RunAgentInput` 전체 |

`last_user_text`는 멀티모달 message에서 텍스트가 아닌 부분을 버립니다. 기록에 사용자 message가
아예 없으면 `None`을 돌려줍니다. 사용자가 빈 message를 보낸 경우와는 다릅니다. 이미지가
중요하다면 `messages`를 직접 보십시오.

쓰는 쪽은 모두 `&mut self`를 받습니다. 페이지를 나누어 다룹니다.
[text streaming](/ag-ui-rust/ko/server/text/), [tool call](/ag-ui-rust/ko/server/tools/),
[shared state](/ag-ui-rust/ko/server/state/)입니다. 그 아래에 깔린 `ctx.emit(event)`는 타입이
붙은 emitter가 없는 모든 것을 위한 탈출구입니다.

## step으로 run 구간 묶기

`ctx.step(name)`은 `STEP_STARTED`를 emit하고 guard를 돌려줍니다. guard는 드롭될 때
`STEP_FINISHED`를 emit합니다. `?`가 만드는 조기 반환에서도 그렇습니다. step은 stream이 아니라
*scope*입니다. 그래서 message handle이나 tool call handle과 달리 이 guard는 run context로
역참조되고, 모든 것이 그 안에 중첩됩니다.

```rust
use ag_ui_core::{Event, EventType, RunAgentInput, RunOutcome};
use ag_ui_server::{Agent, Result, RunContext, run};
use futures_util::StreamExt;

struct Researcher;

impl Agent for Researcher {
    type State = ();

    async fn run(&self, ctx: &mut RunContext<()>) -> Result<RunOutcome> {
        let mut step = ctx.step("research")?;
        step.say("Looking it up.")?;   // Deref를 통해, context에 대고
        // 위의 `?`가 발동했든 아니든 STEP_FINISHED는 여기서 나갑니다.
        drop(step);

        Ok(RunOutcome::Success)
    }
}

#[tokio::main]
async fn main() {
    let events: Vec<Event> = run(Researcher, RunAgentInput::new("t", "r"))
        .map(|event| event.expect("the stream should not break"))
        .collect()
        .await;

    let types: Vec<EventType> = events.iter().map(Event::event_type).collect();
    assert_eq!(
        types,
        [
            EventType::RunStarted,
            EventType::StepStarted,
            EventType::TextMessageStart,
            EventType::TextMessageContent,
            EventType::TextMessageEnd,
            EventType::StepFinished,
            EventType::RunFinished,
        ]
    );
}
```

step은 선택입니다. protocol은 step을 요구하지 않습니다. message 하나만 emit하는 agent는 step이
없는 편이 더 분명합니다.

## run이 끝나는 방식

`run`은 `Result<RunOutcome>`을 반환합니다. 반환될 수 있는 세 가지가 곧 run이 끝날 수 있는 세
가지입니다.

- `Ok(RunOutcome::Success)` — run이 완료되었습니다. driver는 `success` outcome을 담아
  `RUN_FINISHED`를 emit합니다.
- `Ok(RunOutcome::Interrupt { .. })` — run이 멈춘 채 사람을 기다립니다. 이것도
  `RUN_FINISHED`입니다. 대기 중인 interrupt를 싣고 나갑니다.
  [human in the loop](/ag-ui-rust/ko/server/interrupts/)를 보십시오.
- `Err(_)` — run이 실패했습니다. driver는 오류 문구와 code를 담아 `RUN_ERROR`를 emit합니다. agent
  오류는 panic도 아니고 잘린 stream도 아닙니다.
  [error와 cancellation](/ag-ui-rust/ko/server/errors/)를 보십시오.

:::caution
agent 안에서 난 *panic*은 잡히지 않습니다. 다른 future에서와 똑같이, stream을 polling하는
쪽으로 그대로 unwind됩니다. HTTP에서는 `200`이 이미 나간 뒤입니다. 그래서 client에게는 본문이
잘린 것으로 보입니다. 예상되는 실패에는 `Err(Error::agent(…))`를 반환하십시오.
:::

## API

- [`ag_ui_server::Agent`](/ag-ui-rust/api/ag_ui_server/trait.Agent.html)
- [`ag_ui_server::AgentState`](/ag-ui-rust/api/ag_ui_server/trait.AgentState.html)
- [`ag_ui_server::RunContext`](/ag-ui-rust/api/ag_ui_server/struct.RunContext.html)
- [`ag_ui_server::run`](/ag-ui-rust/api/ag_ui_server/fn.run.html)과
  [`Runner`](/ag-ui-rust/api/ag_ui_server/struct.Runner.html)
- [`ag_ui_server::DynAgent`](/ag-ui-rust/api/ag_ui_server/trait.DynAgent.html)과
  [`BoxAgent`](/ag-ui-rust/api/ag_ui_server/type.BoxAgent.html)
- [`ag_ui_core::RunOutcome`](/ag-ui-rust/api/ag_ui_core/enum.RunOutcome.html)
