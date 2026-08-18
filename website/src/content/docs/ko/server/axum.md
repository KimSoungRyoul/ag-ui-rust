---
title: HTTP로 serving
description: axum router에 agent를 얹는 법. 그리고 그렇게 만들어진 endpoint가 주고받는 요청과 응답.
---

`ag-ui-server`는 [`Agent`](/ag-ui-rust/ko/server/agent/)를 event stream으로 바꾸고 거기서
멈춥니다. 일부러 그렇습니다. executor도 웹 프레임워크도 없어야 wasm으로 빌드됩니다.

`ag-ui-axum`이 나머지 절반입니다. POST endpoint, `text/event-stream` 본문, content
negotiation, 그리고 client가 끊었음을 agent에게 알리는 일을 맡습니다. 이 workspace에서 tokio,
axum, tower에 의존하는 유일한 crate입니다.

## agent 얹기

```rust,no_run
// src/main.rs
use ag_ui_axum::RouterExt;
use ag_ui_core::RunOutcome;
use ag_ui_server::{Agent, Result, RunContext};
use axum::Router;
use axum::routing::get;

struct Greeter;

impl Agent for Greeter {
    type State = ();

    async fn run(&self, ctx: &mut RunContext<()>) -> Result<RunOutcome> {
        ctx.say("Hello!")?;
        Ok(RunOutcome::Success)
    }
}

#[tokio::main]
async fn main() {
    let app: Router = Router::new()
        .route("/health", get(|| async { "ok" }))
        .route_agui("/agent", Greeter);

    let listener = tokio::net::TcpListener::bind("0.0.0.0:3000").await.unwrap();
    axum::serve(listener, app).await.unwrap();
}
```

`route_agui(path, agent)`는 `route(path, post(handler))`입니다. 그 이상은 아무것도 아닙니다.
이 endpoint는 router의 다른 라우트들과 그대로 조합됩니다. `nest`, `merge`, `fallback`과도,
앞뒤로 씌운 어떤 layer와도 그렇습니다. 같은 경로에 `GET`을 보내면 여전히 axum 자신의 `405`가
돌아옵니다.

### router state

AG-UI handler는 요청만 읽습니다. 그래서 **모든** router state `S`에 대해 `Handler<_, S>`입니다.
agent를 얹는 일은 `S`에 아무 제약도 더하지 않습니다. axum 자신의
`Clone + Send + Sync + 'static` 말고는 말입니다. `Router<()>`에서도 `Router<AppState>`에서도
똑같이 돕니다. `with_state`를 부르기 전이어도 그렇습니다.

애플리케이션 state에서 값이 필요한 agent라면 만들어질 때 그것을 붙잡아 두어야 합니다.

```rust
use ag_ui_axum::RouterExt;
use ag_ui_core::RunOutcome;
use ag_ui_server::{Agent, Result, RunContext};
use axum::Router;
use std::sync::Arc;

#[derive(Clone)]
struct Catalog;

#[derive(Clone)]
struct AppState {
    catalog: Arc<Catalog>,
}

struct CartAgent {
    catalog: Arc<Catalog>,
}

impl Agent for CartAgent {
    type State = ();

    async fn run(&self, ctx: &mut RunContext<()>) -> Result<RunOutcome> {
        let _ = &self.catalog;
        ctx.say("Your cart is empty.")?;
        Ok(RunOutcome::Success)
    }
}

fn build(state: AppState) -> Router {
    let agent = CartAgent {
        catalog: Arc::clone(&state.catalog),
    };

    Router::new()
        .route_agui("/agent", agent)
        .with_state(state)
}

fn main() {
    let _ = build(AppState { catalog: Arc::new(Catalog) });
}
```

AG-UI handler 안에서 `State`를 추출한다면 한 줄짜리 마운트가 애플리케이션 하나의 state 타입에
묶여 버립니다. 이 API의 취지와 정반대입니다.

## endpoint가 응답하는 것

| 상황 | 응답 |
| --- | --- |
| run — 어떻게 끝나든 | `200`, `text/event-stream`, `RUN_FINISHED`나 `RUN_ERROR`로 끝남 |
| 본문이 AG-UI JSON이 아님 | `400`과 어느 필드인지 밝히는 JSON 응답 |
| `Content-Type`이 JSON이 아님 | `415` |
| 본문이 크기 제한을 넘음 | `413` |
| `Accept`가 이 빌드가 낼 수 있는 것을 모두 배제함 | `406` |
| `POST` 아닌 모든 메서드 | axum이 주는 `405` |

거절할 때에도 맨 상태 라인이 아니라 JSON 객체로 답합니다. 호출자가 사람이 아니라
프로그램이기 때문입니다.

```json
{"code": "INVALID_INPUT", "message": "missing field `messages` at line 1 column 34"}
```

