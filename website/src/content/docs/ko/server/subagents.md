---
title: subagent
description: run의 일부를 자식 agent에 맡기는 법, 그 agent가 emit한 것에 출처를 다는 법, 그리고 오래된 client에게 무엇을 보여 줄지 고르는 법.
---

많은 agent가 일을 맡깁니다. supervisor가 조사를 자식에게 넘깁니다. planner가 하위 작업을
나눠 줍니다. tool call 하나가 그 자체로 중첩된 agent이기도 합니다. frontend에는 그 모두가
event stream 하나로 도착합니다. 정보가 더 없으면 동시에 도는 researcher 셋이 글자 벽
하나로 그려집니다.

protocol의 답은 일부러 작습니다. event 하나하나에 그것을 만든 subagent를 **attribute**합니다.
36개 event type 중 24개에 있는 optional `subagentRunId`가 그것입니다. 그리고 subagent가
언제 시작하고 멈추는지를 `SUBAGENT_STARTED`, `SUBAGENT_FINISHED`, `SUBAGENT_ERROR`로
알립니다. subagent를 orchestrate하거나 schedule하거나 정의하지는 않습니다. 그것은 여러분의
몫으로 남습니다.

## id는 호출 한 번의 이름입니다

`subagentRunId`는 **호출 한 번**을 가리키는 불투명한 handle입니다. 같은 subagent를 두 번
돌리면 id도 둘입니다. 재사용되는 쪽은 `SubagentStartedEvent::name`이고, UI가 표시하는 것도
그것입니다. 최상위 run과의 대칭으로 기억하면 됩니다. `agentId`와 `runId`의 관계가 `name`과
`subagentRunId`의 관계입니다.

