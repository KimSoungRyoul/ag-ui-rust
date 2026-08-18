---
title: 테스트
description: 이 workspace를 test하는 두 개의 명령, 두 번째가 선택이 아닌 이유, 두 개의 QA tier, 그리고 CI job 전체 목록.
---

## 두 개의 명령, 두 번째는 선택이 아닙니다

```sh
cargo nextest run --workspace --all-features
cargo test --doc --workspace --all-features
```

**`cargo nextest`는 doctest를 돌리지 않습니다.** "건너뛴다"도 아니고 "무시됨으로
보고한다"도 아닙니다. 아예 보지 못합니다. 초록으로 끝난 nextest 실행은 반쪽짜리
결과입니다. 그리고 자기가 반쪽이라고 알려 주지 않습니다.

이 workspace에서는 그것이 특히 중요합니다. 여기서 증명되는 것의 상당 부분이
doctest에 있기 때문입니다. crate별 README, workspace quickstart, 이 문서 사이트의
모든 Rust 스니펫이 그렇습니다. `crates/ag-ui-server/src/emit/mod.rs`의
`compile_fail` 예제도 그렇습니다. 그것은
[설계 원칙](/ag-ui-rust/ko/design/commitments/)이 대표 기능으로 내세우는 typestate
보증의, 유일한 실행 가능한 증명입니다. emitter API를 느슨하게 만들어도 nextest는
초록으로 남습니다.

그 틈은 보기 쉽고, coverage로 착각하기도 쉽습니다. 이 페이지를 쓸 당시
`cargo nextest list --workspace --all-features`는 50개 binary에 걸친 672개 test
case를 보고했습니다. 그중 doctest는 하나도 없었습니다. 두 번째 명령이 보고하는
모든 것에 대해 첫 번째 명령은 할 말이 없습니다.

명령 하나로 끝내고 싶고 nextest의 출력 없이 지낼 수 있다면
`cargo test --workspace --all-features`가 두 종류를 다 돌립니다. CI는 두 형태를 다
돌립니다. doctest 명령은 일부러 한 번 더 돌립니다. 언젠가 누군가 속도를 사려고
`cargo test`를 `cargo nextest run`으로 바꿔치기해도, doctest가 계속 돌게 하려는
것입니다. 그러지 않으면 doctest는 build를 실패시키지도 못한 채 사라집니다.

### 이 사이트도 그 일부입니다

이 페이지들의 모든 Rust block은 `cargo test --doc -p ag-ui-e2e --all-features`로
compile됩니다. 페이지는 `e2e/src/website.rs`에 module 문서로 포함됩니다. 그러면
rustdoc이 fenced Rust block을 뽑아, `lib.rs`에 있는 것과 똑같이 compile합니다.
frontmatter와 산문과 `:::note` directive는 손대지 않은 채 통과합니다.

그래서 낡은 스니펫은 빨간 build가 됩니다. 그것을 망가뜨린 사람의 기계에서 납니다.
초심자가 붙여넣기를 해 보고 발견하는 것이 아닙니다. 실제로 돌면 안 되는 block은
`no_run`으로 표시합니다. port를 잡거나 network에 닿는 것들입니다. 그래도 타입
검사는 받습니다. 페이지 목록은 glob이 아니라 손으로 적습니다. 독자가 신뢰할 수
있는 것이 그 목록이기 때문입니다. 스니펫이 compile되지 않는 페이지는 일부러
목록에서 빼야 합니다.

## 직접 쓴 agent를 test하기

emit 경로가 동기입니다. 그래서 agent는 runtime 없이, port 없이, client 없이
test할 수 있습니다. `RunContext::new`는 context와 그 event stream의 받는 쪽 끝을
함께 줍니다. agent 코드를 호출하고 나면 emit된 것이 이미 queue에 쌓여 있습니다.
`drain`이 그것을 가져옵니다.

```rust
use ag_ui_core::{Event, RunAgentInput, TextMessageRole};
use ag_ui_server::{Result, RunContext};

fn greet(ctx: &mut RunContext<()>) -> Result<()> {
    let mut message = ctx.assistant_message()?;
    message.delta("Hello")?;
    message.end()
}

fn main() {
    let (mut ctx, mut events) =
        RunContext::<()>::new(RunAgentInput::new("thread-1", "run-1")).unwrap();

    greet(&mut ctx).unwrap();

    assert_eq!(
        events.drain(),
        vec![
            Event::text_message_start("run-1-msg-1", TextMessageRole::Assistant),
            Event::text_message_content("run-1-msg-1", "Hello"),
            Event::text_message_end("run-1-msg-1"),
        ],
    );
}
```

