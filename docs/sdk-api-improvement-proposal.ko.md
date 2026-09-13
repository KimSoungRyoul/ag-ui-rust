# AG-UI / A2UI SDK 개선안: As-is → To-be

작성일: 2026-09-13. 분석 대상: `ag-ui-rust`의 `37ce5e0`.

상태: 0.4.1 명칭 정리 반영. 독립 소비자 앱, 공식 renderer core와의 상호운용, Rust 1.85·wasm 검증까지 수행했다. 실제 사용법은 README와 migration-0.4.md를 기준으로 하며 아래 As-is는 0.3 동작을 기록한다.

이 문서는 스펙 분석, SDK 사용성 분석, `HttpAgent`·`Thread` 명명 논의와 self-review를 합친 구현 설계 기록이다. To-be의 핵심 API는 0.4에 구현했으며 코드 조각의 `model`, `storage`, `search`는 애플리케이션이 제공하는 객체·함수다. 독립 실행 가능한 예제는 examples/와 컴파일되는 문서 예제에 있다.

## 1. 공개 API의 기본 관계

```text
HttpAgent                       접속할 원격 에이전트: URL, 인증, HTTP 설정
  ├─ Thread("conversation-1")    대화 ID, 메시지, 상태, 승인 대기
  │    ├─ RunStream             한 번의 요청과 응답 스트림
  │    └─ RunStream             다음 턴 또는 승인 후 재개
  └─ Thread("conversation-2")    독립적인 다른 대화

Transport                       HTTP·테스트 재생 등 통신 구현
```

- 대화 객체의 공개 타입은 `Thread`로 통일하고 옛 명칭의 별칭은 제거한다. 프로토콜의 `threadId`에 대화 상태를 묶는 SDK 객체라는 의미다.
- `HttpAgent`를 기본 진입점으로 노출하고, URL만으로 사용할 수 있게 한다.
- 한 agent에서 여러 thread를 만들 수 있다. thread 사이에 메시지·상태·승인 대기를 공유하지 않는다.
- `agent.thread(id)`는 로컬 대화 객체를 만든다. 서버에 저장된 대화를 자동으로 조회하거나 영속화한다는 뜻은 아니다.
- 기존 transport 분리는 유지한다. 사용자 정의 transport와 원본 이벤트 처리는 고급 API로 제공한다.
- 서버의 `server::Agent`는 유지한다. 사용자는 `impl Agent`로 서버를 만들고 `HttpAgent`로 서버에 접속한다.

### 타입과 소유권 계약

`HttpAgent`는 `RemoteAgent<HttpTransport>`의 별칭에서 **HTTP 전용 struct**로 변경한다. 기존 generic `RemoteAgent<T>::new(T)`와 별칭에 추가한 `HttpAgent::new(url)`는 Rust의 중복 inherent method 규칙에 걸린다. 이 충돌은 작은 컴파일 예제로 `E0592`를 확인했다.

- `RemoteAgent<T>::new(transport)`는 저수준 API로 유지하고 `HttpAgent`는 이를 내부에서 사용한다.
- `Thread<T, S = Value>`는 공유 transport 핸들과 독립적인 대화 상태를 소유한다. 생성에 사용한 agent의 borrow를 유지하지 않으므로 agent 변수가 먼저 drop되어도 thread를 사용할 수 있다.
- `agent.thread(id)`는 기본 JSON state의 thread를 만들고, `agent.thread_with_state(id, initial_state)?`는 호출자가 넘긴 타입으로 검증된 typed thread를 만든다.
- 같은 ID로 thread를 두 번 만들면 자동 동기화되는 객체가 아니라 같은 원격 대화를 가리키는 독립 로컬 복사본이다. 동시 편집과 서버 기록 동기화는 애플리케이션이 관리한다.
- `run_events()`는 정규화·상태 적용을 하지 않는다. 고수준 실행과 같은 이름으로 다른 처리 수준을 숨기지 않는다.

## 2. 사용성 변경표

| 영역 | As-is | To-be | 구분 |
|---|---|---|---|
| 기본 접속 | `HttpTransport`를 만든 뒤 기존 대화 객체 구성 | `HttpAgent::new(url)?.thread(id)` | 논의한 방향 |
| 대화 객체 이름 | 기존 대화 객체 | `Thread` | 논의한 방향 |
| 원본 이벤트 실행 | `HttpAgent::run()`이 원본 스트림 반환 | `run_events()`처럼 기능이 드러나는 이름으로 제공 | 제안 |
| 결과만 받기 | `Update`를 순회하고 종료·오류를 직접 모음 | `RunStream::collect_report()`로 종료 결과와 진단 반환 | 제안 |
| 원본 이벤트 관찰 | 기존 대화 객체에서 custom/raw/step 이벤트를 볼 수 없음 | 상태 자동 관리를 유지하면서 `on_event` 관찰 | 확인된 제약 / 제안 |
| 대화 복원 | 메시지·state를 builder에 넣지만 ID 카운터는 초기화 | snapshot 복원과 충돌 없는 ID 발급 | 확인된 결함 / 제안 |
| 상태 설정 | `set_state()`가 raw JSON만 갱신 | raw·typed 상태를 함께 검증·갱신, 실패는 `Result` | 확인된 결함 |
| 승인 재개 | 일부 응답만 보내도 나머지 대기를 제거 | 미응답·중복·잘못된 interrupt ID를 요청 전에 검사 | 확인된 결함 / 제안 |
| 승인 거절 | 기존 승인 거절 메서드 | `Thread::decline()` | 제안 |
| 실행 취소 | stream drop | drop 동작 유지 + 명시적 abort handle | 제안 |
| subagent 종료 | `?`로 나가도 Drop이 성공 종료를 보냄 | 명시적인 이벤트 종료와 미종료 검사; Drop이 성공을 추정하지 않음 | 확인된 결함 |
| A2UI 비동기 생성 | 동기 `FnMut -> Result<String>`만 수용 | Future를 받는 생성·검증·재시도 | 확인된 제약 / 제안 |
| A2UI 설정 | catalog·prompt·parser·validator를 각각 연결 | `A2uiAuthor`가 같은 버전·catalog 설정을 공유 | 제안 |
| A2UI 전송 | operation 조립·직렬화·tool 이벤트를 직접 연결 | AG-UI용 선택적 `send_a2ui()` 확장 | 제안 |

