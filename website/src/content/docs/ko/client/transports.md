---
title: transport
description: client에서 비동기인 유일한 계층 — HTTP transport, SSE decoder, replay transport, 그리고 직접 하나 만드는 데 필요한 것.
---

`ag_ui::client`의 나머지는 전부 동기입니다. 적용, chunk normalization,
verification 모두 평범한 state machine입니다. loop에서도, test에서도,
event handler에서도 직접 돌릴 수 있습니다. transport는 바깥 세상과
이야기하는 유일한 계층입니다. `async`가 나타나는 유일한 자리이기도
합니다.

그 덕분에 crate의 나머지는 어떤 executor 위에서든 돌아갑니다. executor
없이도 돌아갑니다. 하나뿐인 비동기 계층이 trait인 이유도 같습니다.
wasm frontend, 프로세스 안의 agent, websocket, 녹화해 둔 fixture. 각각이
하나의 `impl Transport`입니다. 그 위의 어떤 것도 달라지지 않습니다.

## trait

```rust
// 인용이 아니라 여기에 다시 적은 전부입니다. signature가 옮겨 가면 이
// 페이지가 컴파일되지 않도록.
use ag_ui::client::transport::TransportFuture;
use ag_ui::RunAgentInput;

trait Transport {
    fn run(&self, input: RunAgentInput) -> TransportFuture;
}
```

method 하나입니다. `RunAgentInput`을 건네면 future가 돌아옵니다. 그
future는 event stream으로 귀결됩니다. *연결*에 실패하는 것은 future가
내는 error입니다. stream 도중에 실패하는 것은 stream 안의 error
항목입니다. 둘은 다른 일이고, client도 다르게 말하고 싶어 합니다.

`TransportFuture`는
`Pin<Box<dyn Future<Output = Result<EventStream>> + Send>>`입니다.
여기 어디에도 lifetime은 적혀 있지 않습니다. 그렇다면 boxed trait
object의 기본값인 `'static`입니다. 이 점이 구조를 떠받칩니다.

transport는 보통 `Session` 안에 들어 있습니다. session은 event가 도착할
때마다 자기 state를 바꿉니다. 돌려받은 future가 transport를 borrow하고
있다면 그 borrow는 run이 끝날 때까지 살아 있습니다. 그러면 session은
streaming 중에 자기 자신을 건드릴 수 없습니다. 그래서 `run`은 필요한
것을 clone합니다. `reqwest::Client`가 바로 그 용도로 설계되어 있습니다.
future는 홀로 섭니다.

`T`가 transport라면 `&T`, `Box<T>`, `Arc<T>`도 transport입니다. 덕분에
client는 runtime에 transport를 고를 수 있습니다. generic parameter를
application 전체에 끌고 다니지 않아도 됩니다.

```rust
// src/main.rs
use ag_ui::client::transport::{ReplayTransport, Transport};
use ag_ui::client::{RunEnd, Session, Update};
use ag_ui::Event;
use futures_util::StreamExt;

#[tokio::main]
async fn main() {
    // 실제 application이라면 `--replay` flag가 fixture와
    // `HttpTransport` 중 하나를 고르는 자리입니다.
    let transport: Box<dyn Transport> = Box::new(ReplayTransport::new([
        Event::run_started("thread-1", "run-1"),
        Event::run_finished_success("thread-1", "run-1"),
    ]));

    let mut session = Session::<_>::new(transport, "thread-1");
    let updates: Vec<_> = session.send("hello").collect().await;

    assert!(matches!(updates.last(), Some(Update::Done(RunEnd::Success { .. }))));
}
```

wasm에서는 `EventStream`과 `TransportFuture` alias가 `Send` bound를 떼어
냅니다. 거기서 transport를 얹게 될 browser API는 단일 thread입니다.
애초에 `Send`가 아닙니다. `Send`를 요구하면 wasm이라는 경우를 만족시킬
방법이 없어집니다. 이 crate가 애초에 transport를 추상화한 이유가 바로 그
경우입니다.

## `HttpTransport`

기본값이며 `http` feature flag 뒤에 있습니다. `RunAgentInput`을 JSON으로
한 번 POST 합니다. `text/event-stream` 응답 하나를 frame 단위로
decode합니다. crate 안에서 HTTP client를 끌어오는 유일한 곳입니다.