*실패한* run도 여전히 `200`입니다. agent가 실패할 수 있는 시점이면 상태 라인은 이미 한참 전에
나갔습니다. 그래서 그 실패는 뚝 끊기는 연결이 아니라 온전한 stream 안의 `RUN_ERROR` event입니다.
덕분에 client는 "agent가 오류를 냈다"와 "네트워크가 죽었다"를 구별할 수 있습니다. 마땅한 답이
없는 유일한 경우는 panic하는 agent입니다. panic은 hyper의 연결 태스크로 unwind되고, client는
잘린 stream을 봅니다.

event 하나는 그 event의 JSON을 담은 SSE 프레임 하나입니다.

```rust
use ag_ui_core::{Event, SseFormatter};

fn main() {
    let formatter = SseFormatter::new();
    let frame = formatter
        .encode_to_string(&Event::text_message_content("run-1-msg-1", "Hello!"))
        .expect("an event always serializes");

    assert_eq!(
        frame,
        "data: {\"type\":\"TEXT_MESSAGE_CONTENT\",\"messageId\":\"run-1-msg-1\",\"delta\":\"Hello!\"}\n\n"
    );
}
```

응답에는 `cache-control: no-cache, no-store, no-transform`, `x-accel-buffering: no`,
`vary: accept`도 함께 실립니다. 중요한 쪽은 `no-transform`입니다. 이 stream을 gzip으로 압축하는
프록시는 버퍼링도 합니다. 그런데 stream의 요점은 token이 하나씩 도착한다는 것입니다. nginx용
헤더는 같은 동작에서 빠져나오는 그쪽의 수단이고, 다른 곳에서는 아무 효과가 없습니다.

## content negotiation

`negotiate`는 무엇으로 답할지 정합니다. 답이 "없음"일 때는 거절합니다. `application/xml`을
요구한 client는 `406`을 받습니다. 읽지도 못할 SSE stream 대신 말입니다. `Accept`가 없거나 비어
있으면 `*/*`로 봅니다.

```rust
use ag_ui_axum::negotiate;

fn main() {
    assert!(negotiate(None).is_ok());
    assert!(negotiate(Some("")).is_ok());
    assert!(negotiate(Some("text/event-stream")).is_ok());
    assert!(negotiate(Some("text/*;q=0.4, application/json")).is_ok());

    assert!(negotiate(Some("application/xml")).is_err());
    assert!(negotiate(Some("*/*;q=0")).is_err());
}
```

## 기본값 바꾸기

`AgentEndpoint`는 agent와 그 run별 설정을 합친 것입니다. `route_agui_with`가 그것을 얹습니다.

```rust
use ag_ui_axum::{AgentEndpoint, RouterExt};
use ag_ui_core::RunOutcome;
use ag_ui_server::{Agent, FilterToolCalls, Result, RunContext};
use axum::Router;
use std::time::Duration;

struct CartAgent;

impl Agent for CartAgent {
    type State = ();

    async fn run(&self, _ctx: &mut RunContext<()>) -> Result<RunOutcome> {
        Ok(RunOutcome::Success)
    }
}

fn main() {
    let endpoint = AgentEndpoint::new(CartAgent)
        .transformer(|| FilterToolCalls::deny(["internal_debug"]))
        .keep_alive(Duration::from_secs(15))
        .echo_input(false);

    let app: Router = Router::new().route_agui_with("/agent", endpoint);
    let _ = app;
}
```

- **`transformer`**는 transformer가 아니라 *클로저*를 받습니다. 쓸모 있는
  `StreamTransformer`는 모두 작은 상태 기계입니다. `FilterToolCalls`는 자기가 걸러 낸 call id를
  기억합니다. 인스턴스 하나를 동시에 도는 여러 run이 나눠 쓰면 한 run의 상태가 다른 run으로
  새어 나갑니다. endpoint는 만드는 방법만 저장해 두고 요청마다 새 chain을 세웁니다.
- **`keep_alive`**는 run이 그 시간 동안 아무것도 내놓지 않으면 SSE 주석을 보냅니다. 기본은
  꺼짐입니다. agent와 브라우저 사이의 무언가가 유휴 연결을 닫을 때 켜십시오. 대부분의 리버스
  프록시가 30~60초에 그렇게 합니다. 느린 첫 token이 걸릴 수 있는 시간 안쪽입니다.
- **`echo_input`**은 `RUN_STARTED`에 요청을 되비춰 싣습니다. 그러면 기록한 stream을 원래 HTTP
  본문 없이도 재생할 수 있습니다. 기본은 꺼짐입니다. 프로토콜에서 가장 큰 페이로드이기
  때문입니다.

## 연결이 끊기면 cancellation

응답 본문이 run을 소유합니다. stream을 polling하는 일이 곧 agent를 실행하는 일이므로, 본문과
run의 수명은 정확히 같습니다. client가 사라지면 hyper가 본문을 드롭하고 run도 함께 사라집니다.
여기까지는 저절로 됩니다.