예외는 하나, 중단입니다. [아래](#중단과-이어-가기)에서 다룹니다. interrupt로 멈춘
subagent는 재개하는 run에서 자기 id를 다시 쓸 수 있습니다.

`subagentRunId`가 없는 event는 부모 agent의 것입니다. 그래서 이 field를 한 번도 쓰지 않는
stream은 subagent가 생기기 전과 똑같이 동작합니다. `RUN_STARTED`, `RUN_FINISHED`,
`RUN_ERROR`는 이 field를 실을 수 없습니다. run 전체를 서술하는 event이기 때문입니다.
`MESSAGES_SNAPSHOT`도 실을 수 없습니다. 그 안의 message가 각자 자기 것을 싣습니다.
`EventType::is_attributable`이 type마다 답해 줍니다.

## subagent는 scope입니다

`ctx.subagent(name)`은 새 id로 `SUBAGENT_STARTED`를 emit하고 handle을 돌려줍니다.
[step](/ag-ui-rust/ko/server/agent/#step으로-run-구간-묶기)처럼 이 handle은 run context로
deref됩니다. 그래서 message, tool call, reasoning, step, 그리고 또 다른 subagent가 모두 이
handle을 통해 열립니다. 그들이 emit하는 모든 것은 이 subagent의 id를 달고 나갑니다. handle이
drop되면 `SUBAGENT_FINISHED`가 success outcome으로 나갑니다. `?`가 만드는 이른 return에서도
정상 경로에서와 똑같이 나갑니다.

```rust
use ag_ui::{Event, EventType, RunAgentInput, RunOutcome};
use ag_ui::server::{Agent, Result, RunContext, run};
use futures_util::StreamExt;
use serde_json::json;

struct Supervisor;

impl Agent for Supervisor {
    type State = ();

    async fn run(&self, ctx: &mut RunContext<()>) -> Result<RunOutcome> {
        let mut planner = ctx.subagent("planner")?;
        planner.say("Two tasks: scope, then risks.")?;   // Deref를 거쳐, attribute된 채로
        {
            let mut estimator = planner.subagent("estimator")?;   // 중첩
            estimator.say("About a day each.")?;
        }                                                          // SUBAGENT_FINISHED
        planner.finish_with(json!({ "tasks": 2 }))?;

        ctx.say("Plan ready.")?;                                   // 부모 자신의 것
        Ok(RunOutcome::Success)
    }
}

#[tokio::main]
async fn main() {
    let events: Vec<Event> = run(Supervisor, RunAgentInput::new("t", "run-1"))
        .map(|event| event.expect("the stream should not break"))
        .collect()
        .await;

    let types: Vec<EventType> = events.iter().map(Event::event_type).collect();
    assert_eq!(
        types,
        [
            EventType::RunStarted,
            EventType::SubagentStarted,      // planner
            EventType::TextMessageStart,
            EventType::TextMessageContent,
            EventType::TextMessageEnd,
            EventType::SubagentStarted,      // estimator, planner 안에서
            EventType::TextMessageStart,
            EventType::TextMessageContent,
            EventType::TextMessageEnd,
            EventType::SubagentFinished,     // estimator
            EventType::SubagentFinished,     // planner
            EventType::TextMessageStart,
            EventType::TextMessageContent,
            EventType::TextMessageEnd,
            EventType::RunFinished,
        ]
    );

    // id는 다른 id처럼 파생됩니다. run id에 counter를 붙인 것입니다.
    let tag = |i: usize| events[i].subagent_run_id().map(|id| id.as_str());
    assert_eq!(tag(2), Some("run-1-sub-1"));
    assert_eq!(tag(6), Some("run-1-sub-2"));
    assert_eq!(tag(11), None);

    // 중첩된 announcement는 agent가 말하지 않아도 부모를 가리킵니다.
    let Event::SubagentStarted(estimator) = &events[5] else { unreachable!() };
    assert_eq!(estimator.parent_subagent_run_id.as_deref(), Some("run-1-sub-1"));
}
```

handle 자신의 method는 끝내는 방법들입니다. `finish()`와 `finish_with(result)`는 success
outcome으로 `SUBAGENT_FINISHED`를 emit합니다. 두 번째는 완료 payload를 싣습니다.
`RUN_FINISHED.result`의 subagent판입니다. `fail(message)`와 `fail_with_code(message, code)`는
`SUBAGENT_ERROR`를 emit합니다. `suspend(interrupt_ids)`는 아래의 멈춘 경우입니다. 이들 모두
자기가 닫는 subagent의 이름을 대되, 그 subagent에 attribute되지는 않습니다. terminator는
바깥 scope의 것이고, 나가는 순간 그 scope가 다시 유효해집니다.

`Drop`은 성공과 실패를 구분하지 못합니다. 그러니 신경 쓰는 error 경로에서는 직접 `fail`을
부르십시오. `?`가 풀려 나가며 그냥 drop된 handle도 subagent를 닫기는 합니다. success로
닫고, 그 뒤에 driver가 run에 대해 emit하는 `RUN_ERROR`가 따라옵니다.

### tag는 어디서 오는가

attribution은 handle이 아니라 event sink에 삽니다. subagent scope가 열려 있는 동안 sink는
tag 없이 도착하는 attributable event마다 tag를 답니다. scope 안에서 연 `MessageHandle`은
자기가 그 안에 있다는 것을 알 필요가 없습니다. agent가 직접 tag한 event는 그대로 둡니다.
이 field를 실을 수 없는 event type은 맨 채로 남습니다.

```rust
use ag_ui::{Event, RunAgentInput};
use ag_ui::server::RunContext;
use serde_json::json;

fn main() -> ag_ui::server::Result<()> {
    let (mut ctx, mut events) = RunContext::<()>::new(RunAgentInput::new("t", "r"))?;
    {
        let mut sub = ctx.subagent("researcher")?;
        sub.emit(Event::custom("mine", json!(1)))?;
        sub.emit(Event::custom("theirs", json!(2)).with_subagent_run_id("other"))?;
        sub.emit(Event::messages_snapshot(Vec::new()))?;
    }
    let events = events.drain();
    let tag = |i: usize| events[i].subagent_run_id().map(|id| id.as_str());

    assert_eq!(tag(1), Some("r-sub-1"));   // sink가 tag를 달았습니다
    assert_eq!(tag(2), Some("other"));     // 직접 단 tag는 존중됩니다
    assert_eq!(tag(3), None);              // MESSAGES_SNAPSHOT은 실을 수 없습니다
    Ok(())
}
```

`ctx.subagent_run_id()`는 지금 유효한 scope를 알려 줍니다. `ctx.new_subagent_run_id()`는
scope를 열지 않고 id만 발급합니다. 아래의 동시 실행 경우를 위한 것입니다.

## tool로서의 agent

tool call이 곧 자식 agent라면, UI가 tool-call card 안에 그릴 수 있도록 link를 달아
subagent를 announce하십시오. `parent_tool_call_id`와, call이 message 안에 있다면
`parent_message_id`입니다. `ctx.subagent_with`는 여러분이 만든 announcement를 받습니다.
비워 둔 `parent_subagent_run_id`는 여전히 바깥 scope에서 채웁니다.

순서가 중요합니다. client는 call이 닫히는 것을 보고, 그 call이 낳은 subagent를 보고, 그 다음
call의 result를 봅니다. 그러니 subagent를 열기 전에 call을 끝내고, subagent가 끝난 뒤에
result를 emit하십시오.

```rust
use ag_ui::{Event, EventType, RunAgentInput, SubagentStartedEvent};
use ag_ui::server::RunContext;

fn main() -> ag_ui::server::Result<()> {
    let (mut ctx, mut events) = RunContext::<()>::new(RunAgentInput::new("t", "r"))?;

    let mut call = ctx.tool_call("task")?;
    call.args(r#"{"brief":"find sources"}"#)?;
    let (call_id, result_id) = (call.id().clone(), call.result_message_id().clone());
    call.end()?;

    let announce = SubagentStartedEvent::new("researcher-7", "researcher")
        .with_description("Finds and ranks sources")
        .with_parent_tool_call(call_id.clone());
    let mut researcher = ctx.subagent_with(announce)?;
    researcher.say("Three sources found.")?;
    researcher.finish_with(serde_json::json!({ "sources": 3 }))?;

    ctx.emit(Event::tool_call_result(result_id, call_id, "3 sources"))?;

    let types: Vec<EventType> = events.drain().iter().map(Event::event_type).collect();
    assert_eq!(types[2], EventType::ToolCallEnd);
    assert_eq!(types[3], EventType::SubagentStarted);
    assert_eq!(types[7], EventType::SubagentFinished);
    assert_eq!(types[8], EventType::ToolCallResult);
    Ok(())
}
```

여기처럼 id를 직접 정하는 것은 재개하는 run이 subagent를 이어 가는 방법이기도 합니다. 다음
절이 그 이야기입니다.

## 중단과 이어 가기

사람이 필요한 쪽이 subagent일 수 있습니다. run은 멈춘 run이 늘 끝나는 방식 그대로
끝납니다. interrupt outcome을 실은 `RUN_FINISHED`, 닫힌 연결, 열어 둔 것 없음. 시작된
subagent는 전부 `RUN_FINISHED` 전에 닫혀야 하므로 멈춘 subagent도 닫힙니다. 다만 success가
아니라 **suspended** outcome으로 닫힙니다. 자기가 소유한 interrupt의 이름을 대면서요. 그래서
client는 "done"이 아니라 "waiting"을 보여 줄 수 있습니다. interrupt는
`Interrupt::with_subagent_run_id`로 만드십시오. 그래야 subagent의 group 안에 그려집니다.

재개하는 run에서는 **같은 id**를 다시 announce하십시오. consumer는 그것을 두 번째
subagent가 아니라 이어 가기로 읽습니다. group이 waiting에서 running으로 돌아갑니다.

```rust
use ag_ui::{Event, EventType, Interrupt, ResumeEntry, RunAgentInput, RunOutcome, SubagentStartedEvent};
use ag_ui::server::{Agent, Result, RunContext, run};
use futures_util::StreamExt;
use serde_json::json;

const APPROVE: &str = "approve-delete";
const DELETER: &str = "deleter-1";

struct Janitor;

impl Agent for Janitor {
    type State = ();

    async fn run(&self, ctx: &mut RunContext<()>) -> Result<RunOutcome> {
        let approved = ctx.resume_for(APPROVE).is_some();

        // 두 run 모두 같은 id입니다. 새 id를 쓰면 subagent가 하나 더 그려집니다.
        let mut deleter = ctx.subagent_with(SubagentStartedEvent::new(DELETER, "deleter"))?;
        if approved {
            deleter.say("Deleted.")?;
            deleter.finish()?;
            return Ok(RunOutcome::Success);
        }

        deleter.say("This cannot be undone. May I?")?;
        let interrupt = Interrupt::new(APPROVE, "tool_approval").with_subagent_run_id(DELETER);
        deleter.suspend(vec![interrupt.id.clone()])?;
        Ok(RunOutcome::interrupt(vec![interrupt]))
    }
}

#[tokio::main]
async fn main() {
    let paused: Vec<Event> = run(Janitor, RunAgentInput::new("t", "run-1"))
        .map(|event| event.expect("the stream should not break"))
        .collect()
        .await;

    let Event::SubagentFinished(finished) = &paused[5] else { unreachable!() };
    let outcome = finished.outcome.as_ref().expect("an outcome");
    assert!(outcome.is_suspended());
    assert_eq!(outcome.interrupt_ids(), [APPROVE]);

    let Event::RunFinished(end) = &paused[6] else { unreachable!() };
    let interrupts = end.outcome.as_ref().expect("an outcome").interrupts();
    assert_eq!(interrupts[0].subagent_run_id.as_deref(), Some(DELETER));

    let mut input = RunAgentInput::new("t", "run-2");
    input.resume = Some(vec![ResumeEntry::resolved(APPROVE, json!(true))]);
    let resumed: Vec<Event> = run(Janitor, input)
        .map(|event| event.expect("the stream should not break"))
        .collect()
        .await;

    let Event::SubagentStarted(again) = &resumed[1] else { unreachable!() };
    assert_eq!(again.subagent_run_id.as_str(), DELETER);
    assert_eq!(resumed.last().map(Event::event_type), Some(EventType::RunFinished));
}
```

멈춘 subagent의 조상도 suspended가 됩니다. interrupt 목록은 비어 있습니다. 자기 interrupt는
없고, interrupt를 가진 자손을 기다릴 뿐입니다. 두 경우 모두 `SubagentOutcome`으로 읽습니다.
조상에서는 `interrupt_ids()`가 비어 있습니다.

## 직접 하는 동시 실행

handle 둘을 동시에 열 수는 없습니다. 두 번째 `subagent()`는 borrow check error입니다. 이
SDK에서 겹치는 것은 모두 그렇습니다. 정말로 동시에 stream하는 subagent들은
[병렬 tool call](/ag-ui-rust/ko/server/tools/)과 같은 상황입니다. subagent마다 event에
`Event::with_subagent_run_id`로 직접 tag를 달고, 뒤섞인 그대로 emit하십시오. lifecycle
event로 감싸서요.

```rust
use ag_ui::{Event, RunAgentInput, TextMessageRole};
use ag_ui::server::RunContext;

fn main() -> ag_ui::server::Result<()> {
    let (mut ctx, _events) = RunContext::<()>::new(RunAgentInput::new("t", "r"))?;
    let (a, b) = (ctx.new_subagent_run_id(), ctx.new_subagent_run_id());
    let role = TextMessageRole::Assistant;

    for event in [
        Event::subagent_started(a.clone(), "researcher"),
        Event::subagent_started(b.clone(), "researcher"),
        Event::text_message_start("m1", role).with_subagent_run_id(a.clone()),
        Event::text_message_start("m2", role).with_subagent_run_id(b.clone()),
        Event::text_message_content("m1", "GDP is ").with_subagent_run_id(a.clone()),
        Event::text_message_content("m2", "Population is ").with_subagent_run_id(b.clone()),
        Event::text_message_end("m1").with_subagent_run_id(a.clone()),
        Event::subagent_finished_success(a),
        Event::text_message_end("m2").with_subagent_run_id(b.clone()),
        Event::subagent_error(b, "rate limited"),
    ] {
        ctx.emit(event)?;   // verifier는 뒤섞임을 받아들입니다
    }
    Ok(())
}
```

verifier는 모든 entity를 id로 색인하고 누가 열었는지 기억합니다. 그래서 뒤섞임은
통과합니다. 거부하는 것은 entity를 연 subagent와 *다른* subagent의 이름을 대는 continuation,
terminator, 재열기입니다. `Rule::OwnerMismatch`, 여덟 번째 규칙이고, subagent가 server에
더한 유일한 규칙입니다. 아무도 지목하지 않는 continuation은 받아들입니다. attribution은
event마다 optional이고, 맨 continuation은 subagent 이전의 producer가 보내는 것이기
때문입니다. 그렇다고 entity를 부모에게 넘기지도 않습니다. 처음 쓴 쪽이 owner로 남습니다.
upstream이 기록하는 방식 그대로입니다. step은 이름과 함께 owner로도 색인됩니다. 그래서 subagent는 부모의 step을 닫을 수
없고, 두 agent가 같은 이름의 step을 동시에 돌릴 수 있습니다.

subagent 여럿이 동시에 stream할 때는 chunk마다 tag를 다십시오. message도 subagent도
지목하지 않는 `*_CHUNK` event는 소비하는 쪽에서 stream이 하나만 열려 있을 때에만 풀 수
있습니다.

## 오래된 client에게 보이는 것

attribution은 덧붙는 것이고 안전합니다. 그 이전의 client는 모르는 *field*를 보고
무시합니다. lifecycle event 셋은 그 client에게 모르는 *event type*입니다. 모르는 event
type은 application code가 돌기도 전에 decode 단계에서 실패합니다. 소비하는 쪽에서는
[의도된 설계](/ag-ui-rust/ko/client/updates/#이-build가-모르는-event)이고, producer가 나중에
고칠 수 있는 일이 아닙니다. `@ag-ui/client` 0.0.59보다 오래된 consumer를 둔 producer는
이들을 보내면 안 됩니다. `SubagentVisibility`가 그 방법입니다.

| mode | wire에서 |
| --- | --- |
| `Attributed` | **기본값이고, transformer가 아예 없는 상태.** lifecycle event, 그리고 subagent가 만든 모든 것에 붙은 `subagentRunId`. |
| `Inline` | subagent 이전의 모양. lifecycle event도 없고 `subagentRunId`도 어디에도 없습니다. event에도, `MESSAGES_SNAPSHOT`이나 `RUN_STARTED`의 input echo 안 message에도, 멈춘 run이 보고하는 interrupt에도 없습니다. subagent의 text가 부모의 일로 도착합니다. 이 모양이 표현할 수 없는 것은 거부됩니다. 부모의 열린 step과 같은 이름의 subagent step은 평평해지면 중복이고, verifier가 run을 끝냅니다. |
| `Hidden` | 부모 자신의 event만. subagent가 만든 것은 전부 버립니다. subagent가 요청한 call의 result도, 부모가 실행했더라도 버립니다. consumer가 본 적 없는 call의 result는 protocol error이기 때문입니다. 반대도 성립합니다. 부모의 call에 답하는 result는 누가 실행했든 tag를 지운 채 남깁니다. 또 하나의 예외는 run의 공유 state입니다. subagent가 publish한 `STATE_*` event는 tag를 지운 채 그대로 내보냅니다. 그것을 놓친 client는 다음 request에 낡은 state를 되돌려 보내기 때문입니다. |

둘 다 평범한 [transformer](/ag-ui-rust/ko/server/axum/)입니다. chain의 나머지와 함께
compose되고, endpoint마다 정합니다.

```rust
use ag_ui::{Event, EventType, RunAgentInput, RunOutcome};
use ag_ui::server::{Agent, Result, RunContext, Runner, SubagentVisibility};
use futures_util::StreamExt;

struct Delegating;

impl Agent for Delegating {
    type State = ();

    async fn run(&self, ctx: &mut RunContext<()>) -> Result<RunOutcome> {
        ctx.say("parent first")?;
        {
            let mut researcher = ctx.subagent("researcher")?;
            researcher.say("child")?;
        }
        ctx.say("parent last")?;
        Ok(RunOutcome::Success)
    }
}

#[tokio::main]
async fn main() {
    let inline: Vec<Event> = Runner::new(Delegating)
        .transformer(SubagentVisibility::inline())
        .run(RunAgentInput::new("t", "r"))
        .map(|event| event.expect("the stream should not break"))
        .collect()
        .await;

    // message 셋, wire의 무엇도 subagent를 말하지 않습니다.
    assert!(inline.iter().all(|e| !matches!(
        e.event_type(),
        EventType::SubagentStarted | EventType::SubagentFinished | EventType::SubagentError
    )));
    assert!(inline.iter().all(|e| e.subagent_run_id().is_none()));
    assert_eq!(inline.iter().filter(|e| e.event_type() == EventType::TextMessageEnd).count(), 3);

    let hidden: Vec<Event> = Runner::new(Delegating)
        .transformer(SubagentVisibility::hidden())
        .run(RunAgentInput::new("t", "r"))
        .map(|event| event.expect("the stream should not break"))
        .collect()
        .await;

    // 둘, 부모 자신의 것.
    assert_eq!(hidden.iter().filter(|e| e.event_type() == EventType::TextMessageEnd).count(), 2);
}
```

HTTP에서는 같은 선택이 `AgentEndpoint::transformer(|| SubagentVisibility::inline())`입니다.
closure인 이유는 transformer가 state machine이라서입니다. endpoint는 run마다 새 chain을
만듭니다.

:::note[기본값이 inline이 아닌 이유]
upstream의 integration은 inline 모양을 기본으로 하고 전체 surface를 opt-in으로 둡니다. 이
crate는 반대로 합니다. stream을 고쳐 쓰는 transformer는 여기서 다른 모든 transformer처럼
opt-in입니다. `ctx.subagent(..)`를 쓴 agent는 그럴 뜻이 있었던 것이고, 그 말을 조용히
평평하게 펴는 것은 [설계 원칙](/ag-ui-rust/ko/design/commitments/)이 반대하는 종류의
놀라움입니다. consumer가 오래되었다면 endpoint마다 뒤집으십시오.
:::

## API

- [`RunContext::subagent`](/ag-ui-rust/api/ag_ui/server/struct.RunContext.html#method.subagent),
  [`subagent_with`](/ag-ui-rust/api/ag_ui/server/struct.RunContext.html#method.subagent_with),
  [`new_subagent_run_id`](/ag-ui-rust/api/ag_ui/server/struct.RunContext.html#method.new_subagent_run_id)
- [`ag_ui::server::SubagentHandle`](/ag-ui-rust/api/ag_ui/server/struct.SubagentHandle.html)
- [`ag_ui::server::SubagentVisibility`](/ag-ui-rust/api/ag_ui/server/enum.SubagentVisibility.html)과
  [`SubagentFilter`](/ag-ui-rust/api/ag_ui/server/struct.SubagentFilter.html)
- [`ag_ui::SubagentStartedEvent`](/ag-ui-rust/api/ag_ui/struct.SubagentStartedEvent.html),
  [`SubagentFinishedEvent`](/ag-ui-rust/api/ag_ui/struct.SubagentFinishedEvent.html),
  [`SubagentErrorEvent`](/ag-ui-rust/api/ag_ui/struct.SubagentErrorEvent.html),
  [`SubagentOutcome`](/ag-ui-rust/api/ag_ui/enum.SubagentOutcome.html)
- [`Event::subagent_run_id`](/ag-ui-rust/api/ag_ui/event/enum.Event.html#method.subagent_run_id)와
  [`EventType::is_attributable`](/ag-ui-rust/api/ag_ui/event/enum.EventType.html#method.is_attributable)
- 소비하는 쪽 절반: [update stream](/ag-ui-rust/ko/client/updates/#subagent)
