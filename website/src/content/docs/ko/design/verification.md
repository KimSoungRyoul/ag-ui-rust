---
title: verification
description: event stream을 올바른 형태로 지키는 세 layer. typestate handle, 양쪽 끝의 runtime ordering verifier, 그리고 CI에서 도는 upstream drift check.
---

"stream이 올바른 형태다"는 서로 다른 세 개의 주장입니다. 장치도 셋이 필요합니다.

1. **code가 겹치는 block 두 개를 열 수 없습니다.** type system의 보증이고,
   `compile_fail` doctest가 증명합니다.
2. **나가는 event는 ordering 규칙을 지킵니다.** runtime state machine입니다.
   server와 client 양쪽에 있고, release build에서도 기본으로 켜져 있습니다.
3. **event 집합이 여전히 protocol과 맞습니다.** 저장소에 넣어 둔 upstream
   snapshot을 상대로 하는 offline drift check입니다. 모든 pull request에서
   돕니다.

각각은 나머지가 잡을 수 없는 것을 잡습니다. 이 페이지는 그 셋 전부입니다.

## Layer 1: borrow checker

`ag_ui::server`의 emitter는 typestate handle입니다. `ctx.assistant_message()`는
run context를 mutable로 빌리는 handle을 돌려줍니다. 그래서 겹치는 두 번째 block은
compile되지 않습니다.

```rust,compile_fail,E0499
use ag_ui::server::RunContext;

fn interleave(ctx: &mut RunContext<()>) {
    let mut first = ctx.assistant_message().unwrap();
    // error[E0499]: cannot borrow `*ctx` as mutable more than once at a time
    let mut second = ctx.assistant_message().unwrap();
    first.delta("a").unwrap();
    second.delta("b").unwrap();
}
```

저 block은 `compile_fail`입니다. 언젠가 compile되기 시작하면 이 페이지가
빨개집니다. 같은 예제가 `crates/ag-ui/src/server/emit/mod.rs`에도 있습니다.
[설계 원칙](/ag-ui-rust/ko/design/commitments/)이 대표 기능으로 내세우는 보증에
대한, 유일한 실행 가능한 증명이 그것입니다. emitter API를 느슨하게 만들면 그
doctest는 초록이 됩니다. 그것이 곧 실패입니다.

handle은 `Drop`에서 종료 event도 emit합니다. 그래서 보증의 나머지 반쪽도
유지됩니다. 연 것은 닫힙니다. message 한복판에서 `?`가 풀려 나가도 그렇습니다.

### stable rustdoc으로는 부족한 이유

저 block의 annotation은 기대하는 error code를 적습니다. `compile_fail,E0499`입니다.
**stable rustdoc은 그 code를 parse한 다음 무시합니다.** 예제는 compile에 실패하기만
하면 됩니다. 이유는 무엇이든 상관없습니다. 오타 하나로도 충분합니다. 그러면 test는
계속 통과하고, 보증만 조용히 검사되지 않습니다.

추측이 아닙니다. `emit/mod.rs`의 doctest에 `E0308`을 붙여 확인했습니다. type
불일치이고, 그것이 붙은 borrow check 예제와는 아무 상관이 없습니다. stable은
통과했습니다. nightly는 "Some expected error codes were not found"로 실패했습니다.

그래서 CI는 doctest를 nightly에서도 돌립니다. 그 job은 오로지 이 이유로
존재합니다. build 전체에서 nightly를 쓰는 유일한 곳입니다.

## Layer 2: runtime ordering verifier

protocol이 금지하는 것이 모두 borrow로 표현되지는 않습니다. `ctx.emit`은 chunk
event와 뒤섞인 병렬 tool call을 위해 문서화된 escape hatch입니다. 이것을 쓰는
agent는 아무도 열지 않은 message에 `TEXT_MESSAGE_CONTENT`를 emit할 수 있습니다.
그것은 bug입니다. 그러지 않으면 원인에서 network를 세 번 건너간 곳에서, 혼란에
빠진 frontend로 드러납니다.

### server에서