여기서는 아무것도 `RUN_STARTED`를 emit하지 않습니다. 그것은 run driver의 일입니다.
그것을 건너뛰기에 test가 method 하나만 따로 다룰 수 있습니다. 메시지 id는 UUID가
아니라 run id와 counter에서 나왔습니다. 그래서 위와 같은 기대 event 목록을 애초에
쓸 수 있습니다.

## 두 개의 QA tier

`docs/QA.md`는 suite를 둘로 나눕니다. 두 쪽이 다른 질문에 답하기 때문입니다.

| tier | 무엇을 증명하나 | 언제 도나 |
| --- | --- | --- |
| **Deterministic E2E** | 프로토콜 배관이 올바르다는 것. event ordering 전체, state delta, human in the loop 왕복을 `ag-ui-client`가 실제 axum server를 상대로 진짜 HTTP 위에서 돌립니다. 기록된 SSE frame으로 돌리는 LLM 매핑도 여기 있습니다. | 항상. CI gate입니다. |
| **Live smoke** | 실제 streaming model에 닿는다는 것, 그것이 AG-UI event로 올바르게 매핑된다는 것, 그리고 이 SDK가 정말 어떤 LLM crate에도 의존하지 않는다는 것. | `#[ignore]`입니다. key나 로컬 endpoint가 설정된 경우에만 돕니다. 결코 CI gate가 아닙니다. |

deterministic tier는 각본대로 움직이는 mock agent와 기록된 model frame을 씁니다.
그래서 빠르고 흔들리지 않습니다. **매핑을 지키는 것도 이 tier입니다.** 파싱과 누적
규칙 하나하나가 `e2e/src/llm.rs`의 unit test로 덮여 있습니다. 캡처했거나 합성한
frame으로 돌립니다. live test가 증명하는 것은 선이 닿아 있다는 것뿐입니다.

### live tier가 gate에서 빠진 이유

남의 서비스와 이야기하고, 그 서비스는 용량이 빠듯합니다. 그것이 동작하지 않는
방식 대부분은 이 SDK에 대해 아무것도 말해 주지 않습니다. `503 high demand`가 test
실패로 보고되면, 누군가는 있지도 않은 bug를 찾느라 한 시간을 씁니다.

그래서 harness는 결과를 똑같이 다루지 않고 분류합니다. stream은 크게 소리 내어
단언합니다. *분당* 할당량을 말하는 `429`는 기다렸다 다시 묻습니다. *일일* 할당량을
말하는 `429`는 기다려서 넘길 수 없으므로 다음 model로 갑니다. `500`, `502`,
`503`, `504`는 정의상 일시적입니다. 물러섰다 재시도하고, 그다음 다른 model을
시도합니다. `404`는 그 model이 여기 없다는 뜻입니다. socket에서 아무도 응답하지
않으면 **skip**합니다. endpoint가 떠 있지 않은 것입니다. 시도할 model이 떨어져도
**skip**하되, 어느 model이 어떻게 실패했는지 이름을 댑니다. 그 밖은 실패입니다.
`400`이나 agent 오류는 우리 것이기 때문입니다.

**실패는 응답을 했는데 잘못 매핑된 model에게만 예약되어 있습니다.**

그 방침 뒤의 숫자는 읽은 것이 아니라 재 본 것입니다. Gemini 무료 등급은 project별,
model별로 분당 약 10회를 허용합니다. 하루에는 약 **20회**뿐입니다. 일일 할당량도
약 1분의 `RetryInfo.retryDelay`를 보고하는데, 그 할당량에 대해서는 거짓말입니다.
실행은 직렬화합니다. `--test-threads=1`을 쓰고 프로세스 안에서 mutex도 씁니다.
병렬 test가 분당 제한을 곧바로 건드리기 때문입니다.

### live tier 돌리기

```sh
cargo test -p ag-ui-e2e --test live_llm -- --ignored --test-threads=1 --nocapture
```

`--nocapture`는 타이핑할 값어치가 있습니다. skip된 실행과 model의 실제 답변은
단언되지 않고 출력됩니다. 그리고 harness는 통과한 test의 출력을 삼킵니다.

환경 변수 세 개가 이것을 설정합니다.

