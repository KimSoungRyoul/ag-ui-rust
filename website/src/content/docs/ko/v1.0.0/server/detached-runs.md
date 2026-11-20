---
title: 응답 이후에도 계속되는 run
description: 연결이 끊겨도 driver를 유지하고, 새 구독자를 따라잡게 하거나 저장한 event부터 이어서 실행합니다.
---

`Runner::run`은 응답 stream을 poll할 때 agent를 실행합니다. stream을 drop하면
driver future도 drop됩니다. 애플리케이션에서 하나의 응답과 독립적으로 실행해야
한다면 `Runner::into_parts`를 사용합니다.

## run 실행과 저장

1. 애플리케이션의 executor에서 driver future를 실행합니다.
2. run마다 event receiver 하나를 유지합니다. 즉시 소비하고 순서대로 저장한 뒤
   구독자에게 전달합니다.
3. run을 중지할 때는 runner의 cancellation token을 취소하고 종료 event까지 계속
   읽습니다. driver가 여전히 대기 중인데 receiver가 닫히면 driver를 중지합니다.
   크기가 제한된 큐가 가득 차면 `RUN_ERROR`를 내보낸 뒤 receiver가 닫힐 수 있습니다.

```rust
use ag_ui::{Event, EventType, RunAgentInput, RunOutcome};
use ag_ui::server::{Agent, Result, RunContext, Runner};
use futures_util::StreamExt;

# struct Greeter;
# impl Agent for Greeter {
#     type State = ();
#     async fn run(&self, ctx: &mut RunContext<()>) -> Result<RunOutcome> {
#         ctx.say("Hello")?;
#         Ok(RunOutcome::Success)
#     }
# }
# #[tokio::main]
# async fn main() {
let (driver, mut events) =
    Runner::new(Greeter).into_parts(RunAgentInput::new("thread-1", "run-1"));
let task = tokio::spawn(driver);
let mut stored = Vec::new();
while let Some(event) = events.next().await {
    stored.push(event);
}
task.abort();

assert_eq!(stored.last().map(Event::event_type), Some(EventType::RunFinished));
# }
```

이 벡터는 event store를 대신합니다. 실행 허용, checkpoint, 저장, 구독자 전달은
호스트가 맡습니다. 기본 Axum endpoint의 driver 수명은 응답 body에 연결됩니다.
응답과 독립적인 run은 event store를 구독하는 애플리케이션 endpoint에서 제공합니다.

## 다른 client 연결

저장한 event를 `RunProgress::replay`에 전달하거나, 전송하는 각 event로
`observe`를 호출합니다. `prelude()`는 `RUN_STARTED`에 이어 열려 있는 대상과
쌓인 내용을 재구성합니다. 구독자에게 기준이 필요하면 `Prelude::messages`와
`Prelude::state`로 앱이 관리하는 대화와 state를 전달합니다. `RunProgress`는
열린 대상을 추적하며 `OpenEntity` 값으로 제공할 수 있습니다.

```rust
use ag_ui::{Event, EventType, TextMessageRole};
use ag_ui::server::RunProgress;

let sent = [
    Event::run_started("thread-1", "run-1"),
    Event::text_message_start("msg-1", TextMessageRole::Assistant),
    Event::text_message_content("msg-1", "Hello"),
];
let mut progress = RunProgress::new();
progress.replay(&sent)?;
let prelude = progress.prelude().into_events();
assert_eq!(prelude[0].event_type(), EventType::RunStarted);
# Ok::<(), ag_ui::server::VerificationError>(())
```

구독자가 받을 stream의 앞부분은 한 가지를 선택합니다. 저장한 stream을 처음부터
재생하거나, prelude를 전송한 뒤 그 시점 이후의 event만 보냅니다. 둘 다 적용하면
같은 내용이 두 번 추가될 수 있습니다. 호스트는 저장된 로그와 실시간 구독 사이에서
event가 빠지지 않도록 기준 위치를 정합니다.

## 다른 driver에서 같은 run 계속하기

`Runner::continue_run(input, &sent)`에 같은 thread ID와 run ID, run 시작 시의
메시지, 현재 state, 그리고 이미 보낸 모든 event를 순서대로 전달합니다. 반환된
future와 receiver는 앞에서와 같이 다룹니다. 이어서 실행된 driver는 새 event만
보냅니다. 순서 검사, state 기준, ID 발급에는 이전 event를 사용합니다.

이전 driver가 열어 둔 대상에는 typed handle이 없습니다. 끝내기 전에 기존 ID를
사용해 `RunContext::emit`으로 이어서 보내고 닫습니다. 취소되면 driver가
이어받은 대상을 닫은 뒤 취소 결과를 전송합니다. 직접 event를 만들 때는
`EventVerifier`의 `seed`로 요청 history를 적용하고 `replay`로 이미 보낸 event를
읽힌 뒤, 같은 verifier로 이후 event를 검사합니다.

호스트가 이어서 실행할 때 필요한 앱 state와 실행 로직을 제공합니다. 승인 응답은
별도의 [interrupt와 resume 흐름](/ag-ui-rust/ko/v1.0.0/server/interrupts/)으로 새 run을
시작합니다.

## 실행을 이어갈 때 event 변환

새 transformer에는 `StreamTransformer::seed`로 요청 메시지를 먼저 전달합니다.
이어서 `StreamTransformer::replay`로 구독자가 받은 event를 전달한 뒤 새 event를
처리합니다. 다른 driver가 구독자가 본 내용을 복원할 수 있도록 필터링을 마친 event를
저장합니다.

`FilterToolCalls`는 tool call과 결과에 허용 목록 또는 차단 목록을 적용합니다.
`MESSAGES_SNAPSHOT`과 `RUN_STARTED`의 input echo에 포함된 call도 적용 대상입니다.
필터가 tool 이름을 알 수 없는 call의 event는 버립니다. 새 driver가 tool call을
chunk로 이어서 보낼 때는 첫 chunk에 기존 call ID를 넣습니다. 이후 chunk에는
stream이 기억한 ID를 사용할 수 있습니다. 이 필터는 알려진 tool call 필드만 다룹니다.
raw와 custom event, `rawEvent`, interrupt에 담을 내용은 호스트가 정합니다.