저절로 되지 않는 것이 있습니다. run이 자기 *바깥*에서 건드린 것들에게 알리는 일입니다. spawn된
tool call, 날아가 있는 model 요청 말입니다. 그래서 본문은 guard도 함께 쥐고 있습니다. 드롭될 때
run의 `CancellationToken`을 발동시키는 guard입니다. run이 무사히 끝났다면 스스로 해제되어,
완료된 run이 취소되었다고 보고되는 일은 없습니다. 이 이야기의 agent 쪽은
[error와 cancellation](/ag-ui-rust/ko/server/errors/)에 있습니다.

## 요청을 직접 읽기

`AgUiInput`은 평범한 axum extractor입니다. 그래서 요청을 먼저 들여다봐야 하는 손수 쓴 handler도
얹어 둔 endpoint와 똑같은 방식으로 본문을 파싱합니다. 인증, 테넌트 라우팅, 어느 agent를 돌릴지
정하는 경로 조각 같은 경우입니다.

```rust
use ag_ui_axum::AgUiInput;
use axum::Router;
use axum::extract::Path;
use axum::routing::post;

async fn handler(Path(agent): Path<String>, AgUiInput(input): AgUiInput) -> String {
    format!("{agent} runs thread {}", input.thread_id)
}

fn main() {
    let app: Router = Router::new().route("/agents/{agent}", post(handler));
    let _ = app;
}
```

거절 타입은 `ag_ui_axum::Error`이고 `IntoResponse`를 구현합니다. `AgUiInput`을 받으면 axum이
`4xx`를 대신 답해 줍니다. `Result<AgUiInput, Error>`를 받으면 실패를 먼저 들여다볼 수 있습니다.

`SseResponse`가 나머지 절반입니다. run을 시작하기 전에 자기 할 일을 먼저 하는 handler를 위한
것입니다. `route_agui`가 바로 이것에 기본값을 채워 넣은 것입니다.

```rust
use ag_ui_axum::SseResponse;
use ag_ui_core::{RunAgentInput, RunOutcome};
use ag_ui_server::{Agent, Result, RunContext, Runner};

struct Greeter;

impl Agent for Greeter {
    type State = ();

    async fn run(&self, ctx: &mut RunContext<()>) -> Result<RunOutcome> {
        ctx.say("hi")?;
        Ok(RunOutcome::Success)
    }
}

fn serve(
    accept: Option<&str>,
    input: RunAgentInput,
) -> axum::response::Result<axum::response::Response> {
    let response = SseResponse::negotiate(accept)?;

    let runner = Runner::new(Greeter);
    // `run`이 runner를 소비하기 *전에* token을 받아 둡니다.
    let response = response.cancellation(runner.cancellation_token());

    Ok(response.stream(runner.run(input)))
}

fn main() {
    let response = serve(Some("text/event-stream"), RunAgentInput::new("t", "r"))
        .expect("SSE is acceptable");
    assert_eq!(response.headers()["content-type"], "text/event-stream");

    assert!(serve(Some("application/xml"), RunAgentInput::new("t", "r")).is_err());
}
```

## 왜 `AgUiLayer`가 없는가

tower layer는 `Service`를 감쌉니다. 그래서 `Request`와 `Response`를 봅니다. 그때쯤이면 event는
이미 SSE 본문으로 직렬화된 뒤입니다.

거기서 `StreamTransformer`를 적용하려면 프레임을 다시 event로 파싱하고, 변환하고, 다시
인코딩해야 합니다. 더 느립니다. 경계에서 정보가 샙니다. 그리고 그 layer가 어쩌다 함께 덮은
*다른* 라우트의 본문까지 조용히 망가뜨립니다. `AgentEndpoint::transformer`는 event에 아직 타입이
붙어 있는 자리에서, run마다 chain 하나씩 transformer를 적용합니다.

정작 필요한 layer들은 tower가 이미 제공합니다. CORS, 인증, 타임아웃, 트레이싱, 압축 말입니다.
다른 라우트에서와 똑같이 이 endpoint와도 조합됩니다.

## API

- [`ag_ui_axum::RouterExt`](/ag-ui-rust/api/ag_ui_axum/trait.RouterExt.html)와
  [`AgentEndpoint`](/ag-ui-rust/api/ag_ui_axum/struct.AgentEndpoint.html)
- [`ag_ui_axum::AgUiInput`](/ag-ui-rust/api/ag_ui_axum/struct.AgUiInput.html)
- [`ag_ui_axum::SseResponse`](/ag-ui-rust/api/ag_ui_axum/struct.SseResponse.html)와
  [`negotiate`](/ag-ui-rust/api/ag_ui_axum/fn.negotiate.html)
- [`ag_ui_axum::Error`](/ag-ui-rust/api/ag_ui_axum/enum.Error.html)
- 직접 transport를 만들 때를 위한
  [`ag_ui_server::Runner`](/ag-ui-rust/api/ag_ui_server/struct.Runner.html)
- wire 반대편: [transport](/ag-ui-rust/ko/client/transports/)