| 변수 | 기본값 | 의미 |
| --- | --- | --- |
| `AG_UI_LLM_BASE_URL` | Gemini의 OpenAI 호환 endpoint | 여기에 `/chat/completions`가 덧붙습니다 |
| `AG_UI_LLM_MODEL` | `gemini-2.5-flash-lite` | model id. 고정하며, `*-latest` 별칭은 쓰지 않습니다 |
| `AG_UI_LLM_API_KEY` | `GEMINI_API_KEY`로 폴백 | Bearer token |

harness는 OpenAI 호환 `POST {base}/chat/completions` 모양으로 말합니다. 어느 벤더의
고유 방언도 쓰지 않습니다. 거의 모든 곳이 제공하는 모양이 그것 하나이기
때문입니다. 같은 세 변수로 Ollama, LM Studio, llama.cpp, vLLM, Groq, Together,
OpenRouter, OpenAI 자체를 가리킬 수 있습니다.

:::caution[key 규칙 세 가지, 모두 하중을 집니다]
- key는 **`Authorization: Bearer` header**에 넣습니다. query parameter에는 절대
  넣지 않습니다. query 문자열은 log에 남습니다. key는 출력되지 않습니다. 일부도,
  오류 안에도, `Debug` 덤프에도 나오지 않습니다. `LlmAgent`의 `Debug` 구현은
  그것을 가리도록 손으로 썼고, 그에 대한 test가 있습니다.
- key가 아예 없으면 live test는 **skip**합니다. 찾던 변수의 이름을 대며
  skip합니다. 그것 때문에 실패하지는 않습니다. 그래서 key가 없는 기여자도 초록
  실행을 봅니다.
- **없는 것은 없는 채로 두어야 합니다.** 빈 `Authorization: Bearer`는 익명 요청이
  아니라 *거부되는* 요청입니다. 그래서 key가 없다는 것은 endpoint가 기본값일 때만
  오류입니다. "꺼 두려고" 변수에 빈 문자열을 넣지 마십시오. 설정하지 않은 채로
  두십시오.
:::

### 로컬 model을 가리키게 하기

요청 제한이 없고, key도 필요 없고, 실행에 돈도 들지 않습니다. 그래서 매핑을
손보기에는 이쪽이 낫습니다.

```sh
ollama serve && ollama pull qwen3:4b
export AG_UI_LLM_BASE_URL=http://localhost:11434/v1
export AG_UI_LLM_MODEL=qwen3:4b
cargo test -p ag-ui-e2e --test live_llm -- --ignored --test-threads=1 --nocapture
```

key는 설정하지 마십시오. base URL이 기본값이 아니면 model 폴백은 꺼집니다. 로컬
server에는 올린 model 하나뿐이고, 우회할 model별 할당량도 없습니다. tool call을
실제로 지원하는 model을 고르십시오. 그러지 않으면 tool test가 실패합니다. 진짜
이유로 실패하지만, 그 이유는 SDK의 것이 아닙니다. 작은 instruct model은 tool call을
산문으로 뱉는 일이 잦습니다.

### architecture test도 겸합니다

`LlmAgent`는 평범한 `reqwest`로 model에 닿고, `Agent` 말고는 아무것도 구현하지
않습니다. 어떤 `ag-ui-*` crate도 LLM 라이브러리에 의존하지 않습니다. 그 agent가
compile되고 stream을 흘리면,
[`Agent` trait이 LLM 경계라는](/ag-ui-rust/ko/design/commitments/) 주장은 주장이
아니라 실증이 됩니다. `rig`와 `async-openai` 같은 것들이 `e2e/Cargo.toml` 바깥에
남는 이유가 그것입니다. 없다는 사실이 곧 증거입니다.

## commit하기 전에