`collect_report()`는 `RunEnd::Success`만 보고 로컬 오류를 버리지 않아야 한다. 반환 보고서에 `end`, `new_messages`, `diagnostics`를 함께 보존한다. 로컬 수신 취소는 `Aborted`로 구분하며, 새로운 AG-UI wire event를 발명하지 않는다.

부분적으로 stream을 읽은 뒤 `collect_report()`를 호출해도 보고서 범위는 실행 전체다. 이미 전달한 진단을 누락하지 않는다. `new_messages`는 실행 시작 시 없던 ID의 메시지이며 기존 메시지의 수정 결과는 `thread.messages()`에서 읽는다. 원본 이벤트 전체를 보고서에 보관하지 않는다. 진단 저장은 설정 가능한 상한을 두고 생략 개수를 표시하며 종료 결과는 항상 보존한다.

## 3. A2UI 스펙 정합성 변경표

| 항목 | As-is | To-be |
|---|---|---|
| 버전 | 송신 `v0.9`; 검증기도 `v0.9`만 수용 | `v0.9`·`v0.9.1` 수용, 송신 버전 명시 가능 |
| 메시지 종류 | v0.9 표시와 v1.0 RPC 메시지가 혼재 | v0.9 계열의 네 서버 메시지와 후보 RPC 확장을 분리 |
| 데이터 삭제 | 생략과 null을 동일하게 처리 | v0.9 계열에서 생략은 삭제, 명시적 null은 저장 |
| 스트림 검증 | 모든 컴포넌트를 한 배열에 누적 | surface별 상태에 메시지를 순서대로 적용 |
| 컴포넌트 교체 | 같은 ID의 후속 갱신도 중복으로 판단 | 메시지 사이의 갱신은 교체; 다른 surface의 ID는 독립 |
| surface 수명 | 중복 create를 놓치고 삭제 반영도 불충분 | 활성 surface 중복 create 거부, delete 후 재사용 허용 |
| 구조 검증 | 일부 타입·필수 필드와 그래프 검사 | 공식 스키마 검사와 그래프·수명·바인딩 검사 결합 |
| capabilities | 내부 평면 구조를 직접 직렬화 | 전송의 버전별 namespace와 내부 모델 분리 |
| inline 함수 | 객체만 병합하여 배열 정의를 누락 | 공식 배열 형태를 처리하고 catalog 선택 정책 명시 |
| catalog ID | toolkit용 별칭 고정 | 공식 ID와 기존 호환 ID를 명시적으로 관리 |
| MIME | `application/a2ui+json` | 유지 |
| 검증 기준 | 이전 upstream conformance 중심 | 고정한 공식 스키마와 상태 전이 회귀 사례 추가 |

v0.9.1 스키마는 `v0.9`도 허용하므로 기존 저수준 message helper의 송신 기본값은 `v0.9`로 유지한다. 새 author 예제는 송신 버전을 명시한다. catalog URL의 버전 문자열을 일괄 치환하지 않는다. 공식 v0.9.1 catalog 파일 내부에도 v0.9 식별자가 남아 있으므로 실제 선언된 ID를 기준으로 한다. v1.0 RPC를 v0.9 계열로 직렬화·검증하는 경로는 거부한다. 후보 타입을 남기더라도 v1.0 전체 지원을 선언하지 않는다.

상태를 관리하는 검증기는 새로운 전체 스트림과 기존 상태에 대한 부분 갱신을 구분한다. 전체 이력이 없는 부분 갱신은 기존 surface 상태를 넘겨 검증한다. 미완성 스트림의 root·자식 대기와 최종 검증 실패도 구분해야 한다.

### 검증과 catalog 선택 계약

