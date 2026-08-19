---
title: text streaming
description: assistant message를 생성되는 대로 emit하는 법. 그리고 event ordering을 컴파일 타임의 문제로 만드는 handle.
---

assistant message는 wire에서 세 종류의 event입니다. `TEXT_MESSAGE_START`, 여러 개의
`TEXT_MESSAGE_CONTENT`, 그리고 `TEXT_MESSAGE_END`. 셋 다 같은 message id를 싣습니다.

agent에게 날것의 emit 호출 세 개를 쥐여 준다고 해 봅시다. 조기 반환을 포함한 모든 경로에서
자기가 연 것을 순서대로 닫으리라 믿어야 합니다. `ag_ui::serve`는 대신 RAII handle을 줍니다.

## message를 한 번에 보내기

텍스트가 이미 손에 있다면 `say`가 셋을 모두 emit합니다.

```rust
use ag_ui::{Event, RunAgentInput, TextMessageRole};
use ag_ui::serve::RunContext;

fn main() -> ag_ui::serve::Result<()> {
    // `RunContext::new`는 단위 테스트용 harness입니다. context와 event
    // stream의 수신 쪽을 줍니다. agent 안에서는 이것이 그냥 `ctx`입니다.
    let (mut ctx, mut events) = RunContext::<()>::new(RunAgentInput::new("t", "r"))?;

    let id = ctx.say("Hello from Rust.")?;

    assert_eq!(id.as_str(), "r-msg-1");
    assert_eq!(
        events.drain(),
        vec![
            Event::text_message_start("r-msg-1", TextMessageRole::Assistant),
            Event::text_message_content("r-msg-1", "Hello from Rust."),
            Event::text_message_end("r-msg-1"),
        ]
    );
    Ok(())
}
```

message id는 UUID가 아닙니다. run id와 카운터에서 만들어집니다. 위의 `r-msg-1`이 그렇습니다.
protocol은 불투명한 문자열만 요구합니다. 이 crate는 `uuid` 의존성을 두지 않습니다. 그리고
결정적인 id라야 기록한 stream을 diff할 수 있습니다. 직접 정한 id가 필요하면
`message_with_id`가 받습니다.

## 도착하는 대로 streaming하기

`assistant_message()`는 `TEXT_MESSAGE_START`를 emit하고 handle을 돌려줍니다. `delta`는 내용을
덧붙이고, `end`는 닫습니다.

```rust
use ag_ui::{Event, RunAgentInput, TextMessageRole};
use ag_ui::serve::RunContext;

fn main() -> ag_ui::serve::Result<()> {
    let (mut ctx, mut events) = RunContext::<()>::new(RunAgentInput::new("t", "r"))?;

    let mut message = ctx.assistant_message()?;
    for word in ["Hello", ", ", "world"] {
        message.delta(word)?;
    }
    message.end()?;

    assert_eq!(
        events.drain(),
        vec![
            Event::text_message_start("r-msg-1", TextMessageRole::Assistant),
            Event::text_message_content("r-msg-1", "Hello"),
            Event::text_message_content("r-msg-1", ", "),
            Event::text_message_content("r-msg-1", "world"),
            Event::text_message_end("r-msg-1"),
        ]
    );
    Ok(())
}
```

model stream이 그대로 얹히는 모양이 이것입니다. provider가 주는 chunk 하나에 `delta` 하나.
버퍼링은 없습니다. client는 단어가 도착하는 대로 그립니다.

`assistant_message`는 `message(TextMessageRole::Assistant)`입니다. role은
`TEXT_MESSAGE_START`에 실립니다. 나머지 세 배리언트 — `Developer`, `System`, `User` — 는 새로
만드는 대신 기록을 다시 재생하는 agent를 위한 것입니다.

## `end()`는 선택이지만 terminator는 아닙니다

`end`를 부르지 않으면 handle이 `Drop`에서 `TEXT_MESSAGE_END`를 emit합니다. 깜빡했든, message
중간에 `?`로 조기 반환했든, stream은 여전히 온전합니다.

```rust
use ag_ui::{Event, EventType, RunAgentInput};
use ag_ui::serve::{Error, RunContext};

fn write_it(ctx: &mut RunContext<()>) -> ag_ui::serve::Result<()> {
    let mut message = ctx.assistant_message()?;
    message.delta("Looking that up")?;
    // message는 아직 열려 있는데 여기서 반환합니다.
    Err(Error::agent("the weather service is down"))
}

fn main() -> ag_ui::serve::Result<()> {
    let (mut ctx, mut events) = RunContext::<()>::new(RunAgentInput::new("t", "r"))?;
    assert!(write_it(&mut ctx).is_err());

    let types: Vec<EventType> = events.drain().iter().map(Event::event_type).collect();
    assert_eq!(
        types,
        [
            EventType::TextMessageStart,
            EventType::TextMessageContent,
            // 실패하는 함수를 빠져나오는 길에 `Drop`이 emit한 것.
            EventType::TextMessageEnd,
        ]
    );
    Ok(())
}
```