위생 검사는 [prek](https://github.com/j178/prek)이 지킵니다. pre-commit의 hook
runner를 Rust binary 하나로 다시 만든 것입니다. 설치할 Python이 없습니다. 명령
두 개, 한 번이면 됩니다.

```sh
brew install prek   # or: cargo install --locked prek
prek install
```

`prek install`은 shim 두 개를 겁니다. 두 번째가 중요합니다. 공백, 파일 문법(YAML,
TOML, JSON), 줄바꿈, 병합 충돌 표시, 지나치게 큰 파일, 맞춤법, 그리고
`cargo fmt --all -- --check`가 **commit**마다 돕니다.
`cargo clippy --workspace --all-targets --all-features -- -D warnings`는
**push**마다 돕니다. 기다림이 값어치를 하는 자리이기 때문입니다. pre-push shim이
없으면 clippy hook은 로컬에서 아예 발동하지 않습니다.

`prek run --all-files`는 전부를 손으로 돌립니다. CI의 `hygiene` job은 똑같은
`.pre-commit-config.yaml`을 돌립니다. 그래서 `prek install`을 해 둔 사람은 그 job이
거절할 commit을 만들 수 없습니다.

이 설정은 일부러 작습니다. CI가 이미 fmt, clippy, test, doctest, feature matrix,
MSRV, 문서, wasm과 의존성 graph 검사, drift check를 맡고 있습니다. 그중 무엇이든
매 commit마다 되풀이하면, 새로 잡히는 것 없이 모든 commit이 느려집니다.

제외 목록도 있고, 그 목록은 우연이 아닙니다. A2UI 적합성 suite, spec fixture,
insta snapshot, drift baseline은 모두 다른 프로젝트에서 복사했거나 도구가 쓴
것입니다. 그 가치는 원본과 byte까지 같은 데 달려 있습니다. 이 설정의 첫 실행이
vendoring된 fixture 열일곱 개에 조용히 후행 개행을 더했습니다. 그렇게 해서 이
목록이 생겼습니다.

## CI가 돌리는 것

job은 열 개입니다. 아홉 개는 모든 push와 pull request에서 돕니다. 열 번째는 주간
timer로 돕니다. 어느 것이든 손으로 발동시킬 수 있습니다.

| job | 하는 일 |
| --- | --- |
| `hygiene (prek)` | 위의 `.pre-commit-config.yaml`을 `--all-files`로. `cargo fmt --all -- --check`가 사는 곳입니다. 포매팅 gate는 하나이고, 기여자가 직접 돌리는 그 자리에 있습니다. |
| `test` | `cargo clippy --workspace --all-targets --all-features -- -D warnings`, 그다음 `cargo test --workspace --all-features`, 그다음 `cargo test --doc --workspace --all-features`를 일부러 한 번 더. |
| `doctest error codes (nightly)` | doctest를 nightly에서 한 번 더. `compile_fail,E0499` annotation의 오류 코드를 강제하는 유일한 장치입니다. build에서 nightly를 쓰는 유일한 곳입니다. |
| `executor-agnostic` | core, server, client, a2ui를 `wasm32-unknown-unknown`으로 build하고(`cargo check` 다섯 번), 의존성 graph 네 개에 tokio가 없음을 단언합니다. |
| `feature matrix` | `cargo check --all-targets` 열다섯 번. feature를 하나씩 단독으로, 그리고 crate마다 기본 feature를 끈 채로. |
| `MSRV 1.85` | 1.85에서 `cargo check --workspace --all-features --all-targets`. edition 2024를 이해하는 첫 compiler라, 그 약속에는 여유분이 없습니다. |
| `docs` | `RUSTDOCFLAGS: -D warnings`로 `cargo doc --workspace --all-features --no-deps`. 공개 API가 곧 제품이라, 깨진 intra-doc link는 산출물의 결함입니다. |
| `package manifest` | `publish = false`가 없는 crate 다섯 개에 `cargo package --list`를 돌립니다. `xtask`, e2e suite, 예제와 대비되는 SDK crate들입니다. 각각이 자기 `README.md`와 `LICENSE`를 packaging하는지 단언합니다. offline입니다. archive를 만들지도, 무엇을 올리지도 않습니다. |
| `protocol drift vs upstream` | `cargo run -p xtask -- drift-check`. offline이고 결정적이라, 필수 검사가 될 자격이 있습니다. |
| `upstream freshness (scheduled)` | 주간으로 도는 `drift-check --upstream`. network가 필요하므로 gate가 아니라 timer입니다. rate limit은 이것을 실패시킬 수 없고, 진짜 upstream 변화만 실패시킵니다. |

마지막 두 개는 [검증 체계](/ag-ui-rust/ko/design/verification/)입니다.

이 가운데 두 job은 손대기 전에 근거를 알아 둘 값어치가 있습니다. `hygiene` job의
Rust toolchain은 하중을 집니다. `cargo-fmt` hook이 `cargo fmt`를 셸로 불러내는데,
rustfmt가 없으면 건너뛰지 않고 실패합니다. 그리고 이 job을 지우면 CI에서 포매팅이
통째로 사라집니다. clippy는 일부러 `hygiene`에 넣지 *않았습니다*. 설정이 그것을
pre-push 단계에 두는데, `prek run`은 기본적으로 거기에 닿지 않습니다. 그래서
clippy는 `test` job에 남습니다. 자기 몫의 compile 비용을 따로 치르는 대신 그 job의
cache를 함께 씁니다.
