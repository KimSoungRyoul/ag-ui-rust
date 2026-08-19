---
title: human in the loop
description: 사람을 기다리려고 run을 끝내는 법. 그리고 뒤따르는 요청에서 하던 일을 다시 집어 드는 법.
---

어떤 run은 혼자 힘으로 끝날 수 없습니다. 배포에는 승인이 필요합니다. 파괴적인 명령에는 확인이
필요합니다. 폼은 누군가 채워야 합니다.

AG-UI는 run이 *멈출* 수 있게 해서 이를 표현합니다. agent가 `Success` 대신
`RunOutcome::Interrupt`를 반환합니다. client가 답을 모읍니다. 두 번째 요청이 그 답을 실어
옵니다.

중요한 것은 그 멈춤이 wire에서 무엇이냐입니다. 그것은 **끝난 run**입니다. 평범한
`RUN_FINISHED` event이고, `outcome`이 `interrupt`라고 말하며 무엇이 대기 중인지 나열합니다.
연결은 닫힙니다. 열어 둔 것도 없습니다. 멈춤을 넘어 살아남는 server 쪽 session도 없습니다. 다음
요청은 답을 싣고 있을 뿐인, 같은 스레드의 평범한 요청입니다.

## 왕복 한 번

```rust
// src/agent.rs
use ag_ui::{Event, Interrupt, ResumeEntry, ResumeStatus, RunAgentInput, RunOutcome};
use ag_ui::serve::{Agent, Error, Result, RunContext, run};
use futures_util::StreamExt;
use serde_json::{Value, json};

const APPROVE_DEPLOY: &str = "approve-deploy";

struct Deployer;

impl Agent for Deployer {
    type State = ();

    async fn run(&self, ctx: &mut RunContext<()>) -> Result<RunOutcome> {
        // 아직 답이 없으면 이유를 말하고 멈춥니다.
        let Some(answer) = ctx.resume_for(APPROVE_DEPLOY) else {
            ctx.say("Deploying to production needs a human.")?;
            return Ok(RunOutcome::interrupt(vec![request()]));
        };

        match answer.status {
            ResumeStatus::Resolved => {
                // 페이로드를 읽어야 왕복을 무사히 건넜다는 것이 증명됩니다.
                // 비어서 도착한 답은 조용히 성공하지 않고 run을 실패시킵니다.
                let build = answer
                    .payload
                    .as_ref()
                    .and_then(|payload| payload.get("build"))
                    .and_then(Value::as_u64)
                    .ok_or_else(|| Error::agent("the approval carried no build number"))?;
                ctx.say(format!("Deployed build {build}."))?;
            }
            ResumeStatus::Cancelled => {
                ctx.say("Left production alone.")?;
            }
        }

        Ok(RunOutcome::Success)
    }
}

/// client에게 무엇을 묻는지, 그리고 그 답이 어떤 모양이어야 하는지.
fn request() -> Interrupt {
    Interrupt {
        id: APPROVE_DEPLOY.to_owned(),
        reason: "tool_approval".to_owned(),
        message: Some("Deploy build 42 to production?".to_owned()),
        ..Default::default()
    }
}

#[tokio::main]
async fn main() {
    // 첫 번째 차례: agent가 멈춥니다.
    let first: Vec<Event> = run(Deployer, RunAgentInput::new("deploy", "run-1"))
        .map(|event| event.expect("the stream should not break"))
        .collect()
        .await;

    let Some(Event::RunFinished(finished)) = first.last() else {
        panic!("a paused run still finishes: {first:?}");
    };
    assert_eq!(
        finished.outcome.as_ref().map(RunOutcome::interrupts),
        Some(&[request()][..])
    );

    // 두 번째 차례: 같은 스레드, 새 run id, 답을 실어서.
    let mut resumed = RunAgentInput::new("deploy", "run-2");
    resumed.resume = Some(vec![ResumeEntry::resolved(
        APPROVE_DEPLOY,
        json!({"build": 42}),
    )]);

    let second: Vec<Event> = run(Deployer, resumed)
        .map(|event| event.expect("the stream should not break"))
        .collect()
        .await;

    assert!(second.iter().any(|event| matches!(
        event,
        Event::TextMessageContent(content) if content.delta == "Deployed build 42."
    )));
}
```