```rust
// src/main.rs
use ag_ui::client::transport::HttpTransport;
use std::time::Duration;

fn main() -> Result<(), ag_ui::client::Error> {
    let transport = HttpTransport::builder("https://example.com/agent")
        .header("authorization", "Bearer token")
        .header("x-tenant", "acme")
        // 연결 설정에만 제한을 겁니다. stream 자체는 제한하지 않습니다.
        .connect_timeout(Duration::from_secs(5))
        .build()?;

    assert_eq!(transport.url().as_str(), "https://example.com/agent");
    // 호출자가 말하지 않아도 모든 요청이 stream 형식을 요구합니다.
    assert_eq!(transport.headers()["accept"], "text/event-stream");
    Ok(())
}
```

header 값은 setter가 아니라 `build`에서 검사합니다. 그래야 setter를 이어
붙인 chain이 chain으로 남습니다. 단계마다 `Result`를 꿸 필요가 없습니다.
parse되지 않는 URL, header 이름이 아닌 header 이름, 만들어지지 않는
client는 모두 `Error::Config`입니다.

:::caution[`timeout`과 `connect_timeout`은 서로의 변형이 아닙니다]
`timeout`은 run *전체*에 제한을 겁니다. 연결, header, body streaming까지
전부입니다. 그보다 오래 생각하는 agent는 답하는 도중에 stream이
잘립니다. client에는 잘려 나간 run으로 도착합니다. 오래 도는 agent에
필요한 것은 `connect_timeout`입니다. 설정에만 제한을 걸고 stream은
제한하지 않습니다.
:::

`client(…)`는 미리 설정해 둔 `reqwest::Client`를 받습니다. proxy, 직접
지정한 TLS 루트 인증서, application의 나머지와 공유하는 connection pool
같은 것들을 위해서입니다.

2xx가 아닌 응답은 stream이 되지 않습니다. status와 body의 앞 2048자를
실은 `Error::Http`가 됩니다. gateway의 HTML error 페이지를 읽기에는
충분한 길이입니다. log 한 줄이 megabyte로 불어나지도 않습니다.

`HttpAgent`는 같은 transport를 한 계층 아래에서 쓰는 것입니다. 이쪽
builder로 넘겨주는 builder를 가진 `RemoteAgent<HttpTransport>`입니다.

## `ReplayTransport`

살아 있는 agent를 상대로 client를 test하는 것은 느립니다. 불안정하고,
model까지 있어야 합니다. 게다가 그럴 필요도 없습니다. 대화에서 agent가
맡은 절반은 event 목록에 지나지 않습니다.

```rust
// tests/client.rs
use ag_ui::client::{Session, transport::ReplayTransport};
use ag_ui::{Event, Interrupt};
use futures_util::StreamExt;
use serde_json::json;

#[tokio::main]
async fn main() {
    // run 하나에 목록 하나입니다. 그래서 human in the loop 왕복도
    // script로 짤 수 있습니다. 첫 run은 멈추고, 두 번째 — 재개 — 가
    // 이어 갑니다.
    let transport = ReplayTransport::with_runs([
        vec![
            Event::run_started("thread-1", "run-1"),
            Event::run_finished_interrupt(
                "thread-1",
                "run-1",
                vec![Interrupt::new("i-1", "tool_approval")],
            ),
        ],
        vec![
            Event::run_started("thread-1", "run-2"),
            Event::run_finished_success("thread-1", "run-2"),
        ],
    ]);

    // clone해도 script와 기록은 공유됩니다. 그래서 test는 session에
    // 하나를 넘겨준 뒤에도 handle을 들고 있을 수 있습니다.
    let mut session = Session::<_>::new(transport.clone(), "thread-1");
    session.send("delete the staging database").collect::<Vec<_>>().await;

    let paused = session.interrupts().to_vec();
    session.resume(&paused[0], json!({ "approved": true })).collect::<Vec<_>>().await;

    // client가 실제로 보낸 것입니다. 재개가 올바른 답을 실어 갔는지
    // test가 단언하는 방법입니다.
    let requests = transport.requests();
    assert_eq!(requests.len(), 2);
    assert_eq!(requests[1].resume.as_deref().unwrap()[0].interrupt_id, "i-1");
    assert_eq!(transport.remaining(), 0);
}
```