그래도 오류를 보고 싶다면 `end()`를 부르십시오. `Drop`은 오류를 보고할 곳이 없어서 그냥
삼킵니다.

## 왜 `delta`에 `.await`이 없는가

`msg.delta(text)?`는 동기입니다. emitter API로는 드문 일입니다. TypeScript SDK와 .NET SDK는
둘 다 `await`을 씁니다. 이 crate의 첫 초안도 그것을 따라 했습니다.

하지만 async emit은 위의 보장과 공존할 수 없습니다. Rust에서 `Drop`은 async일 수 없습니다.
그래서 handle은 terminator를 emit하면서 `await`할 수 없습니다. 길은 둘뿐입니다. terminator가
자동이고 emit 경로가 동기이거나, emit 경로가 async이고 모든 agent가 자기 message를 직접 닫는
것을 잊지 말아야 하거나. 이 SDK는 앞쪽을 골랐습니다.

emitter는 unbounded channel에 밀어 넣고 transport가 그것을 비웁니다. 막히는 곳도 없고, 읽는
쪽을 기다리며 쌓이는 것도 없습니다.

여기서 나오는 실질적 결과는 반갑습니다. agent code를 호출하고 나면 그것이 emit한 것은 이미
전부 큐에 들어가 있습니다. 위의 단언들이 런타임 하나 없이 그냥 `drain()` 호출로 끝나는 이유가
이것입니다.

## message 두 개를 동시에 열면 컴파일되지 않습니다

handle은 살아 있는 동안 run context를 가변으로 빌립니다. 그래서 첫 message가 열려 있는 동안
두 번째를 여는 것은 borrow check 오류입니다. frontend가 나중에 발견하는 protocol 위반이
아닙니다.

```rust,compile_fail
use ag_ui::serve::RunContext;

fn interleave(ctx: &mut RunContext<()>) {
    let mut first = ctx.assistant_message().unwrap();
    // error[E0499]: cannot borrow `*ctx` as mutable more than once at a time
    let mut second = ctx.assistant_message().unwrap();
    first.delta("a").unwrap();
    second.delta("b").unwrap();
}
```

저 블록은 `compile_fail`입니다. 언젠가 컴파일되기 시작하면 이 페이지의 빌드가 깨집니다. 같은
예제가 `crates/ag-ui/src/serve/emit/mod.rs`에 `compile_fail,E0499` doctest로 있습니다. 그것이
이 보장이 아직 살아 있다는 실행 가능한 증거입니다. emitter API를 느슨하게 만들면 그 테스트가
빨개집니다.

:::caution
안정 버전 rustdoc은 `compile_fail` doctest의 오류 code를 강제하지 않습니다. 어떤 이유로든 그
블록이 컴파일에 실패하는지만 봅니다. 오타여도 통과합니다. 그래서 CI는 doctest를
나이틀리에서도 돌립니다. 나이틀리는 오류 code를 강제합니다.
:::

## 열려 있는 message가 그래도 할 수 있는 일

borrow가 금지하는 것은 두 번째 *블록*이지 작업이 아닙니다. handle은 run context 자체가 아니라
그 필드 두 개를 쥡니다. event sink와 상태입니다. 그래서 message가 열려 있어도 상태에 손이
닿습니다.

```rust
use ag_ui::RunAgentInput;
use ag_ui::serve::RunContext;
use serde::{Deserialize, Serialize};

#[derive(Default, Serialize, Deserialize)]
struct Progress {
    words: u32,
}

fn main() -> ag_ui::serve::Result<()> {
    let (mut ctx, mut events) = RunContext::<Progress>::new(RunAgentInput::new("t", "r"))?;

    let mut message = ctx.assistant_message()?;
    message.delta("Hello")?;
    message.state_mut().words += 1;
    // message가 열린 채로 STATE_SNAPSHOT. `STATE_*`는 wire에서 순서가
    // 없으므로 이것은 적법한 stream입니다.
    message.publish_state()?;
    message.end()?;

    assert_eq!(ctx.state().words, 1);
    assert_eq!(events.drain().len(), 4);
    Ok(())
}
```

