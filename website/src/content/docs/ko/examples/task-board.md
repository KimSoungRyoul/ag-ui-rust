---
title: task-board (agent)
description: 끝까지 만들어 본 agent 예제를 읽어 나갑니다. streaming text, tool call, shared state, A2UI surface, 그리고 사람을 기다리는 정지.
---

`task-board`는 AG-UI로 말하는 workshop 작업 board입니다. 직접 띄우는 agent와 그것과
대화하는 terminal이 crate 하나에 들어 있습니다. 이 SDK의 첫 외부 소비자가 되려고
존재합니다. `ag_ui::server`, `ag_ui::client`, `ag_ui::axum`, `ag-ui-a2ui`, `ag-ui`를
남들과 똑같이 씁니다. 공개 항목만 쓰고, 안으로 손을 뻗지 않습니다. 주요 subsystem이
하나도 빠짐없이 정확히 한 번씩 나옵니다.

[GitHub에서 source 읽기](https://github.com/KimSoungRyoul/ag-ui-rust/tree/main/examples/task-board).

| subsystem | 어디에 나오는가 |
| --- | --- |
| streaming text | 답변. `TEXT_MESSAGE_CONTENT` 하나에 단어 하나씩 |
| reasoning | `ctx.think()`. agent가 message를 어떻게 읽었는지 한 줄 |
| tool call | `add_task`, `complete_task`, `estimate`, `clear_board`. agent가 실행합니다 |
| shared state | board. `STATE_SNAPSHOT`으로, 그다음에는 `STATE_DELTA`로 publish됩니다 |
| A2UI | surface로 표현한 board. `a2ui_operations` tool 결과 봉투에 담깁니다 |
| human in the loop | `clear`가 run을 멈추고 승낙을 기다립니다 |
| step | turn 전체가 `STEP_STARTED` / `STEP_FINISHED`로 감싸집니다 |

## 실행하기

terminal 둘. agent를 먼저 8080 port에:

```sh
cargo run -p task-board -- serve
```

그다음 client:

```sh
cargo run -p task-board -- chat
```

`add draft the agenda, book the room`을 치고, 이어서 `list`, `complete 1`, `clear`를
쳐 보세요. `quit`이나 Ctrl-D로 끝납니다. flag는 `--port`, `--url`, `--thread`입니다.
pipe도 됩니다. test가 이것을 굴리는 방식이 그것입니다:

```sh
printf 'add draft the agenda, book the room\nlist\n' | cargo run -p task-board -- chat
```

한 turn은 이렇게 생겼습니다:

```text
you> add draft the agenda, book the room
  ~ adding 2 task(s)
  · add_task({"title":"draft the agenda"})
  [state] 1 open · 0 done
    → {"id":1,"title":"draft the agenda"}
  · add_task({"title":"book the room"})
  [state] 2 open · 0 done
    → {"id":2,"title":"book the room"}
  agent> Added #1 draft the agenda, #2 book the room. 2 open · 0 done
  · render_a2ui({"surfaceId":"task-board"})
    ┌ a2ui surface
    │ Workshop board
    │ 2 open · 0 done
    │ [ ] #1 draft the agenda
    │ [ ] #2 book the room
    └
```

`~`는 reasoning, `·`는 tool call, `→`는 그 결과입니다. `[state]`는 state event 이후의
board입니다. 상자는 A2UI surface입니다. component tree를 걸으며 binding을 하나씩 풀어
그렸습니다. terminal이 rendering에 정직하게 다가갈 수 있는 한계입니다.

이 agent는 **결정적입니다**. board가 움직이는 것은 model이 tool을 부르기로 해서가
아니라 누군가 `add`를 쳤기 때문입니다. 그래서 위 기록을 글자 단위로 단언할 수 있습니다.
`tests/flows.rs`는 binary가 실행하는 것과 같은 `converse` 함수를 굴립니다. keyboard
대신 미리 짜 둔 `&[u8]`을, 화면 대신 `Vec<u8>`을 씁니다. 그래서 저 기록은 예시가 아니라
단언입니다.

## 왜 state가 tool call 한가운데 떨어지는가

위 기록을 다시 보세요. `[state]`가 call과 그 결과 *사이*에 나옵니다. renderer가 만든
모습이 아닙니다. agent가 일을 하는 자리가 거기입니다.

`ctx.tool_call(…)`이 돌려주는 handle은 event sink뿐 아니라 run state에도 닿습니다.
그래서 call이 열려 있는 채로 변경이 일어납니다. `TOOL_CALL_START`, 인자, `STATE_*`
event, 그다음 `TOOL_CALL_END`와 결과 순입니다. state event는 순서가 없으므로 protocol이
이를 허용합니다. client가 끝난 call만 보는 대신 *진행 중*인 call을 보여 줄 수 있는
근거이기도 합니다:

```rust
use ag_ui::{Event, EventType, RunAgentInput, RunOutcome};
use ag_ui::server::{Agent, Result, RunContext, run};
use futures_util::StreamExt;
use serde::{Deserialize, Serialize};
use serde_json::json;

#[derive(Default, Serialize, Deserialize)]
struct Board {
    tasks: Vec<String>,
}

struct Adder;

impl Agent for Adder {
    type State = Board;

    async fn run(&self, ctx: &mut RunContext<Board>) -> Result<RunOutcome> {
        let mut call = ctx.tool_call("add_task")?;
        call.args_json(&json!({ "title": "draft the agenda" }))?;

        // call이 아직 열려 있는 동안 board가 움직입니다.
        call.state_mut().tasks.push("draft the agenda".into());
        call.publish_state()?;

        call.result_json(&json!({ "id": 1 }))?;

        Ok(RunOutcome::Success)
    }
}

#[tokio::main]
async fn main() {
    let types: Vec<EventType> = run(Adder, RunAgentInput::new("thread-1", "run-1"))
        .map(|event| event.expect("the stream should not break").event_type())
        .collect()
        .await;

    assert_eq!(
        types,
        [
            EventType::RunStarted,
            EventType::ToolCallStart,
            EventType::ToolCallArgs,
            EventType::StateSnapshot,
            EventType::ToolCallEnd,
            EventType::ToolCallResult,
            EventType::RunFinished,
        ]
    );
}
```

emitter API의 이전 초안은 event sink만 들고 있었습니다. 그러면 무언가 열려 있는 동안
내내 state에 닿을 수 없습니다. 모든 agent가 call을 알리기 *전에* 먼저 변경해야 했습니다.
같은 event, 다른 순서입니다. client가 call이 자리 잡는 것을 지켜볼 수 있는지는 그
순서가 정합니다.

값은 소비자 쪽에 떨어집니다. `Update::State`는 자기가 도착한 시점의 call과 아무 연관이
없습니다. wire도 그 연관을 싣지 않기 때문입니다. `board-watch`가 그것을 진지하게
받아들인 예제입니다. [board-watch](/ag-ui-rust/ko/examples/board-watch/)를 보세요.

## snapshot이냐 delta냐, publish마다 결정

두 번 추가하면 두 번 publish합니다. client가 둘 다 같은 `Board`에 적용했으므로, 기록만
봐서는 각각이 wire에서 무엇이었는지 알 수 없습니다. server가 매번 정합니다. 첫 publish는
언제나 `STATE_SNAPSHOT`입니다. 뒤의 것은 *patch가 그것이 서술하는 state보다 작아지지
않는 한* `STATE_DELTA`입니다.

이만큼 작은 board에서는 작아지지 않습니다. 짧은 task 둘을 다시 보내는 값이 하나를
추가하는 RFC 6902 patch보다 쌉니다. 그래서 둘 다 snapshot으로 나갑니다. task 제목을
현실적인 길이로 바꾸면 두 번째가 delta가 됩니다:

```text
STATE_SNAPSHOT {"tasks":[{"id":1,"title":"write the workshop agenda and circulate it",…}],"nextId":1}
STATE_DELTA    [{"op":"add","path":"/tasks/1","value":{"id":2,…}},{"op":"replace","path":"/nextId",…}]
```

두 encoding 모두 test가 못 박습니다. 여기서 "동작한다"는 어느 쪽이든 client가 같은
자리에 도착한다는 뜻이기 때문입니다.
[shared state](/ag-ui-rust/ko/server/state/)가 그 reference입니다.

## human in the loop, 두 번의 request에 걸쳐

`clear`는 유일하게 파괴적인 명령입니다. 그래서 agent가 멈춰 서서 묻습니다. 답은 **두
번째** request를 타고 옵니다:

```text
you> clear
  ~ clearing cannot be undone, so a human decides
  agent> Clearing drops 1 task(s) and cannot be undone.
  ?? Clear the board? 1 task(s) will be removed.
  [y/N] y
  ~ a human approved clearing the board
  · clear_board({})
  [state] nothing on the board
    → {"removed":1}
  agent> Cleared 1 task(s). The board is empty.
```

첫 run은 interrupt outcome과 함께 `RUN_FINISHED`로 끝납니다. client가 답을 모아
`resume`에 실어 되돌립니다. agent가 `ctx.resume_for(…)`로 읽고 이어 갑니다. 거절한
경로도 같은 code에 `ResumeStatus::Cancelled`로 닿고, tool call을 하나도 하지 않습니다.
test가 단언하는 것이 그것입니다. `tests/flows.rs`의
`a_paused_run_ends_as_interrupted_and_resumes_as_its_own_run`입니다.

[human in the loop](/ag-ui-rust/ko/server/interrupts/)에 그 작동 방식이 있습니다.

## board는 어디에 사는가

**client에.** agent는 run과 run 사이에 아무것도 저장하지 않습니다.
`RunAgentInput.state`에서 board를 읽고, 바꾼 것을 publish하고, 잊습니다. 그것을 한
run에서 다음 run으로 나르는 것은 대화와 함께 `Session`입니다.

이 모양 위에 무언가를 쌓기 전에 흡수해 둘 것이 둘 있습니다:

- 같은 thread id로 합류한 두 번째 `chat` process는 **빈 board**에서 시작합니다. thread
  id는 대화의 이름이지 대화를 가져오지 않습니다. thread를 남기는 것은 application의
  일입니다. 이 예제는 그것을 하지 않습니다.
- agent가 surface가 이미 화면에 있는지 아는 것은 client가 이력을 되돌려 보내기
  때문입니다. `find_prior_surface`가 대화에 이미 있는 A2UI operation을 다시 훑습니다.
  그래서 두 번째 render가 두 번째 `createSurface`가 아니라 `updateComponents`가 됩니다.
  [surface 작성](/ag-ui-rust/ko/a2ui/authoring/)이 그것을 다룹니다.

## model에게 말을 맡기기

```sh
export AG_UI_LLM_API_KEY=…        # or GEMINI_API_KEY
cargo run -p task-board -- serve --llm
```

또는 키 없이, 각자의 machine에서 도는 model을 상대로:

```sh
export AG_UI_LLM_BASE_URL=http://localhost:11434/v1
export AG_UI_LLM_MODEL=qwen3:4b
cargo run -p task-board -- serve --llm
```

model은 답변 문장만 다시 씁니다. 그 밖에는 아무것도 건드리지 않습니다. id와 개수와
state 전이는 결정적으로 남습니다. model이 실패해도 run이 실패하지는 않습니다. 미리 짜 둔
문장이 나가고, 실패는 reasoning으로 보고됩니다.

dependency tree에 LLM crate는 없습니다. `src/llm.rs`는 `reqwest`와 `serde` struct
둘입니다. 지름길이 아니라 설계 결정의 증명입니다. 이 SDK는 어떤 model client에도
의존하지 않습니다. model client가 필요한 예제라면 그 설계를 스스로 반박하는 셈입니다.

## code

| 파일 | 무엇이 들어 있는가 |
| --- | --- |
| `src/board.rs` | `Board`와 `Task`, tool schema 넷, A2UI surface. AG-UI event는 전혀 모릅니다. |
| `src/agent.rs` | `impl Agent`와 명령 parser |
| `src/chat.rs` | terminal client. 입력과 출력에 대해 generic합니다 |
| `src/llm.rs` | 선택인 `--llm` 문장 다듬기 |
| `src/main.rs` | CLI. 인자 parsing은 손으로 |
| `tests/flows.rs` | 실제 port 위의 server를 상대로 한 세 흐름 전부 |

먼저 읽을 것은 `src/board.rs`입니다. 도메인을 protocol에서 떼어 놓았습니다. state는
평범한 `serde` struct, tool은 `Tool` 정의, surface는 component tree입니다. 그 덕분에
`src/agent.rs`가 한자리에서 읽을 만큼 짧습니다.

`tests/flows.rs`의 마지막 test는 `Session` 아래 `HttpAgent`까지 내려갑니다. 한 run이
wire에 올리는 정확한 event 순서를 못 박습니다:

```sh
cargo test -p task-board
```

## 다음

- [board-watch](/ag-ui-rust/ko/examples/board-watch/) — 같은 protocol을 반대편에서.
  특정 agent를 상정하지 않고 쓴 client입니다.
- [Agent trait](/ag-ui-rust/ko/server/agent/) — 이 예제가 하는 일의 reference.
- [tool call](/ag-ui-rust/ko/server/tools/)과
  [shared state](/ag-ui-rust/ko/server/state/) — 여기서 가장 많이 얽히는 subsystem 둘.
- [A2UI](/ag-ui-rust/ko/a2ui/) — surface, 그리고 그것을 그리는 데 필요한 것.