- envelope를 typed 메시지로 바꾸기 **전에** 공식 스키마로 검사한다. serde의 기본값·미지 필드 무시로 잘못된 입력이 정상화되는 것을 막는다.
- surface 상태는 한 대화/renderer 문맥 안에서 `surfaceId`별로 관리한다. 서로 다른 사용자의 같은 ID를 전역 맵으로 합치지 않는다.
- 각 메시지는 임시 상태에 적용하고 성공했을 때만 검증 상태를 갱신한다. 잘못된 pointer나 부분 적용 오류를 무시하지 않는다. 이미 renderer에 보낸 이전 메시지까지 되돌렸다는 의미는 아니다.
- 메시지 내부의 중복 component ID는 거부하고, 후속 메시지에서 같은 ID를 정의하면 교체한다. 전송 중의 전방 참조는 대기시키고 최종 surface 검증에서 미해결 참조를 보고한다.
- catalog는 등록된 문서와 로컬 resolver만으로 검증한다. `$ref`나 `catalogId`가 URL이라는 이유로 자동 네트워크 조회를 하지 않는다.
- 공식 catalog ID와 기존 toolkit ID의 alias는 애플리케이션이 동일한 catalog로 등록한 경우만 허용한다. 다른 catalog의 inline 정의를 기본 catalog에 합친 뒤 기존 ID로 보내지 않는다. 같은 ID에 충돌하는 정의가 있으면 선택 오류다.
- inline catalog를 선택할 때 components·함수 배열·theme·참조 리소스를 함께 유지한다. `acceptsInlineCatalogs`와 클라이언트가 지원한다고 광고한 ID를 적용하며, 매칭 실패를 서버 기본값으로 덮지 않는다.
- wire metadata는 실제 스키마에 맞춘다. v0.9.1 메시지를 써도 capabilities의 namespace는 이 버전군 스키마가 정의한 `v0.9`다.

## 4. 원격 에이전트와 대화하기

### As-is

이전에는 transport와 대화 객체를 직접 구성하고, 종료 결과를 얻기 위해 업데이트 스트림을 순회했습니다.

### To-be — 0.4

```rust
use ag_ui::client::{HttpAgent, Update};
use futures_util::StreamExt;

let agent = HttpAgent::new("http://localhost:3000/agent")?;
let mut thread = agent.thread("thread-1");

{
    // ?는 승인 대기 등 요청 전 조건을 검사한다.
    let mut run = thread.send("안녕")?;
    while let Some(update) = run.next().await {
        match update {
            Update::Message(message) => println!("{:?}", message.change),
            Update::Error(error) => eprintln!("{error}"),
            Update::Done(end) => println!("{end:?}"),
            _ => {}
        }
    }
}
println!("{} messages", thread.messages().len());

// 같은 연결 설정으로 독립적인 대화를 만든다.
let mut another = agent.thread("thread-2");
let report = another.send("주문 상태를 알려줘")?.collect_report().await;
println!("종료: {:?}, 진단: {:?}", report.end, report.diagnostics);
```

stream을 소비하는 동안 thread를 빌리는 Rust 소유권 모델은 유지한다. 블록이나 도우미로 사용을 단순화한다. `send()`의 사전 검사가 실패하면 메시지·ID·승인 대기를 변경하지 않는다. stream 생성은 요청 준비까지만 수행하고 첫 poll에서 메시지를 기록하고 transport를 호출한다. poll하기 전에 drop하거나 abort하면 요청을 보내거나 사용자 메시지를 추가하지 않는다.

## 5. 진행률 관찰과 실행 취소

### As-is

- 기존 대화 객체은 `CUSTOM`, `RAW`, step 이벤트를 사용자에게 노출하지 않는다.
- 관찰하려면 transport 래퍼를 만들거나 저수준 이벤트 API를 사용한다.
- 실행 중단은 stream을 drop한다. 기존 승인 거절 메서드은 이 기능이 아니다.

### To-be — 0.4

```rust
use ag_ui::Event;

thread.on_event(|event| {
    if let Event::Custom(progress) = event {
        println!("{}: {}", progress.name, progress.value);
    }
});

let run = thread.send("보고서를 만들어줘")?;
let abort = run.abort_handle();

// UI의 중지 버튼 처리기로 abort 핸들을 전달한다.
// 버튼을 누르면 abort.abort()를 호출한다.
let report = run.collect_report().await;
println!("{:?}", report.end);
```

관찰 콜백은 요청을 추가로 보내지 않으며, thread의 상태 갱신도 계속 실행된다. 관찰 시점은 decode된 원본 이벤트 수신 직후, 정규화·프로토콜 검증·상태 적용 이전이다. 같은 이벤트를 정규화 후 다시 알리지 않는다. decode 실패는 이벤트가 아니라 진단으로 전달한다.

첫 버전의 `on_event`는 동기·읽기 전용 콜백 하나를 설정하며 재호출은 교체다. `clear_event_observer()`로 제거한다. 콜백은 이벤트와 thread를 변경하거나 재진입하지 않는다. 저장되는 콜백은 `'static`이며 native에서는 `Send`, wasm에서는 local callback을 허용한다. 느린 I/O는 호출자의 큐로 전달한다.

### 취소와 종료 경합

