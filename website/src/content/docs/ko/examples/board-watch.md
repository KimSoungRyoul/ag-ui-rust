---
title: board-watch (client)
description: 실제 port로 task-board agent를 소비하는 terminal client를 읽어 나갑니다.
---

[task-board](/ag-ui-rust/ko/examples/task-board/)는 client를 붙인 agent였습니다.
`board-watch`는 반대입니다. 여기서 application은 **client**입니다. 같은 crate의
server는 살아남을 가치가 있는 stream을 그 client에게 던져 주려고만 있습니다.

그 뒤집기가 핵심입니다. 자기가 직접 쓴 agent 하나만 상대로 만든 client는 사실
아무것에도 시험되지 않습니다. 그래서 이 client는 특정 agent를 상정하지 않고 썼습니다.
아래 기록은 일부러 까다롭게 만든 backend를 상대로 남겼습니다. chunk로 오는 출력, 뒤섞인
병렬 call, 여러 결정에서 한꺼번에 멈춰 서기, 영영 끝나지 않는 run, 그리고 protocol이
금지하는 stream입니다.

[GitHub에서 source 읽기](https://github.com/KimSoungRyoul/ag-ui-rust/tree/main/examples/board-watch).

| 명령 | 무엇인가 |
| --- | --- |
| `watch` | application. 한 줄을 보내고, run을 그리고, 멈춰 서서 묻는 것에 답하고, board를 그립니다. |
| `trace` | 같은 대화를 한 단계 아래에서. event를 도착한 그대로. |
| `replay` | network를 빼낸 client 전체를, 기록해 둔 fixture 위에서. |
| `serve-fake` | 위 기록을 남긴, 그 까다로운 backend. |

키도, loopback 너머의 network도 필요 없습니다.

## 실행하기

terminal 둘. backend를 8090 port에:

```sh
cargo run -p board-watch -- serve-fake
```

그다음 그것을 가리키는 client:

```sh
cargo run -p board-watch -- watch --url http://127.0.0.1:8090/agent --fragments
```

scenario는 처음 친 단어로 고릅니다. 그래서 기록만 봐도 무엇을 시험했는지 압니다.
`chunks`, `call`, `parallel`, `mixed`, `approve`, `busy`, `slow`, `fail`입니다. 그 밖의
아무 말이나 치면 얌전한 turn이 됩니다.

`--fragments`는 도착하는 delta마다 괄호를 칩니다. client가 다시 이어 붙여야 했던 것이
출력에 그대로 보입니다. `--in-order`는 tool call을 한 줄로 모으는 대신 update 하나에 한
줄씩 그립니다. 이것이 진짜 거래로 드러나서, 아래에 절을 따로 두었습니다.

## chunk로 오는 stream

provider adapter는 자기 출력을 감싸지 못하는 일이 흔합니다. upstream API가 다음
message가 시작되기 전까지는 message가 끝났다고 말해 주지 않습니다. 그래서 `*_CHUNK`
event를 보냅니다. start와 content와 end를 하나로 접고, id는 **첫 번째에만** 싣습니다.
event 다섯, message 하나입니다:

```text
> chunks
  text   [Chunked text arrives in frag][ments, and the client rejoins ][them — emoji included: 👩][‍][💻.]
  done   success
```

그 id를 기억하는 것이 client가 가장 먼저 하는 일입니다. 다른 무엇이 stream을 보기
전입니다:

```rust
use ag_ui::client::chunks::normalize_all;
use ag_ui::{Event, EventType, MessageId};

let events = normalize_all([
    Event::text_message_chunk(Some(MessageId::new("msg-1")), Some("Hel".into())),
    Event::text_message_chunk(None, Some("lo".into())),
])
.unwrap();

let types: Vec<EventType> = events.iter().map(Event::event_type).collect();
assert_eq!(
    types,
    [
        EventType::TextMessageStart,
        EventType::TextMessageContent,
        EventType::TextMessageContent,
        EventType::TextMessageEnd,
    ]
);
```

위 기록의 마지막 fragment 셋은 ZWJ emoji를 그 부분들 사이에서 쪼갭니다. fragment
하나하나는 그 자체로 유효한 UTF-8입니다. Rust `String`이 그럴 수밖에 없습니다. 그래도
*grapheme*은 다시 이어 붙여야만 존재합니다.

## escape 한가운데서 잘린 인자

tool 인자는 더 고약합니다. 임의의 byte 위치에서 잘린 JSON이기 때문입니다:

```text
> call
  call   add_task [{"no][te":"line\][nbreak","ti][tle":"ship ][the SDK","depth":3}]
  result {"id":1,"title":"ship the SDK"}
```

`line\` 뒤의 이음매가 모든 adapter가 한 번은 틀리는 자리입니다. backslash와 그것이
escape하는 `n`이 서로 다른 event로 옵니다. fragment 하나만 따로 parse하면 잘못된 JSON을
봅니다. client가 건네는 것은 전체이고, 그것은 parse됩니다:

```rust
use ag_ui::client::{MessageChangeKind, Session, Update, transport::ReplayTransport};
use ag_ui::Event;
use futures_util::StreamExt;

#[tokio::main]
async fn main() {
    let transport = ReplayTransport::new([
        Event::run_started("thread-1", "run-1"),
        Event::tool_call_start("call-1", "add_task"),
        Event::tool_call_args("call-1", r#"{"no"#),
        Event::tool_call_args("call-1", r#"te":"line\"#),
        Event::tool_call_args("call-1", r#"nbreak","ti"#),
        Event::tool_call_args("call-1", r#"tle":"ship the SDK"}"#),
        Event::tool_call_end("call-1"),
        Event::run_finished_success("thread-1", "run-1"),
    ]);

    let mut session = Session::<_>::new(transport, "thread-1");
    let mut args = String::new();

    let mut run = session.send("call");
    while let Some(update) = run.next().await {
        if let Update::Message(message) = update {
            if let MessageChangeKind::ToolCallArgs { delta, .. } = message.change {
                args.push_str(&delta);
            }
        }
    }
    drop(run);

    // 위의 어떤 fragment도 홀로는 parse되지 않습니다. 전체는 됩니다.
    let parsed: serde_json::Value = serde_json::from_str(&args).unwrap();
    assert_eq!(parsed["note"], "line\nbreak");
    assert_eq!(parsed["title"], "ship the SDK");
}
```

## 동시에 두 call, 그리고 좋은 답이 없는 거래

한 번에 두 가지를 요청하는 model은 뒤섞인 event를 만듭니다.
`args(a) args(b) args(a) end(a) end(b)` 같은 모양입니다. `ToolCallStarted`에 접두사를
찍고 `ToolCallEnded`에 줄바꿈을 찍는 뻔한 renderer는 뒤엉킨 한 줄을 만듭니다. 이
client는 call id로 buffering합니다:

```text
> parallel
  call   add_task [{"title":]["write it down"}]
  call   add_task [{"title":]["read it back"}]
  result {"id":1,"title":"write it down"}
  result {"id":2,"title":"read it back"}
  state  2 open · 0 done
```

그 buffering에는 값이 붙습니다. call은 *닫힐 때* 출력됩니다. 그래서 call이 열려 있는
동안 agent가 emit한 것은 그 **앞에** 찍힙니다. `task-board`는 call 안에서 state를
publish합니다. 그래서 묶어 그리는 view는
[task-board](/ag-ui-rust/ko/examples/task-board/)가 애써 바로잡은 바로 그것을
뒤집습니다:

```text
  state  1 open · 0 done
  call   add_task {"title":"draft the agenda"}
  result {"id":1,"title":"draft the agenda"}
```

`--in-order`는 거래의 반대편을 택합니다. update 하나에 한 줄씩, 도착한 순서대로
그립니다. tool 관련 줄에는 각자의 call tag를 답니다:

```text
  call   add_task (1)
  args   (1) {"title":"draft the agenda"}
  state  1 open · 0 done
  end    add_task (1)
  result {"id":1,"title":"draft the agenda"}
```

wire가 그것을 놓은 자리가 거기입니다. 도착 순서가 *곧* 중첩입니다. `Update::State`는
자기가 도착한 시점의 call과 아무 연관이 없습니다. 병렬 call에서는 call 둘이 열려 있고,
wire도 그 state를 누구에게 돌리지 않습니다. 연관을 지어내는 것은 추측을 사실로 보고하는
일입니다.

그래서 가질 수 없는 것은 call을 한 줄로 그리면서 **동시에** 순서를 지키는 것입니다.
call이 닫히기 전에는 그 줄을 쓸 수 없습니다. 병렬 call에서의 가독성은 대신 id tag가
만듭니다. 대화를 읽으려면 묶어 그리는 view를, 하나를 debug하려면 `--in-order`를
고르세요.

## 하나 이상에서 멈춰 서기

run은 여러 결정에서 한꺼번에 멈춰 설 수 있습니다. 그것들은 **한 번의** request로
답합니다. request마다 하나씩 답하면 영영 끝나지 않습니다. agent는 재개하는 request가
싣고 온 것만 보기 때문입니다:

```text
> approve
  pause  approve-budget · Approve the budget?
  pause  confirm-date · Confirm the date?
  done   interrupted on 2
  approve approve-budget [y/N] y
  answer approve-budget · approved
  approve confirm-date [y/N] n
  answer confirm-date · declined
  text   Declined: confirm-date. Nothing booked.
  done   success
```

`--approve`와 `--decline`은 script를 위해 사람 없이 전부 답합니다.

run은 일을 *하고 나서* 멈춰 설 수도 있습니다. `busy` scenario는 task 둘을 추가하고
state를 publish한 다음에야 묻습니다. 재개할 때는 처음부터 다시 하는 대신 앞 절반이 만든
것을 싣고 갑니다. [human in the loop](/ag-ui-rust/ko/server/interrupts/)가 그
reference입니다.

## 멈추기

byte를 끌어오는 일이 곧 stream을 poll하는 일입니다. 그래서 그것을 놓아 버리는 것이
client 쪽 취소의 전부입니다. `--stop-after N`은 update N개 뒤에 run stream을
drop합니다:

```text
> slow
  text   working on it, this will take a while
  stop   dropped the stream after 3 updates
```

agent는 영영 끝나지 않을 call에 30초째 들어가 있습니다. drop은 거기까지 닿습니다.
integration test는 agent의 future가 빠져나온 시점에 그 run의 cancellation token이 이미
올라가 있었음을 단언합니다. session은 계속 씁니다. 다음 run은 여느 run과 같습니다. 그
반대편은 [error와 cancellation](/ag-ui-rust/ko/server/errors/)가 다룹니다.

## protocol이 금지하는 stream

`ag_ui::server`는 잘못된 stream을 emit하지 않습니다. 그것이 그 crate의 존재 이유입니다.
그래서 가짜 backend의 `/raw` endpoint는 다른 언어로 짠 producer가 그러듯
`SseFormatter`로 byte를 손수 감쌉니다. 그것을 잡아야 하는 것은 client 자신의
verifier입니다:

```text
$ board-watch watch --url http://127.0.0.1:8090/raw/unbracketed
> go
  error  protocol violation: TEXT_MESSAGE_CONTENT for message "ghost", which was never opened
  done   success
```

문제의 event는 적용되지 *않습니다*. 대화에는 사용자의 message 하나만 남습니다.
`--no-verify`를 주면 그래도 적용합니다. applier는 어느 쪽이든 너그럽습니다.
verification이 값을 치르고 사는 것은 대화가 아니라 진단입니다.

run이 여전히 `success`로 끝난다는 점도 보세요. `Update::Error`가 반드시 치명적인 것은
아닙니다. `Update::Done`만 읽는 client는 이것을 통째로 놓칩니다. 이 예제가 드러내려고
존재하는 종류의 일입니다. [verification](/ag-ui-rust/ko/design/verification/)에 그
규칙이 있습니다.

## 진짜 agent를 상대로

앞에서 본 `task-board`를 손대지 않은 채로 8080 port에:

```sh
cargo run -p task-board -- serve
cargo run -p board-watch -- watch --url http://127.0.0.1:8080/agent \
    --tools examples/board-watch/fixtures/task-board-tools.json
```

```text
> add draft the agenda, book the room
  think  adding 2 task(s)
  state  1 open · 0 done
  call   add_task {"title":"draft the agenda"}
  result {"id":1,"title":"draft the agenda"}
  state  2 open · 0 done
  call   add_task {"title":"book the room"}
  result {"id":2,"title":"book the room"}
  text   Added #1 draft the agenda, #2 book the room. 2 open · 0 done
  call   render_a2ui {"surfaceId":"task-board"}
  surface
    Workshop board
    2 open · 0 done
    [ ] #1 draft the agenda
    [ ] #2 book the room
  done   success
┌ board
│ 2 open · 0 done
│ [ ] #1 draft the agenda
│ [ ] #2 book the room
└ run board-run-1 · 8 messages · surface task-board (6)
```

저 panel에서 짚어 둘 것이 셋입니다. board는 이 client **자신의** view model입니다.
agent의 것과 무관하게 `src/board.rs`에 선언되어 있습니다. frontend 팀이 건네받는 것은
crate가 아니라 JSON 모양입니다. surface는 A2UI component tree를 걸으며 binding을 하나씩
풀어 그립니다. 그리고 `surface task-board (6)`은 마침 그것을 싣고 있던 tool 결과가
아니라 **대화**에서 되찾은 것입니다.

### `--tools`가 있는 이유

AG-UI에서는 *client*가 tool을 제안하고 agent가 그중에서 고릅니다. discovery는 없습니다.
agent가 받지 않은 tool을 달라고 할 방법이 없습니다. 하나도 받지 못한 agent는 그냥
실패합니다.

```text
$ board-watch watch --url http://127.0.0.1:8080/agent      # no --tools
> add anything
  think  adding 1 task(s)
  error  run failed: agent error: the client offered no add_task tool
  done   failed [AGENT_ERROR] agent error: the client offered no add_task tool
```

agent의 bug처럼 읽히지만 bug가 아닙니다. 특정 agent를 상정하지 않고 쓴 client는 URL을
설정하듯 tool 집합도 *설정*받아야 합니다. 함께 넣어 둔 fixture는 정확히 `task-board`가
제안하는 것입니다. 그것이 어긋나지 않았음을 test가 단언합니다.

## 한 단계 아래로, 그리고 offline으로

`trace`는 event를 조립하지 않은 채로 출력합니다. proxy나 recorder, 또는 stream을
debug하는 사람이 원하는 것입니다. session 없이 human in the loop 왕복도 해냅니다.
`interrupts_of`가 run이 무엇에서 멈춰 섰는지 읽습니다. `resume_run`이 그것에 답하는
request를 만듭니다.

```sh
cargo run -p board-watch -- trace --url http://127.0.0.1:8090/agent --approve approve
```

`Transport`는 trait입니다. disk 위의 fixture가 server를 대신해도 그 위의 무엇도 바뀌지
않습니다:

```sh
cargo run -p board-watch -- replay examples/board-watch/fixtures/chunked-run.json --fragments
```

기록을 새로 뜨려면 `serve-fake`를 띄운 채로 `fixtures/capture.py`를 돌리세요.

## 진짜 model을 상대로

선택입니다. 기본 경로에서는 아무것도 여기 닿지 않습니다. workspace의 LLM agent도 여느
것과 다름없는 AG-UI endpoint입니다. client는 손댈 것이 없습니다:

```sh
export GEMINI_API_KEY=…                    # or AG_UI_LLM_API_KEY
cargo run -p ag-ui-e2e --example llm_agent
cargo run -p board-watch -- watch --url http://127.0.0.1:8080/agent --fragments
```

`tests/live.rs`가 같은 일을 test로 합니다. `#[ignore]`가 붙어 있어 `cargo test`도 CI도
network를 건드리지 않습니다. 키가 없으면 실패하는 대신 건너뜁니다:

```sh
cargo test -p board-watch --test live -- --ignored --nocapture
```

그렇게 얻는 것은 fixture가 흉내 낼 수 없는 chunk 처리의 한 부분입니다. 이 crate가
생각하는 방식이 아니라 실제 provider가 자기 박자대로 쪼갠 delta입니다.

## code

| 파일 | 무엇이 들어 있는가 |
| --- | --- |
| `src/watch.rs` | driver와 renderer 둘. 입력과 출력에 대해 generic합니다 |
| `src/view.rs` | panel, A2UI 순회, 그리고 transport를 한정하지 않고 `Session`을 지칭하는 helper |
| `src/board.rs` | agent state에 대한 client 자신의 view model |
| `src/trace.rs` | 조립하지 않은 view, 그리고 session 없는 재개 |
| `src/fake.rs` | 까다로운 agent와, 손으로 감싼 불법 stream |
| `src/main.rs` | CLI |
| `tests/client.rs` | 위의 모든 흐름을, 실제 socket 위 backend 둘을 상대로 |
| `tests/live.rs` | 같은 client를 진짜 model을 상대로. `#[ignore]` |

직접 client를 쓸 생각이라면 읽을 것은 `src/fake.rs`입니다. 실제 producer가 하는 짓의
목록입니다. 항목마다 그것이 못 박는 동작의 이름을 딴 test가 붙어 있습니다.
`tool_arguments_split_mid_escape_reassemble_into_valid_json`,
`an_event_published_inside_a_call_loses_its_nesting`,
`a_truncated_stream_ends_the_run_rather_than_hanging`입니다.

```sh
cargo test -p board-watch
```

## 다음

- [session](/ag-ui-rust/ko/client/session/)과
  [update stream](/ag-ui-rust/ko/client/updates/) — 이 예제가 딛고 선 API.
- [run rendering](/ag-ui-rust/ko/client/rendering/) — 묶어 그리기의 거래를, 기록이 아니라
  reference로.
- [transport](/ag-ui-rust/ko/client/transports/) — `replay`가 하는 일.
- [task-board](/ag-ui-rust/ko/examples/task-board/) — 이 client가 겨누는 agent.
