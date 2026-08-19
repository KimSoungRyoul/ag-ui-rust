---
title: error와 cancellation
description: run이 실패를 client에 알리는 방식. 그리고 stream 도중에 호출자가 사라졌을 때 agent에 벌어지는 일.
---

실패한 run도 여전히 run입니다. driver는 `Agent::run`을 빠져나온 것이 무엇이든 `RUN_ERROR`
event로 바꿉니다. 그래서 client는 온전한 stream을 받습니다. 무엇이 잘못되었는지 말하며 끝나는
stream입니다. panic도 아니고, 그냥 뚝 끊기는 본문도 아닙니다.

```rust
// src/agent.rs
use ag_ui::{Event, RunAgentInput, RunOutcome};
use ag_ui::serve::{Agent, Error, Result, RunContext, run};
use futures_util::StreamExt;

struct Flaky;

impl Agent for Flaky {
    type State = ();

    async fn run(&self, ctx: &mut RunContext<()>) -> Result<RunOutcome> {
        ctx.say("Looking that up.")?;
        Err(Error::agent("the weather service is down"))
    }
}

#[tokio::main]
async fn main() {
    let events: Vec<Event> = run(Flaky, RunAgentInput::new("t", "r"))
        .map(|event| event.expect("the stream should not break"))
        .collect()
        .await;

    let Some(Event::RunError(error)) = events.last() else {
        panic!("a failed run ends in RUN_ERROR: {events:?}");
    };
    assert_eq!(error.message, "agent error: the weather service is down");
    assert_eq!(error.code.as_deref(), Some("AGENT_ERROR"));
}
```

`Error::agent`는 `Into<Box<dyn std::error::Error + Send + Sync>>`인 것이면 무엇이든 감쌉니다.
대부분이 여기 들어갑니다. `String`, `&str`, 여러분이 만든 오류 타입 모두입니다. 그래서 직접
만든 실패에 `?`를 쓰려면 보통 `map_err(Error::agent)` 하나면 충분합니다.

## 배리언트

`ag_ui::serve::Error`는 이 crate의 모든 메서드가 반환하는 타입입니다. `Result<T, E = Error>`
별칭을 통해서 말입니다. 각 배리언트에는 `RUN_ERROR` event에 실리는 안정된 code가 있습니다.

| 배리언트 | code | 언제 나오는가 |
| --- | --- | --- |
| `Protocol` | `PROTOCOL` | core 타입이 값을 거부했을 때. 이를테면 interrupt가 없는 `interrupt` outcome |
| `Json` | `SERIALIZATION` | 상태, tool 인자, tool 결과가 JSON으로 오가지 못할 때 |
| `Verification` | `PROTOCOL_VIOLATION` | emit된 stream이 ordering 규칙을 어겼을 때 |
| `Cancelled` | `CANCELLED` | run이 취소되었을 때. 대개 client가 연결을 끊었기 때문 |
| `Disconnected` | `DISCONNECTED` | 소비자가 event stream을 드롭했을 때 |
| `Agent` | `AGENT_ERROR` | 여러분의 code가 실패했을 때. `Error::agent`로 만듭니다 |

분기할 일이 가장 많은 둘은 `is_cancelled()`와 `is_disconnected()`입니다. 둘 다 "무언가
망가졌다"는 뜻이 아닙니다. "그만두라, 아무도 듣고 있지 않다"는 뜻입니다.

이 열거형은 `#[non_exhaustive]`입니다. 이 workspace의 모든 오류 타입이 그렇습니다. `Event`와
`EventType`은 일부러 그렇게 하지 **않았습니다**. 그 비대칭이 핵심입니다.

새 protocol event는 소비자에게 컴파일 오류여야 *합니다*. `_` 갈래야말로 "새 event가
도착했다"를 아무 진단도 없는 상태로 만들어 버리는 구문이기 때문입니다. 실패 양상은 반대입니다.
전체를 빠짐없이 매치하고 싶은 사람은 없습니다. 호출자는 몇 안 되는 배리언트로만 분기하고
나머지는 흘려보냅니다. 그리고 새로운 실패 양상은 wire 계약의 변경이 아닙니다. 이 논증은
`docs/DESIGN.md`에 적혀 있습니다.

:::caution
*panic*은 오류가 아닙니다. 이 crate 어디에서도 잡히지 않습니다. 다른 future에서와 똑같이,
stream을 polling하는 쪽으로 그대로 unwind됩니다. HTTP에서는 그때쯤 상태 라인이 이미 나간
뒤입니다. 그래서 client에게는 본문이 잘린 것으로 보입니다. 예상되는 실패에는
`Err(Error::agent(…))`를 반환하십시오. `tower_http::catch_panic`은 예상하지 못한 것에만
쓰십시오.
:::

## protocol verification

borrow checker는 겹치는 message 두 개를 막아 줍니다. 그것이 볼 수 없는 것이 날것의
`ctx.emit`입니다. 그래서 ordering 상태 기계가 나가는 event를 하나하나 지켜봅니다. 기본으로 켜져
있습니다. 세 홉 떨어진 하류가 아니라 버그가 만들어지는 곳인 server에서 돕니다.