- abort handle은 **해당 run만** 취소하며 이후 run을 취소하지 않는다. abort는 pending poll을 깨우고 연결 future/응답 stream을 drop한다.
- terminal 이벤트를 이미 적용했다면 나중에 온 abort는 종료 결과를 바꾸지 않는다. 아직 terminal을 적용하지 않았을 때 abort를 처리하면 로컬 `Aborted`가 된다. 종료 update는 한 번만 전달한다.
- `collect_report()`가 poll 중이면 `Aborted` 보고서를 받는다. stream 자체를 drop하면 소비자가 없으므로 `Update::Done` 전달을 약속하지 않는다. thread에는 관찰한 부분 결과와 로컬 중단 상태를 남긴다.
- 중단된 메시지를 성공 완료로 표시하지 않고, 다음 run에 이전 parser의 열린 chunk 상태를 가져가지 않는다.
- abort는 로컬 수신 취소다. 원격 작업의 중단 확인은 별도 서버 계약이 필요하며 SDK가 성공으로 추정하지 않는다.

## 6. 상태 설정과 대화 복원

### As-is

이전에는 로컬 state 설정 후 typed 값이 뒤처지고, 이력 복원 시 ID 카운터가 초기화되는 문제가 있었습니다.

### To-be — 0.4

```rust
thread.set_state(serde_json::json!({"selected": "B"}))?;
assert_eq!(thread.state()?["selected"], "B");
assert_eq!(thread.raw_state()["selected"], "B");

let saved = serde_json::to_vec(&thread.snapshot())?;
storage.save("thread-1", saved).await?;

let saved = storage.load("thread-1").await?;
let snapshot = serde_json::from_slice(&saved)?;
let mut restored = agent.restore_thread(snapshot)?;
let report = restored.send("이어서 설명해줘")?.collect_report().await;
```

snapshot에는 thread ID, 메시지, raw state, 승인 대기와 제출 시도, subagent 표시 상태 등 복원에 필요한 부가 상태와 snapshot 버전을 보존한다. 인증·transport·observer·실행 future는 저장하지 않고 연결 설정은 agent에서 공급한다. typed cache는 저장하지 않고 raw state에서 재계산한다. 버전·중복 ID·참조를 검사하고 지원하지 않는 snapshot 버전은 거부한다. 중단된 run의 로컬 snapshot으로 원격 실행 재개를 보장하지 않는다.

기본 ID 발급은 플랫폼에서 지원하는 충돌 가능성이 낮은 난수 기반 문자열이며, 복원된 메시지와 이전 로컬 run ID와의 중복을 검사한다. 실패 시 재발급하고 결정적인 테스트에는 생성기를 주입한다. ID의 공개 타입을 UUID로 제한하지 않는다. native와 wasm의 entropy 및 의존성 경로를 따로 검증한다.

### raw/typed state 계약

`state()`는 `Result<&S, StateViewError>`를 반환한다. 기본 JSON thread에서도 같은 signature이며 JSON state의 조회는 항상 성공한다. `raw_state()`는 최신 유효 JSON을 그대로 반환한다.

| 입력 | raw state | typed state / 조회 |
|---|---|---|
| 초기 값·복원 | JSON thread는 빈 객체 또는 저장값; typed thread는 지정값을 검증 | 검증 성공 후 생성. 변환 실패 시 생성/복원 오류 |
| 올바른 로컬 `set_state()` | 갱신 | 함께 갱신 |
| 타입이 틀린 로컬 `set_state()` | 이전 값 유지 | 이전 값 유지, setter가 오류 반환 |
| 유효한 원격 JSON이 `S`와 불일치 | 원격 JSON으로 갱신 | 이전 cache를 제거하고 `state()`가 오류 반환. `Update::Error`도 전달 |
| 잘못된 원격 JSON Patch | 적용 전 값 유지 | 이전 값 유지, patch 오류 전달 |
| 이후 타입에 맞는 원격 갱신 | 갱신 | 다시 조회 가능, `Update::State` 전달 |

이렇게 해야 마지막 정상 typed 값을 현재 값처럼 읽는 문제가 재발하지 않는다. 다음 요청은 원본 raw state를 사용한다. 자동 타입 수리나 임의 필드 삭제는 하지 않는다.

## 7. 승인과 재개

### As-is

이전에는 일부 승인 응답만 보내도 나머지 대기 목록이 제거되고, 승인 거절과 실행 취소의 명칭도 혼동하기 쉬웠습니다.

### To-be — 0.4

```rust
use ag_ui::client::interrupts::InterruptExt;

let pending = thread.interrupts().to_vec();

// 두 결정의 답을 모아 한 번에 재개한다.
let answers = [
    pending[0].resolve(serde_json::json!({"approved": true})),
    pending[1].cancel(),
];
let report = thread.resume_many(answers)?.collect_report().await;
```

대기 항목을 빠뜨리거나 잘못된/중복 ID를 보내면 요청 전에 오류를 반환하고 기존 대기 목록을 보존한다. `thread.resume(&interrupt, payload)`와 `thread.decline(&interrupt)`는 그 응답만으로 현재 대기가 모두 해소될 때 사용하는 편의 메서드다. ID가 같은 외부 객체를 신뢰하지 않고 현재 thread의 pending 목록에서 검증한다. 광고된 만료 조건도 확인한다. 응답 스키마는 UI가 사용할 수 있도록 보존한다. 최소 client에 임의 JSON Schema 실행기를 강제하지 않으며 응답 스키마 검증과 실제 실행 권한은 서버/애플리케이션 책임이다.