## interrupt에 무엇을 담을 것인가

`Interrupt::new(id, reason)`은 필수 필드 두 개를 채웁니다. 나머지는 선택입니다. client가
여러분의 agent를 전혀 몰라도 질문을 그릴 수 있도록 있는 것들입니다.

| 필드 | 무엇을 위한 것인가 |
| --- | --- |
| `id` | 짝 맞추기. `ResumeEntry::interrupt_id`로 되돌아옵니다 |
| `reason` | 기계가 읽는 값. 예를 들면 `"tool_approval"` |
| `message` | 사용자에게 보여 줄 질문 |
| `tool_call_id` | interrupt가 어떤 call에 관한 것일 때, 승인을 기다리는 그 call |
| `response_schema` | 답이 만족해야 할 JSON Schema. client가 폼을 그릴 수 있게 해 줍니다 |
| `expires_at` | 질문에 더 이상 답할 수 없게 되는 시점, ISO-8601 |
| `metadata` | 연동마다 필요한 부가 정보 |

답이 예/아니오 이상이라면 `response_schema`를 채워 둘 값어치가 있습니다.

```rust
use ag_ui::{Interrupt, JsonObject};
use serde_json::json;

fn confirm_clear(count: usize) -> Interrupt {
    let mut schema = JsonObject::new();
    schema.insert("type".to_owned(), json!("object"));
    schema.insert(
        "properties".to_owned(),
        json!({"confirm": {"type": "boolean"}}),
    );
    schema.insert("required".to_owned(), json!(["confirm"]));

    Interrupt {
        id: "confirm-clear".to_owned(),
        reason: "tool_approval".to_owned(),
        message: Some(format!("Clear the board? {count} task(s) will be removed.")),
        response_schema: Some(schema),
        ..Default::default()
    }
}

fn main() {
    let interrupt = confirm_clear(3);
    assert_eq!(interrupt.id, "confirm-clear");
    assert!(interrupt.response_schema.is_some());
}
```

## 답을 읽기

재개된 요청에 관한 모든 것은 context 위에 있습니다.

| 메서드 | 무엇을 답하는가 |
| --- | --- |
| `is_resume()` | 이 요청이 멈춘 run을 재개하는 것이기는 한가 |
| `resume()` | 요청이 실어 온 모든 `ResumeEntry` |
| `resume_for(id)` | interrupt 하나에 대한 답, 없으면 `None` |

`ResumeEntry`에는 `status`와 선택적인 `payload`가 있습니다. `status`는 `Resolved` 아니면
`Cancelled`입니다.

두 상태는 성공과 실패가 아닙니다. *사용자의* 두 가지 답입니다. 취소된 interrupt는 사람이 거절한
것입니다. 그것을 읽은 run은 다른 갈래로 계속 나아가 성공적으로 끝나야 합니다. 답의 형식이
잘못되어 실패하는 run은 [error](/ag-ui-rust/ko/server/errors/)이고, client에는 `RUN_ERROR`로
도착합니다.

```rust
use ag_ui::{ResumeEntry, ResumeStatus, RunAgentInput};
use ag_ui::serve::RunContext;
use serde_json::json;

fn main() -> ag_ui::serve::Result<()> {
    let mut input = RunAgentInput::new("t", "r");
    input.resume = Some(vec![
        ResumeEntry::resolved("approve-deploy", json!({"build": 42})),
        ResumeEntry::cancelled("confirm-clear"),
    ]);
    let (ctx, _events) = RunContext::<()>::new(input)?;

    assert!(ctx.is_resume());
    assert_eq!(ctx.resume().len(), 2);
    assert_eq!(
        ctx.resume_for("approve-deploy").map(|entry| entry.status),
        Some(ResumeStatus::Resolved)
    );
    assert_eq!(
        ctx.resume_for("confirm-clear").map(|entry| entry.status),
        Some(ResumeStatus::Cancelled)
    );
    assert!(ctx.resume_for("something-else").is_none());
    Ok(())
}
```