`ag_ui::server`는 나가는 모든 event에 ordering state machine을 돌립니다. transport가
그것을 보기 전에 돕니다. 규칙을 어기는 emit은 `Err`를 돌려줍니다. 그래서 agent의
다음 `?`가 run을 풀어냅니다. 실패는 규칙 이름을 담은 `RUN_ERROR`로 보고됩니다.

```rust
use ag_ui::{Event, EventType, RunAgentInput};
use ag_ui::server::{Error, Rule, RunContext};

fn main() {
    let (mut ctx, _events) =
        RunContext::<()>::new(RunAgentInput::new("thread-1", "run-1")).unwrap();

    // 한 번도 열린 적 없는 message에 대한 content입니다.
    let error = ctx
        .emit(Event::text_message_content("msg-1", "Hello"))
        .expect_err("the verifier should reject this");

    let Error::Verification(violation) = error else {
        panic!("expected a verification error");
    };
    assert_eq!(violation.event, EventType::TextMessageContent);
    assert_eq!(violation.rule, Rule::NotOpen);
}
```

[`Rule`](/ag-ui-rust/api/ag_ui/server/error/enum.Rule.html)은 이 machine이 검사하는
것의 닫힌 목록입니다.

| Rule | 거부되는 것 |
| --- | --- |
| `RunEnded` | `RUN_FINISHED` / `RUN_ERROR` 뒤에 오는 모든 것 |
| `DuplicateRunStarted` | 두 번째 `RUN_STARTED` |
| `DuplicateStart` | 이미 열려 있는 id로 message, reasoning block, tool call, step을 여는 것 |
| `NotOpen` | 열린 적 없는 것에 대한 content나 종료 event |
| `UnknownId` | 소개된 적 없는 call id에 대한 `TOOL_CALL_RESULT` |
| `OutOfOrder` | 그 call의 `TOOL_CALL_END`보다 앞선 `TOOL_CALL_RESULT` |
| `OpenAtFinish` | message, reasoning block, tool call, step이 열린 채로 온 `RUN_FINISHED` |

`RUN_ERROR`는 `OpenAtFinish`에서 면제됩니다. message 도중에 터진 run이 그것을
닫았을 리 없습니다.

거부는 하나하나가
[`VerificationError`](/ag-ui-rust/api/ag_ui/server/error/struct.VerificationError.html)입니다.
문제가 된 event type과 규칙, 그리고 detail 문자열을 담습니다. 실제로 열린 message가
`msg-1`인데 `msg-2`에 content를 emit하면, `Display`는 이렇게 읽힙니다.

```text
TEXT_MESSAGE_CONTENT breaks rule `not-open` (content and terminators require a
matching start): message MessageId("msg-2") is not open [open: messages=["msg-1"]]
```

아직 열린 것 전부를 대괄호에 쏟아 내는 부분은 **debug build 전용**입니다. 비싼
쪽이 그것입니다. 그리고 대개 그것만으로 빠진 종료 event를 찾아냅니다.

이 machine이 일부러 통과시키는 것도 거부하는 것만큼 설계의 일부입니다. `*_CHUNK`
event는 그 자체로 완결됩니다. 그래서 새 id를 실은 chunk는 start가 없다고 거부되지
않고, 그 id를 등록합니다. deprecated된 `THINKING_*` family는 아예 추적하지
않습니다. state, activity, raw, custom event는 ordering이 없습니다. *서로 다른* 두
id는 얼마든지 겹칠 수 있습니다. 어떤 call을 서술하는 message 안에서 그 tool call이
열리는 것은, 병렬 call을 하는 provider가 실제로 보내는 모양입니다. 규칙은 하나의
id가 자기 자신과 겹칠 수 없다는 것입니다.

#### 비용과 끄는 법

`HashSet` 몇 개, event당 조회 한 번입니다. release build에서도 기본으로 켜져
있습니다. 사용자에게 닿는 protocol bug 옆에 두면 그 값은 고민할 거리가 아닙니다.

측정해 보고 그 조회를 되찾고 싶다면 `verify` feature를 끄십시오. state machine
전체가 크기 0인 type으로 바뀝니다. 그 type의 `observe`는 inline된 `Ok(())`입니다.
`verify`가 `server`에 함의되지 않고 crate의 default 집합에 있는 이유가 이것입니다.
다른 feature가 끌어온 집합에서 feature 하나를 빼낼 수는 없습니다.