### 승인 응답 전송의 로컬 상태

| 시점 | SDK 처리 |
|---|---|
| 사전 검사 실패 / 첫 poll 전 drop | pending과 대화 기록을 그대로 보존 |
| 첫 poll에서 transport 호출 | run ID와 제출 답을 기록. pending을 완료로 삭제하지 않음 |
| 검증된 `RunFinished` 수신 | success이면 이전 대기를 해소; interrupt이면 서버가 보낸 새 목록으로 교체 |
| transport 호출 뒤 오류·abort·`RunError`·검증 불가 terminal | 답과 시도를 `Unconfirmed`로 보존. 처리 여부를 추정하지 않음 |

`RunStarted`나 HTTP 2xx만으로 승인 처리가 완료됐다고 판단하지 않는다. `Unconfirmed`가 있으면 일반 `send`/`resume`로 같은 결정을 다시 보내지 않는다. 애플리케이션이 서버의 실제 상태를 조회한 뒤 그 결과로 대화 snapshot을 재구성해야 한다. AG-UI에는 그 조회를 위한 보편적 endpoint가 없으므로 SDK가 가상 조회 API나 자동 재시도를 만들지 않는다. 이 기록은 전송 시도 관찰이며 durable 업무 실행기나 exactly-once 보장이 아니다.

## 8. 서버와 subagent

기본 서버 작성법은 유지한다. README의 첫 예제는 기존 `ctx.say()`를 사용하면 더 짧게 설명할 수 있다.

```rust
impl Agent for Greeter {
    type State = ();

    async fn run(&self, ctx: &mut RunContext<()>) -> Result<RunOutcome> {
        ctx.say("안녕하세요")?;
        Ok(RunOutcome::Success)
    }
}
```

### Subagent As-is

```rust
let mut child = ctx.subagent("researcher")?;
let result = search().await.map_err(ag_ui::server::Error::agent)?;
child.finish_with(result)?;
// search가 실패하면 ?로 빠져나가고 Drop이 성공 종료를 emit한다.
```

### Subagent To-be — 0.4

```rust
let mut events = ctx.subagent_events("researcher")?;
events.say("자료를 찾고 있습니다")?;

// 실행은 애플리케이션/프레임워크가 담당한다.
let result = search().await;
match &result {
    Ok(value) => events.finish_with(serde_json::json!(value))?,
    Err(error) => events.fail(error.to_string())?,
}
let result = result.map_err(ag_ui::server::Error::agent)?;
```

앞서 제안한 callback 기반 `with_subagent`와 병렬 child 실행기를 이번 범위에서 제외한다. `subagent_events()`는 시작·출처·종료를 기록하는 emitter이며 모델·도구·프롬프트·실행·재시도·영속화를 소유하지 않는다. 기존 `ctx.subagent()` 명칭의 이행 여부는 같은 이벤트 기능의 alias로만 다룬다.

### 이벤트 스코프의 종료 계약

- 성공은 `finish`/`finish_with`, 하위 실패는 `fail`, 승인 대기는 `suspend`로 명시한다. subagent 오류를 부모 run 오류로 올릴지는 애플리케이션이 결정한다.
- 미완료 emitter의 Drop은 부모 출처를 복원하지만 성공·실패·suspend를 추정해서 emit하지 않는다. 열린 lifecycle 기록은 driver에 남긴다.
- 부모가 `Err`로 끝나면 `RunError`로 종료할 수 있다. 이 경로에서는 하위가 열려 있어도 허위 성공을 추가하지 않는다.
- 부모가 success/interrupt `RunFinished`를 반환하려는데 열린 하위 lifecycle이 남으면 producer 오류로 종료한다. 이는 emitter의 필수 종료 검사이며 선택적 추가 verifier를 꺼도 사라지지 않는다.
- 실제 `RunError` 또는 transport 단절을 받은 클라이언트는 남은 하위 표시를 중단 상태로 정리하며 하위의 업무 실패나 성공을 발명하지 않는다.
- 출처만 있는 이벤트, 외부 프레임워크가 이미 병렬로 생성한 이벤트, suspended ID의 후속 재개는 계속 수용한다. 병렬 이벤트 지원과 병렬 작업 실행은 별개다.
- 하위의 state 이벤트도 run의 공유 state를 갱신한다. `subagentRunId`가 private state를 만든다고 해석하지 않는다.

## 9. A2UI 생성과 전송

### As-is

```rust
let catalog = Catalog::basic();
let spec = PromptSpec::new("UI를 작성해줘", "주문 요약", &catalog);
let prompt = build_subagent_prompt(&spec);

let surface = generate_with_recovery(
    &prompt,
    &catalog,
    &RecoveryOptions::default(),
    |prompt, attempt| blocking_generate(prompt, attempt),
    |_| {},
)?;

let envelope = wrap_as_operations_envelope(&surface.operations)?;
let mut call = ctx.tool_call(RENDER_A2UI_TOOL_NAME)?;
call.args_json(&serde_json::json!({"surfaceId": "order"}))?;
call.result(envelope)?;
```