```rust
use ag_ui::{Event, RunAgentInput};
use ag_ui::serve::{Error, RunContext, Rule};

fn main() -> ag_ui::serve::Result<()> {
    let (mut ctx, _events) = RunContext::<()>::new(RunAgentInput::new("t", "r"))?;

    // 시작된 적 없는 message에 대한 content event.
    let error = ctx
        .emit(Event::text_message_content("msg-1", "hello"))
        .expect_err("the verifier should reject this");

    let Error::Verification(failure) = &error else {
        panic!("expected a verification failure, got {error}");
    };
    assert_eq!(failure.rule, Rule::NotOpen);
    assert_eq!(error.code(), "PROTOCOL_VIOLATION");
    Ok(())
}
```

규칙은 일곱 개입니다. `Rule::describe()`가 각각을 한 문장으로 말해 줍니다.

| 규칙 | 무엇을 금지하는가 |
| --- | --- |
| `RunEnded` | `RUN_FINISHED`나 `RUN_ERROR` 이후의 모든 것 |
| `DuplicateRunStarted` | 두 번째 `RUN_STARTED` |
| `DuplicateStart` | 이미 열려 있는 id로 message, reasoning 블록, tool call, step을 여는 것 |
| `NotOpen` | 열린 적 없는 것에 대한 content나 terminator |
| `UnknownId` | 소개된 적 없는 call id에 대한 tool 결과 |
| `OutOfOrder` | call의 `TOOL_CALL_END`보다 앞선 tool 결과 |
| `OpenAtFinish` | 무언가 아직 열려 있는데 나가는 `RUN_FINISHED` |

`RUN_ERROR`는 `OpenAtFinish`에서 면제됩니다. message 도중에 터져 버린 run이 그것을 닫았을 리
없기 때문입니다. 그 면제는 다른 일도 합니다. 거부당한 `RUN_FINISHED` 때문에 run에 최종 event가
하나도 남지 않는 사태를 막아 줍니다. driver가 그 거부를 대신 `RUN_ERROR`로 보고합니다.

`VerificationError`는 event와 규칙과 관련된 id를 밝힙니다. 디버그 빌드에서는 아직 열려 있는 것
전부를 덧붙여 쏟아 냅니다. 대개 그것이면 빠진 terminator를 짚어 내기에 충분합니다.

```text
TEXT_MESSAGE_CONTENT breaks rule `not-open` (content and terminators require
a matching start): message "msg-2" is not open [open: messages={"msg-1"}]
```

비용은 `HashSet` 몇 개와 event당 조회 한 번입니다. `verify`는 이 crate의 유일한 feature flag고
기본으로 켜져 있습니다. 이것을 끄면 상태 기계 전체가 크기 0인 타입으로 바뀝니다. `observe`가
인라인된 `Ok(())`인 타입입니다. `Verification` 배리언트가 나올 유일한 출처도 사라집니다. 비싼
쪽은 디버그 전용 덤프입니다. 그래서 그것이 디버그 전용입니다.

## cancellation

`CancellationToken`은 공유되는 "지금 멈춰라" 플래그입니다. `AtomicBool` 하나와 waker 목록
하나입니다. 복제 비용이 싸고, 모든 복제본이 같은 플래그를 가리킵니다.

이것이 일부러 `tokio_util::sync::CancellationToken`이 아닌 이유가 있습니다. 이 crate는 wasm과
tokio 아닌 executor를 대상으로도 빌드됩니다. `tokio_util`을 들이면 그것이 끝납니다.

transport는 client가 연결을 끊거나 마감 시각이 지나면 token을 발동시킵니다. agent의 협조가
전혀 없어도 그것이 통하는 이유는 **cancellation 이후의 모든 emit이 실패하기** 때문입니다. 다음
`?`가 run을 되감습니다.

```rust
use ag_ui::{Event, EventType, RunAgentInput, RunOutcome};
use ag_ui::serve::{Agent, Result, RunContext, run};
use futures_util::StreamExt;

struct Chatty;

impl Agent for Chatty {
    type State = ();

    async fn run(&self, ctx: &mut RunContext<()>) -> Result<RunOutcome> {
        ctx.say("one")?;
        // client가 끊은 것을 transport가 알아챈 상황을 대신합니다.
        ctx.cancel_token().cancel();
        ctx.say("two")?;   // 실패하고, `?`가 반환합니다
        ctx.say("three")?;
        Ok(RunOutcome::Success)
    }
}

#[tokio::main]
async fn main() {
    let events: Vec<Event> = run(Chatty, RunAgentInput::new("t", "r"))
        .map(|event| event.expect("the stream should not break"))
        .collect()
        .await;

    let said: Vec<&str> = events
        .iter()
        .filter_map(|event| match event {
            Event::TextMessageContent(content) => Some(content.delta.as_str()),
            _ => None,
        })
        .collect();
    assert_eq!(said, ["one"]);

    // 최종 event는 cancellation과 상관없이 나갑니다.
    assert_eq!(events.last().map(Event::event_type), Some(EventType::RunError));
    let Some(Event::RunError(error)) = events.last() else {
        panic!("{events:?}");
    };
    assert_eq!(error.code.as_deref(), Some("CANCELLED"));
}
```