```toml
[dependencies]
ag-ui = { version = "0.2", default-features = false, features = ["server", "sse"] }
```

그 스위치를 넘어 살아남는 것이 하나 있습니다. 종료 event가 이미 나갔는지는
verifier뿐 아니라 event sink에서도 추적합니다. 그래서 verification을 compile에서
빼도 run driver가 `RUN_FINISHED`를 두 번 emit하게 만들 수는 없습니다.

### client에서

`ag_ui::client`도 검증합니다. 이유는 다릅니다. event가 남의 process에서
도착합니다. 규칙을 어기는 stream은 혼란스러운 UI가 아니라 분명한 error 하나를 내야
합니다. TypeScript SDK가 verifier를 두는 자리가 여기입니다. consumer 입장에서는
옳은 직관입니다.

규칙의 모양은 같습니다. 받는 쪽에서만 말이 되는 세 가지가 더해집니다.

- `RUN_STARTED`가 stream을 열고, 정확히 한 번만 그렇게 합니다. 그 앞에 올 수 있는
  것은 `RAW`와 `CUSTOM`뿐입니다. 둘은 정의상 protocol의 어휘 바깥이고, 그래서
  ordering 바깥이기도 합니다.
- `interrupt` 결과는 interrupt를 적어도 하나 실어야 합니다. type system으로
  표현할 수 없는 유일한 규칙입니다.
- stream은 종료 event에 *도달해야* 합니다. 그러지 않으면 일찍 끊긴 transport가
  짧은 답변과 똑같아 보입니다.

마지막 것이 `Verifier::finish`가 있는 이유입니다. `verify_all`은 기록된 stream을
다 돌린 다음 그것을 호출해 주는 편의 함수입니다.

```rust
use ag_ui::client::verify_all;
use ag_ui::{Event, TextMessageRole};

fn main() {
    // transport가 중간에 끊은 응답입니다. RUN_FINISHED도 RUN_ERROR도 없습니다.
    let truncated = [
        Event::run_started("thread-1", "run-1"),
        Event::text_message_start("msg-1", TextMessageRole::Assistant),
        Event::text_message_content("msg-1", "Hel"),
    ];

    let error = verify_all(&truncated).expect_err("the stream was truncated");
    assert_eq!(
        error.to_string(),
        "protocol violation: the stream ended before RUN_FINISHED or RUN_ERROR",
    );
}
```

`Session`은 기본적으로 streaming 형태를 돌립니다. 여기에는 cargo feature가
없습니다. runtime 스위치인 `SessionBuilder::verify(false)`가 있을 뿐입니다. 버릇을
알고 감수하기로 한 producer를 위한 것입니다. 끄면 잃는 것은 진단이지 대화가
아닙니다. applier는 어느 쪽이든 너그럽습니다.

### 왜 양쪽 끝인가

둘은 다른 질문에 답합니다. server의 verifier는 *당신이* 잘못 emit했다고 말합니다.
잘못한 그 순간에, agent의 stack이 아직 손안에 있을 때 말합니다. client의 verifier는
*다른 누군가가* 이것을 보냈다고 말합니다. 반쯤 적용된 stream이 UI bug로 자라게 두는
대신, 도착한 것의 이름을 댑니다. 어느 쪽도 군더더기가 아닙니다. run의 두 끝은 대개
같은 프로그램이 아니고, 꽤 자주 같은 SDK도 아닙니다.

## Layer 3: upstream 대비 drift

Rust event type은 upstream Zod schema를 손으로 옮긴 것입니다. compiler에는 둘을
잇는 것이 없습니다. 그래서 upstream이 event를 추가해도 이 SDK는 계속 build되고,
계속 test를 통과하고, 조용히 protocol을 더는 말하지 못하게 됩니다. 앞선 어느
커뮤니티 SDK가 그렇게 되었습니다. 당시 32개였던 spec을 상대로 event variant를
24개만 선언했습니다. 오늘 spec은 33개입니다. 어디에도 그 질문을 강제하는 장치가
없었습니다.