이 콜백에 일반적인 async LLM 호출을 직접 넣을 수 없다. prompt·schema·validator 설정도 각각 맞춰야 한다.

### To-be — 0.4

```rust
use ag_ui_a2ui::{A2uiAuthor, A2uiVersion};
use ag_ui_a2ui::agui::A2uiRunContextExt;

let author = A2uiAuthor::basic(A2uiVersion::V0_9_1)?;

let surface = author
    .create("order", "주문 내용을 표로 보여줘")
    .generate(|prompt, _attempt| {
        let model = model.clone();
        async move {
            model.complete(&prompt).await.map_err(ag_ui_a2ui::Error::generation)
        }
    })
    .await?;

ctx.send_a2ui(&surface)?;
```

이 예제는 새 `ag-ui-server` feature를 사용한다. callback은 `F: FnMut(String, u32) -> Fut`, `Fut: Future<Output = Result<String>>`로 받는다. 소유한 prompt를 넘기고 예제의 모델 client는 clone한 핸들을 future로 이동한다. callback 자체를 mutably borrow한 채 future를 반환하는 lending closure는 첫 API의 필수 조건으로 삼지 않는다. `generate`가 future를 await한 뒤 다음 시도를 시작한다.

기본 API에는 필수 `Send`를 강제하지 않고 boxing으로 auto trait을 지우지 않는다. native에서 사용하는 callback·future가 Send이면 생성 future도 Send가 되어 `server::Agent`에서 await할 수 있어야 한다. local callback도 받아들인다. 이 generic 형태는 Arc 기반 Send future와 Rc 기반 local future를 각각 받는 작은 Rust 프로그램으로 컴파일 확인했다. 실제 schema engine을 포함한 Send future도 Review Desk의 server::Agent 구현과 native 빌드로 확인했다.

- `A2uiAuthor::basic()`는 해당 버전의 전체 schema bundle과 catalog를 같이 준비한다.
- 생성 콜백은 provider 중립인 비동기 함수다. Tokio나 특정 모델 SDK를 강제하지 않는다.
- 실패하면 동일 설정으로 검증하고, 다음 시도의 prompt에 교정 정보를 넣는다.
- 표준 생성 오류 변환을 제공해 provider 오류를 의미 없는 parser 오류로 바꾸지 않는다.
- 기존 surface 수정은 `.edit(&prior_surface, request)`처럼 대상 상태를 명시한다.
- `send_a2ui()`는 선택적 AG-UI 서버 확장이다. 이 통합이 사용하는 `a2ui_operations` envelope와 tool 이벤트 전송을 담당한다. 모든 A2UI transport의 필수 형식으로 취급하지 않는다.
- 수동 operation 작성과 저수준 parser는 계속 제공한다.

### 생성 결과의 계약

`A2uiAuthor`는 불변 schema/catalog 설정을 공유하고 요청마다 독립적인 생성 상태를 만든다. `.generate()`의 재시도 대상은 생성된 문서의 parsing·검증 실패다. provider 오류는 원인을 유지해 즉시 반환하며 네트워크 재시도 정책은 호출자가 결정한다.

생성 성공 결과는 private 필드의 `ValidatedSurface`다. `operations()`는 읽기 전용이며 역직렬화나 임의 수정으로 검증 완료 표시를 만들 수 없다. 저장한 결과는 다시 검증한다.

- `.create("order", ...)`는 지정한 버전·catalog·surface 하나에 대해서만 유효하다. 생성이 필요한 메시지 순서도 검사한다.
- `.edit(&prior, ...)`는 기존 catalog와 surface ID를 고정하고 해당 surface의 update만 허용한다. 임의 create/delete, 다른 surface 수정, catalog 교체를 거부하고 교정 대상으로 보고한다.
- 모델이 쓴 target/catalog/version을 몰래 바꾸지 않는다. 요청과 다르면 검증 오류로 재생성한다.
- prior는 관찰된 상태이며 renderer의 사용자 입력보다 오래됐을 수 있다. `.edit()`는 최종 일관성·동시 편집·충돌 없는 병합을 보장하지 않는다. 입력 폼의 수정 예제는 필요한 path만 갱신한다.
- 성공한 로컬 검증과 event sink enqueue는 renderer 수신·반영 확인이 아니다. `send_a2ui()` 오류에서 전체 batch를 자동 재전송하거나 서버의 추정 surface를 확정 상태로 저장하지 않는다.

위 예제는 전체 생성·검증 후 전송하는 경로다. 점진적 streaming과 재시도를 결합하는 API는 partial surface의 복구·삭제 정책까지 별도로 정해야 하므로 이 예제에 숨기지 않는다.

## 10. null 저장과 값 삭제

### As-is

```rust
AgentMessage::update_data_model("order", "/memo", serde_json::Value::Null);
// 현재 서버의 apply에서는 memo 키가 삭제된다.
```

### To-be — 0.4

```rust
// 기존 이름을 유지하되 v0.9 계열의 의미를 바로잡는다.
let set_null = AgentMessage::update_data_model(
    "order", "/memo", serde_json::Value::Null,
);

// 삭제를 명시적으로 표현하는 새 helper.
let remove = AgentMessage::remove_data_model_value("order", "/memo");
```