`new`는 run 하나만 script로 짭니다. 그 뒤의 run에는 모두 error로
답합니다. 대개는 그것이 원하는 동작입니다. 실수로 두 번 실행되는 test는
그렇다고 말해 주어야 합니다.

## SSE decoder

HTTP transport라면 무엇이든 decoder가 필요합니다. 그래서 decoder는
`http` feature flag와 별개로 제공되고 단독으로도 쓸 수 있습니다.
network가 먹여 주는 wire 형식 parser입니다. byte가 어떻게 도착하는지에
대해서는 아무것도 가정하지 않습니다. chunk는 줄 중간에서 끊깁니다.
UTF-8 시퀀스 중간에서도 끊깁니다. 형식이 요구하는 마지막 빈 줄 없이
끝나기도 합니다.

```rust
// src/transport.rs
use ag_ui::client::transport::SseDecoder;

fn main() -> Result<(), ag_ui::client::Error> {
    let mut decoder = SseDecoder::new();

    // proxy의 heartbeat, 그리고 반쪽짜리 event.
    decoder.push(b": keep-alive\n\ndata: {\"type\":\"RUN_ERROR\",\"mes")?;
    assert!(decoder.next_frame()?.is_none());

    decoder.push(b"sage\":\"boom\"}\n\n")?;
    let frame = decoder.next_frame()?.expect("a complete frame");
    assert_eq!(frame.into_event()?.event_type().as_str(), "RUN_ERROR");
    Ok(())
}
```

실제 server와 proxy가 이 모든 것을 합니다. 그래서 decoder도 전부
처리합니다. 여러 줄에 걸쳐 반복되고 `\n`으로 다시 이어 붙는 `data:`.
아무것도 dispatch하지 않는 주석 줄. 빈 줄 없이 끝나는 body — 이때
`finish`는 마지막 frame을 버리지 않고 dispatch합니다. `\n`과 `\r\n`과
홀로 오는 `\r` 줄바꿈. 두 chunk에 걸쳐 쪼개진 `\r\n`. 맨 앞의 byte order
mark. 콜론이 없는 field. 그리고 `data` field가 없는 frame. 형식은 그런
frame을 dispatch하지 말라고 정합니다.

거부하는 것은 둘입니다. 올바르지 않은 UTF-8, 그리고 `max_frame_size`보다
큰 단일 frame입니다. 기본값은 8 MiB입니다. 그러지 않으면 끝나지 않는 한
줄이 반대편이 조종하는 무한 할당이 됩니다. 상한은 chunk가 아니라
*frame*에 걸립니다. 한 번의 읽기가 완전한 frame 천 개를 실어 오는 것은
흔한 일입니다. 그것을 frame당 한도에 합산하면 멀쩡한 server를 거부하게
됩니다.

`decode_events`는 byte chunk stream을 event stream으로 바꾸는
adapter입니다. byte stream에서 난 error는 stream을 끝냅니다. payload가
올바른 event가 아닌 frame은 error 항목이 되고 stream은
**계속됩니다**. 어긋난 event 하나가 run의 나머지를 침묵시켜서는 안 되기
때문입니다.

## 직접 만들기

전부 해서 method 하나입니다. 프로세스 안의 agent나 wasm frontend가
취하는 모양이 이렇습니다. 어디에도 HTTP client는 없습니다.

```rust
// src/transport.rs
use ag_ui::client::transport::{EventStream, Transport, TransportFuture};
use ag_ui::client::{RunEnd, Session, Update};
use ag_ui::{Event, RunAgentInput, TextMessageRole};
use futures_util::StreamExt;

/// 무엇을 요청받든 정해진 event 목록을 내어 줍니다.
#[derive(Clone, Debug)]
struct StaticTransport {
    events: Vec<Event>,
}

impl Transport for StaticTransport {
    fn run(&self, _input: RunAgentInput) -> TransportFuture {
        // borrow가 아니라 clone입니다. future가 이 호출보다 오래 삽니다.
        let events = self.events.clone();
        Box::pin(async move {
            let stream = futures_util::stream::iter(events.into_iter().map(Ok));
            Ok(Box::pin(stream) as EventStream)
        })
    }
}

#[tokio::main]
async fn main() {
    let transport = StaticTransport {
        events: vec![
            Event::run_started("thread-1", "run-1"),
            Event::text_message_start("msg-1", TextMessageRole::Assistant),
            Event::text_message_content("msg-1", "From somewhere else entirely."),
            Event::text_message_end("msg-1"),
            Event::run_finished_success("thread-1", "run-1"),
        ],
    };

    let mut session = Session::<_>::new(transport, "thread-1");
    let updates: Vec<_> = session.send("hello").collect().await;

    assert!(matches!(updates.last(), Some(Update::Done(RunEnd::Success { .. }))));
    assert_eq!(
        session.applier().text_of("msg-1"),
        Some("From somewhere else entirely.")
    );
}
```