더 일찍 알아채고 싶은 agent가 물어볼 방법은 네 가지입니다.

| 메서드 | 모양 |
| --- | --- |
| `is_cancelled()` | `bool`. 루프 조건용 |
| `check_cancelled()` | `Result<()>`. 단계 사이에 `?` 하나만 붙일 때 |
| `cancelled()` | token이 발동하면 완료되는 `'static` future |
| `until_cancelled(f)` | `f`를 cancellation과 경주시키고, cancellation이 이기면 `None` |

긴 model 호출에서 정말로 중요한 것은 `until_cancelled`입니다. 이미 날아가 있는 요청이야말로
cancellation이 그 비용을 멈추려는 대상이기 때문입니다.

```rust
use ag_ui::{RunAgentInput, RunOutcome};
use ag_ui::serve::{Agent, Error, Result, RunContext};

struct Slow;

impl Agent for Slow {
    type State = ();

    async fn run(&self, ctx: &mut RunContext<()>) -> Result<RunOutcome> {
        let reply = ctx
            .until_cancelled(call_the_model())
            .await
            .ok_or(Error::Cancelled)?;
        ctx.say(reply)?;

        Ok(RunOutcome::Success)
    }
}

async fn call_the_model() -> String {
    "the model's reply".to_owned()
}

#[tokio::main]
async fn main() -> ag_ui::serve::Result<()> {
    let (ctx, _events) = RunContext::<()>::new(RunAgentInput::new("t", "r"))?;

    let answer = ctx.until_cancelled(call_the_model()).await;
    assert_eq!(answer.as_deref(), Some("the model's reply"));

    // 이미 발동된 token을 상대로, 결코 완료되지 않는 future.
    ctx.cancel_token().cancel();
    let never = futures_util::future::pending::<String>();
    assert!(ctx.until_cancelled(never).await.is_none());
    Ok(())
}
```

`until_cancelled`가 일부러 `async fn`이 아닌 데에는 이유가 있습니다. `async fn`이었다면 반환된
future가 `&self`를 붙잡습니다. run context의 borrow를 품은 future는 그 context가 `Sync`일
때에만 `Send`입니다. 그리고 run context는 `Sync`가 아닙니다. stream transformer는 `Send`이기만
하면 되기 때문입니다.

`cancelled()`는 token을 빌리지 않고 복제본을 소유하는 future를 돌려줍니다. `Arc` 참조 하나를
더 얹는 대가입니다. 그 대신 agent는 `'static` future를 얻습니다. run context의 borrow를 끌고
다니지 않고도 await를 건너 붙잡고 있을 수 있는 future입니다.

## 누가 token을 발동시키는가

HTTP에서는 [`ag_ui::axum`](/ag-ui-rust/ko/server/axum/)이 합니다. 응답 본문이 run을 소유합니다.
그래서 client가 사라지면 hyper가 본문을 드롭하고 run도 함께 사라집니다.

본문은 guard도 함께 쥐고 있습니다. 드롭될 때 token을 발동시키는 guard입니다. run이 무사히
끝났다면 그 guard는 스스로 해제됩니다. 그 두 번째 부분이야말로 run이 자기 바깥에서 건드린 모든
것에까지 닿는 방법입니다. spawn된 tool call, 날아가 있는 model 요청, 붙잡고 있는 락 말입니다.

직접 transport를 만든다면 `Runner::cancellation_token()`으로 token을 받아 두십시오. `run`이
runner를 소비하기 전에 말입니다. 그리고 연결이 끝날 때 발동시키십시오.

## API

- [`ag_ui::serve::Error`](/ag-ui-rust/api/ag_ui/serve/enum.Error.html)와
  [`Result`](/ag-ui-rust/api/ag_ui/serve/type.Result.html)
- [`ag_ui::serve::Rule`](/ag-ui-rust/api/ag_ui/serve/enum.Rule.html)과
  [`VerificationError`](/ag-ui-rust/api/ag_ui/serve/struct.VerificationError.html)
- [`ag_ui::serve::verify`](/ag-ui-rust/api/ag_ui/serve/verify/index.html) — 규칙 하나하나를
  담은 상태 기계
- [`ag_ui::serve::CancellationToken`](/ag-ui-rust/api/ag_ui/serve/struct.CancellationToken.html)과
  [`Cancelled`](/ag-ui-rust/api/ag_ui/serve/struct.Cancelled.html)
- `verify`가 무엇을 치르고 무엇을 없애는지는 [feature flag](/ag-ui-rust/ko/reference/features/)