| 의도 | 직렬화되는 updateDataModel payload |
|---|---|
| null 저장 | `{"surfaceId":"order","path":"/memo","value":null}` |
| 삭제 | `{"surfaceId":"order","path":"/memo"}` |

decode→encode와 history 복원에서도 생략 여부를 보존한다. v1.0의 null 삭제 규칙을 이 프로파일에 섞지 않는다.

**배열 슬롯과 root 삭제도 이번에 정의한다.** wire update 값은 `Set(Value)`와 `Remove`를 구분하는 표현으로 보관한다. 로컬 A2UI 데이터 모델은 JSON 값에 `Undefined`를 추가한 표현을 사용한다. 객체 키 삭제는 키 제거, 배열 인덱스 삭제는 길이를 유지한 `Undefined` 슬롯, root 삭제는 `Undefined` root다. 명시적 null은 모든 위치에서 값으로 유지한다.

데이터 lookup은 missing/undefined와 명시적 null을 구분한다. 보통 JSON으로 정확히 표현할 수 없는 `Undefined`가 있으면 JSON export가 오류를 반환하며 null 치환을 기본값으로 하지 않는다. 로컬 snapshot은 버전 있는 내부 표현으로 이를 보존하고 그 표현을 A2UI wire에 보내지 않는다. 전송은 원래의 update operations로 하며, JSON 원본을 받는 기존 binding API는 유지하되 새 모델을 읽는 경로를 보완한다.

## 11. 적용 순서와 호환성

1. **동작 수정**: state 설정/조회, 복원 ID, 승인 대기와 불확정 제출 보존, subagent 허위 성공 제거, A2UI null/삭제·surface 상태 적용.
2. **공개 사용 흐름**: `Thread`, agent에서 thread 생성, 결과 보고서, 원본 이벤트 관찰, 명시적 취소.
3. **A2UI 사용 흐름**: 비동기 recovery, 공통 author 설정, AG-UI 전송 확장.
4. **스키마와 문서**: 공식 스키마 검증, catalog/capabilities 정합성, 예제·한국어 문서·skills 동시 갱신.

0.4.1에서는 대화 객체의 옛 타입·모듈 이름과 별칭을 제거한다. 코드·예제·문서는 `Thread`와
`ThreadBuilder`만 사용한다. 버전은 사용자가 지정한 대로 0.4.0에서 0.4.1로 올린다.
호출자는 현재 API로 이행해야 하며, 이행 가이드는 옛 이름의 실행 예제를 유지하지 않는다.

### 의존성과 feature 범위

| feature / 계층 | 계약 |
|---|---|
| `ag-ui` core | protocol 타입 유지 |
| `ag-ui/client` | Thread·observer·로컬 취소·보고서. 특정 executor 의존성 없음 |
| `ag-ui/http` | 기존 HTTP transport 및 HttpAgent. Thread의 상태 관리는 공유 client 코드 |
| `ag-ui-a2ui/toolkit` | 기존 저수준 조립·parsing·의미 검증 유지. native 검증을 full JSON Schema 검증이라고 부르지 않음 |
| 새 `ag-ui-a2ui/schema-validation` | 고정한 스키마의 Draft 2020-12 검증. 로컬 reference registry |
| 새 `ag-ui-a2ui/author` | toolkit + schema-validation. 고수준 생성과 ValidatedSurface. 비동기이나 executor 중립 |
| 새 `ag-ui-a2ui/ag-ui-server` | author + 기존 ag-ui 연동 + `ag-ui/server`. send_a2ui 확장. axum/Tokio를 자동으로 켜지 않음 |

기존 기본 feature에 새 schema engine과 서버 의존성을 몰래 추가하지 않는다. `A2uiAuthor`의 full-validation 보장을 feature off일 때 약한 검증으로 대체하지 않는다. engine의 Rust 1.85·wasm 지원은 dependency 선택 전 검증 gate이며 통과하지 못하면 이 feature의 구현 완료를 선언하지 않는다.

회귀 검증에는 독립 소비자에서 실제로 아래 흐름을 사용하게 한다.

- agent 하나에 독립 thread 둘 만들기, 기록 복원 후 새 메시지 보내기.
- 초기/로컬/원격 state 반영, typed state 실패 후 stale cache 제거와 정상 갱신 복구.
- custom 이벤트 관찰, 부분 소비 후 보고서, abort wakeup/종료 경합/다음 run 격리, 오류 진단 상한.
- 두 승인 중 하나만 답한 요청 거부, 첫 poll 전 drop, 제출 뒤 응답 유실·RunError·복원 시 Unconfirmed 보존.
- subagent 성공·오류·중단·suspend 구분, 성공 경로의 미종료 거부, 부모 오류 경로의 허위 하위 완료 없음.
- 비동기 생성 실패→교정→성공, A2UI 생성/수정/삭제와 데이터 null 왕복.
- 각 공식 스키마 사례와 surface별 순차 검증, 기본·선택 feature 빌드.
- 배열의 Undefined 슬롯·root 삭제·null을 구분한 encode/decode와 snapshot 복원.
- 생성 모델이 target/catalog/version을 바꾸거나 edit에서 create를 보내면 거부.
- schema engine과 비동기 callback의 native/wasm·Rust 1.85 빌드. 제안 예제를 외부 crate에서 컴파일.