## agent는 아무것도 기억하지 않습니다

여기서 걸려 넘어지는 사람이 많습니다. 멈춘 run은 사라졌습니다. stream이 끝날 때 그 future가
드롭되었기 때문입니다. 그래서 재개된 run은 맨바닥에서 시작합니다. `messages`, `state`,
`resume`으로 자기 위치를 다시 세웁니다. *이번 요청*이 실어 온 답만 존재합니다.

그 결과는 run이 결정 두 개 이상에서 멈추는 순간 드러납니다. client가 요청 하나에 interrupt
하나씩 답한다고 해 봅시다. 요청마다 agent가 알게 되는 답은 정확히 하나입니다. agent는 그 요청이
언급하지 않은 나머지에서 다시 멈춥니다. 영원히 끝나지 않습니다.

`e2e/tests/human_in_the_loop.rs`가 이것을 못 박아 둡니다. 예산에 답하고 그다음 날짜에 답하면
예산이 다시 미답 상태가 됩니다. 둘을 한 요청에 함께 보내야만 run이 끝납니다.

그러므로 여러 가지에 대해 한꺼번에 멈추는 agent는 아직 남은 것들을 모두 보고해야 합니다.
client는 그것들에 한꺼번에 답해야 합니다.

```rust
use ag_ui::{Interrupt, RunOutcome};
use ag_ui::serve::{Agent, Result, RunContext};

const BUDGET: &str = "approve-budget";
const DATE: &str = "confirm-date";

struct Planner;

impl Agent for Planner {
    type State = ();

    async fn run(&self, ctx: &mut RunContext<()>) -> Result<RunOutcome> {
        let pending: Vec<Interrupt> = [BUDGET, DATE]
            .into_iter()
            .filter(|id| ctx.resume_for(id).is_none())
            .map(|id| Interrupt::new(id, "tool_approval"))
            .collect();

        if pending.is_empty() {
            ctx.say("Booked.")?;
            return Ok(RunOutcome::Success);
        }

        Ok(RunOutcome::interrupt(pending))
    }
}
```

`filter`가 무엇을 emit하기 전에 먼저 돈다는 점을 눈여겨보십시오. `resume_for`는 context를
불변으로 빌리는데 emitter들은 가변으로 원합니다. 그래서 남은 결정을 먼저 읽어 두는 것은 단정한
정도가 아닙니다. borrow가 맞아떨어지게 하는 조건입니다.

## 타입 시스템이 잡아 주지 않는 규칙 하나

`RunOutcome::Interrupt`는 빈 목록으로도 만들 수 있는데, protocol은 그것을 금지합니다. driver가
emit 전에 outcome을 검사합니다. 그래서 빈 interrupt 목록은 `RUN_FINISHED`가 아니라 `PROTOCOL`
code를 단 `RUN_ERROR`가 됩니다. client가 아무것도 할 수 없는 `RUN_FINISHED` 대신 말입니다.

```rust
use ag_ui::RunOutcome;

fn main() {
    assert!(RunOutcome::Success.validate().is_ok());
    assert!(RunOutcome::interrupt(vec![]).validate().is_err());
}
```

역직렬화도 이를 강제하지 않습니다. 이것도 일부러 그렇습니다. 결함 있는 생산자가 보낸 엉뚱한 빈
배열은 로그로 남길 수 있는 protocol 오류로 드러납니다. stream을 죽이는 파싱 불가 event가
되지 않습니다.

## API

- [`ag_ui::RunOutcome`](/ag-ui-rust/api/ag_ui/enum.RunOutcome.html)
- [`ag_ui::Interrupt`](/ag-ui-rust/api/ag_ui/struct.Interrupt.html)
- [`ag_ui::ResumeEntry`](/ag-ui-rust/api/ag_ui/struct.ResumeEntry.html)와
  [`ResumeStatus`](/ag-ui-rust/api/ag_ui/enum.ResumeStatus.html)
- [`RunContext::resume_for`](/ag-ui-rust/api/ag_ui/serve/struct.RunContext.html#method.resume_for)
- 이 왕복의 client 쪽 절반: [update stream](/ag-ui-rust/ko/client/updates/)