`message.emit(event)`는 그 일반형입니다. message 사이에 적법하게 끼어들 수 있는 순서 없는
계열 — `STATE_*`, `ACTIVITY_*`, `CUSTOM`, `RAW` — 을 위한 것입니다. 이것으로 두 번째 message를
여는 것은 protocol 위반입니다. [ordering verifier](/ag-ui-rust/ko/server/errors/)가 emit
시점에 거부합니다.

handle이 할 수 없는 일은 또 다른 블록을 여는 것입니다. 블록을 열 run context를 쥐고 있지
않습니다. 그리고 handle이 나온 context는 handle이 드롭될 때까지 빌려진 채로 남습니다.

## reasoning

client가 그려 주기를 바라는 model의 reasoning에는 자기만의 event 계열과 자기만의 handle이
있습니다. `REASONING_*`은 블록 안에 message를 중첩합니다. 그래서 handle은 두 개가 아니라 네
개의 event를 감쌉니다.

```text
REASONING_START          ← 생성 시
REASONING_MESSAGE_START  ← 생성 시
REASONING_MESSAGE_CONTENT × n
REASONING_MESSAGE_END    ← end() 또는 Drop 시
REASONING_END            ← end() 또는 Drop 시
```

```rust
use ag_ui::{Event, EventType, RunAgentInput};
use ag_ui::serve::RunContext;

fn main() -> ag_ui::serve::Result<()> {
    let (mut ctx, mut events) = RunContext::<()>::new(RunAgentInput::new("t", "r"))?;

    // `say`처럼 한 번에 끝내는 형태.
    ctx.think("The user wants a title.")?;

    // `assistant_message`처럼 streaming하는 형태.
    let mut reasoning = ctx.reasoning()?;
    reasoning.delta("Checking the board first")?;
    reasoning.end()?;

    let types: Vec<EventType> = events.drain().iter().map(Event::event_type).collect();
    assert_eq!(types.len(), 10);
    assert_eq!(types[0], EventType::ReasoningStart);
    assert_eq!(types[9], EventType::ReasoningEnd);
    Ok(())
}
```

provider가 불투명한 덩어리로만 돌려주는 reasoning도 있습니다. 데이터를 보존하지 않는 경우인데,
model이 일관성을 유지하려면 다음 요청에 서명을 그대로 다시 실어야 합니다. 이것은
`reasoning.encrypted_value(…)`를 거치며 `REASONING_ENCRYPTED_VALUE`를 emit합니다.

## chunk

`*_CHUNK` 계열은 정의상 앞뒤로 감싸이지 않습니다. chunk는 자기 id를 싣고 다니므로 시작도 끝도
필요 없습니다. 이 계열은 provider adapter를 위해 있습니다. 다음 message가 시작되기 전까지
이전 message가 끝났는지 알 수 없는 adapter 말입니다. RAII handle이 닫아 줄 것이 없으니 이
crate도 chunk용 handle을 주지 않습니다. 필요하면 `ctx.emit`으로 emit하십시오.

```rust
use ag_ui::{Event, MessageId, RunAgentInput};
use ag_ui::serve::RunContext;

fn main() -> ag_ui::serve::Result<()> {
    let (mut ctx, mut events) = RunContext::<()>::new(RunAgentInput::new("t", "r"))?;

    ctx.emit(Event::text_message_chunk(
        Some(MessageId::new("chunk-1")),
        Some("a whole update in one event".to_owned()),
    ))?;

    assert_eq!(events.drain().len(), 1);
    Ok(())
}
```

verifier는 chunk가 자기완결적이라는 것을 압니다. 그래서 시작이 없다고 거부하는 대신 그 id를
등록합니다.

## API

- [`RunContext::say`](/ag-ui-rust/api/ag_ui/serve/struct.RunContext.html#method.say),
  [`assistant_message`](/ag-ui-rust/api/ag_ui/serve/struct.RunContext.html#method.assistant_message),
  [`message_with_id`](/ag-ui-rust/api/ag_ui/serve/struct.RunContext.html#method.message_with_id)
- [`ag_ui::serve::MessageHandle`](/ag-ui-rust/api/ag_ui/serve/struct.MessageHandle.html)
- [`ag_ui::serve::ReasoningHandle`](/ag-ui-rust/api/ag_ui/serve/struct.ReasoningHandle.html)
- [`ag_ui::serve::emit`](/ag-ui-rust/api/ag_ui/serve/emit/index.html) — typestate 설계를
  설명하는 모듈