## 12. Self-review 결과

| ID | 중요도 | 발견한 설계 문제 | 반영한 수정 |
|---|---|---|---|
| SR-01 | P1 | HttpAgent 별칭에 URL용 new를 추가하면 generic new와 컴파일 충돌 | 별도 struct, 공유 transport 소유권, 타입 이행 범위 명시 |
| SR-02 | P1 | 원격 typed state 불일치에서 direct getter의 약속이 성립하지 않음 | Result getter와 cache 무효화/복구 계약 |
| SR-03 | P1 | 응답 유실 뒤 pending 보존만으로는 중복 승인 재전송을 막지 못함 | 제출 이력과 Unconfirmed, 자동 재전송 금지, 서버 확인 경계 |
| SR-04 | P1 | callback subagent API가 실행 범위를 넓히고 Drop·suspend 추론 문제를 남김 | 이벤트 전용 emitter, 명시적 종료, driver 미종료 검사 |
| SR-05 | P1 | 검증 성공이 목표 surface 일치·수신 완료를 보장하는 것처럼 보임 | target/version/catalog 고정, 검증 결과 불변, enqueue와 적용 구분 |
| SR-06 | P1 | null/생략 수정만으로 배열 undefined와 root 삭제가 표현되지 않음 | 내부 Undefined 표현, 정확한 snapshot과 JSON export 오류 |
| SR-07 | P2 | abort·observer·부분 소비 보고서·feature·alias 이행의 세부 계약 누락 | 동작 순서·범위·의존성·breaking migration 명시 |

설계상 위 항목을 반영했고, 구현에서 MSRV/wasm 조합, public signature와 callback 컴파일, 공식 renderer core 상호운용 gate를 통과했다. 병렬 실행기, 자동 서버 상태 조회, exactly-once 업무 실행, 생성 중 partial surface의 자동 재전송은 이 설계의 완료 조건에 넣지 않는다.

최초 설계 리뷰에서는 공식 원문 비교, 생성자 충돌 재현, 소유권·Future callback 컴파일과 문서 교차 확인을 수행했다. 이후 실제 구현과 소비자 검증 결과는 아래 기록에 정리했다.

## 근거

- 로컬 실행 재현: `/tmp/ag-ui-sdk-ergonomics-0913/results.txt`, `/tmp/ag-ui-a2ui-audit-091/results.jsonl`.
- 이번 컴파일 재현: `/tmp/ag-ui-design-review-0913/constructor.log`, `/tmp/ag-ui-design-review-0913/api_shape.rs`.
- [공식 AG-UI HttpAgent](https://docs.ag-ui.com/sdk/js/client/http-agent).
- [공식 AG-UI subscriber](https://docs.ag-ui.com/sdk/js/client/subscriber).
- [공식 AG-UI subagent 범위와 종료 규칙](https://github.com/ag-ui-protocol/ag-ui/blob/747933694b05676203da7d5bb8d8e50432e59b75/docs/concepts/subagents.mdx).
- [비교한 공식 TypeScript client 소스](https://github.com/ag-ui-protocol/ag-ui/blob/747933694b05676203da7d5bb8d8e50432e59b75/sdks/typescript/packages/client/src/agent/agent.ts).
- [공식 A2UI DirectJsonFormat](https://github.com/a2ui-project/a2ui/blob/1c45c809b655878d06e3afc6dda22100afecc0a4/agent_sdks/python/a2ui_agent/src/a2ui/inference_formats/direct_json/format.py).
- [A2UI v0.9.1 변경 가이드](https://github.com/a2ui-project/a2ui/blob/1c45c809b655878d06e3afc6dda22100afecc0a4/specification/v0_9_1/docs/evolution_guide.md).
- [A2UI v0.9.1 서버 메시지 스키마](https://github.com/a2ui-project/a2ui/blob/1c45c809b655878d06e3afc6dda22100afecc0a4/specification/v0_9_1/json/server_to_client.json).


## 0.4 구현 검증 기록

- `HttpAgent`, `Thread`, 불변 검증 결과, 상태·승인·중단 계약을 구현했다.
- 독립 소비자는 task-board, board-watch, review-desk 세 프로젝트로 관리한다.
- 공식 `@a2ui/web_core@0.11.0`에 Rust 생성 메시지를 수정 없이 입력하여 13단계를 비교했다.
- 독립 리뷰에서 Activity 요청 필터링, 배열·null 부모의 upsert, weak prior 이력 재검증을 추가 보완했다.
- 전체 runtime 테스트 835개, stable/nightly doctest 각각 269개가 통과했다.
- Rust 1.85 workspace, wasm client/server/A2UI, executor 의존성 경계, clippy·rustdoc와 문서 사이트 링크 검사도 통과했다.
- 실제 Gemini 연결의 text·tool·subagent와 board-watch 소비자 smoke 4개도 통과했다.
- 이 기록은 full Lit renderer의 화면 배치 검증이나 업무 실행의 exactly-once 보장을 뜻하지 않는다.