event가 아니라 byte를 읽는 transport는 가운데에 `decode_events`를
둡니다. `boxed_stream`은 그 결과를 trait이 돌려주는 모양으로 box해 주는
helper입니다.

```rust
// src/transport.rs
use ag_ui::client::transport::{Transport, TransportFuture, boxed_stream, decode_events};
use ag_ui::client::{RunEnd, Session, Update};
use ag_ui::{Event, RunAgentInput, SseFormatter};
use futures_util::StreamExt;

/// 모든 run에 녹화해 둔 response body로 답합니다.
struct Recorded(String);

impl Transport for Recorded {
    fn run(&self, _input: RunAgentInput) -> TransportFuture {
        let body = self.0.clone();
        Box::pin(async move {
            // 여기서는 chunk 하나입니다. 실제 body는 network가 정하는
            // 만큼 나뉘어 도착하고, decoder는 그것을 전제로 쓰였습니다.
            let chunks = futures_util::stream::iter([Ok::<_, std::io::Error>(body)]);
            Ok(boxed_stream(decode_events(chunks)))
        })
    }
}

#[tokio::main]
async fn main() {
    let sse = SseFormatter::new();
    let mut body = String::new();
    for event in [
        Event::run_started("thread-1", "run-1"),
        Event::run_finished_success("thread-1", "run-1"),
    ] {
        body.push_str(&sse.encode_to_string(&event).expect("encodes"));
    }

    let mut session = Session::<_>::new(Recorded(body), "thread-1");
    let updates: Vec<_> = session.send("hello").collect().await;

    assert!(matches!(updates.last(), Some(Update::Done(RunEnd::Success { .. }))));
}
```

별난 runtime에서 나온 error라고 이 crate의 error enum에 variant를 더할
필요는 없습니다. `Error::transport(e)`가
`std::error::Error + Send + Sync + 'static`이기만 하면 무엇이든
감쌉니다.

## `http` 끄기

`http`는 기본으로 켜져 있고 `reqwest`를 끌어옵니다. 이것을 끄는 것이
crate를 wasm에서 쓸 수 있게 유지하는 방법입니다. 끄면 `HttpTransport`와
`HttpAgent`도 함께 사라집니다. `Transport`는 직접 가져오세요.

```toml
[dependencies.ag-ui]
version = "0.3"
default-features = false
features = ["client", "sse"]
```

CI는 이 주장의 양쪽 절반을 모두 강제합니다. feature flag 밖의 무언가가
`reqwest`에 손을 뻗으면 `cargo check -p ag-ui
--no-default-features --features client --target wasm32-unknown-unknown`이 실패합니다.
별도의 job은 그 구성의 의존성 그래프에 `tokio`가 없음을 단언합니다.
tokio 자체는 wasm으로 컴파일되기 때문입니다. wasm build가 초록이라는
것만으로는 이를 잡지 못합니다. `reqwest`를 조건 없이 넣는 manifest
수정은 모든 컴파일을 통과합니다. 그래서
`crates/ag-ui/tests/client_features.rs`가 manifest를 읽어 그것까지
검사합니다.

feature flag 각각의 비용은 [feature
flag](/ag-ui-rust/ko/reference/features/) 문서에 있습니다. 무엇이 어디서
빌드되는지는 [platform과 MSRV](/ag-ui-rust/ko/reference/platforms/)에
있습니다.

## 다음

- [session](/ag-ui-rust/ko/client/session/) — transport 위에 무엇이
  얹히는지.
- API 문서의
  [`Transport`](/ag-ui-rust/api/ag_ui/client/transport/trait.Transport.html)와
  [`HttpTransport`](/ag-ui-rust/api/ag_ui/client/transport/http/struct.HttpTransport.html).