`xtask drift-check`가 그 연결입니다.

```sh
cargo run -p xtask -- drift-check
```

```text
drift-check
  baseline  xtask/baseline/events.json  (ag-ui-protocol/ag-ui@bc8477bfd6, captured 2026-09-02)
  upstream  36 event types
  rust      crates/ag-ui/src/event  (10 files, 36 event types, tagged enum `Event`)

OK  36 event types match the baseline.
```

이 검사는 `xtask/baseline/events.json`을 `crates/ag-ui/src/event/`와
비교합니다. baseline은 upstream `sdks/typescript/packages/core/src/events.ts`를
저장소에 넣어 둔 snapshot입니다. 어느 commit에서 왔는지, upstream 순서 그대로의
`EventType` 값, 그리고 각 event의 payload field를 optional/required 표시와 함께
기록합니다. Rust 쪽은 **텍스트로 읽습니다**. 그 module이 compile되지 않는 동안에도
검사가 돌아야 하기 때문입니다.

offline이고 결정적입니다. 그래서 필수 검사가 될 자격이 있습니다. network 장애가
이것을 빨갛게 만들 수 없습니다. exit code 0은 깨끗함, 1은 drift입니다. 2는
baseline이 없거나 event module이 옮겨졌다는 뜻입니다. 진짜 저장소 결함이므로 이
역시 실패해야 합니다.

추출기가 Zod schema를 확신 있게 읽지 못한 event는 `unparsed`로 기록됩니다. type은
그대로 비교하고 field는 비교하지 않습니다. 실패가 아니라 경고를 냅니다. 늑대가
나타났다고 외치기만 하는 검사는 결국 꺼집니다. 그래서 읽을 수 없는 schema는 하드
실패가 아닙니다. 그 목록이 늘어나면 검사 기준을 낮출 것이 아니라, 추출기에 그
모양을 가르쳐야 합니다.

### baseline 자체는 최신인가?

offline 검사는 Rust type이 snapshot과 맞는다는 것까지만 말합니다. *snapshot*이
여전히 upstream과 맞는지는 다른 질문입니다. 답하려면 network가 필요합니다.

```sh
cargo run -p xtask -- drift-check --upstream
```

이것은 필수 검사가 아니라 예약 job으로 돕니다. offline 판정은 그대로 두고, fetch가
실패하면 보고만 합니다. 그래서 rate limit이나 GitHub 장애는 실행을 실패시킬 수
없습니다. 진짜 upstream 변화만 실패시킵니다.

변화가 보고되면 사람이 그것을 받아들입니다.

```sh
cargo run -p xtask -- drift-check --refresh
```

이 명령은 baseline을 다시 잡고, upstream commit과 fetch 날짜를 기록합니다.
`events.json`의 diff가 **곧** protocol 변경입니다. 그 pull request에서 가장 꼼꼼히
볼 부분이 그것입니다. 그다음 `crates/ag-ui/src/event/`를 같은 pull request
안에서 맞춥니다. `drift-check`가 다시 깨끗해질 때까지 합니다.

`events.json`은 생성되는 파일입니다. 손으로 고치지 않습니다. 손으로 고치는 것은
code에 맞춰 protocol을 고치는 일입니다. 이 검사가 잡으려고 존재하는 실패가 바로
그것입니다.

## 각 layer가 못 하는 일

- borrow checker는 `ctx.emit`으로 나간 event를 보지 못합니다. 그래서 layer 2가
  있습니다.
- runtime verifier는 protocol에 34번째 event가 생겼다는 것을 알지 못합니다.
  그래서 layer 3이 있습니다.
- drift check는 이름과 field가 그대로인 채 event의 *의미*만 바뀐 것을 말해 주지
  못합니다. 여기 있는 어떤 것도 못 합니다. `--refresh`의 diff를 읽는 일이 그래서
  있습니다.

위의 모든 것은 upstream 최신성 job을 빼고 모든 pull request에서 CI로 돕니다.
[테스트](/ag-ui-rust/ko/design/testing/)에 전체 목록과 로컬에서 돌리는 법이
있습니다.
