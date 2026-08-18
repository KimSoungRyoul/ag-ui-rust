---
title: platform과 MSRV
description: 지원하는 최소 Rust version, CI가 build하는 target, wasm 이야기, 그리고 executor에 종속되지 않는다는 약속이 실제로 어떻게 강제되는지.
---

## MSRV는 1.85이고 여유가 없습니다

workspace는 `rust-version = "1.85"`와 `edition = "2024"`를 선언합니다. 둘은 같은 사실입니다.
edition 2024는 정확히 1.85에서 안정화되었습니다. 그래서 1.85는 아래로 여유가 있는 조심스러운
하한이 아닙니다. 이 code를 build할 수 있는 첫 compiler입니다.

실질적인 결과가 따릅니다. 보통 MSRV는 불편해지면 풀 수 있는 약속입니다. 여기서는 edition을
바꾸지 않고는 한 release도 낮출 수 없습니다. edition 변경은 성격이 다르고 훨씬 큰 일입니다.
1.85보다 낮은 곳에 묶여 있다면 이 SDK는 쓸 수 없습니다. 어떤 feature flag도 그것을 바꾸지
못합니다.

CI는 그 toolchain을 정확히 설치하는 job으로 이 선을 지킵니다:

```sh
# .github/workflows/ci.yml, job `msrv`
cargo check --workspace --all-features --all-targets
```

`--all-features`가 중요합니다. feature는 눈치채지 못한 채 더 새로운 compiler를 요구하게 되는
통로이기 때문입니다. `--all-targets`도 한 단계 아래에서 같은 이유로 중요합니다. test와 예제도
code입니다.

repository에는 `rust-toolchain.toml`이 없습니다. local build는 가진 toolchain을 그대로
씁니다. 의도한 것입니다. 고정은 검사로 작동하는 CI에 있어야 합니다. 작업 tree에 두면 모든
기여자의 compiler를 조용히 낮춥니다.

## CI가 무엇을, 무엇을 위해 build하는가

| target | 범위 | 검사 방식 |
| --- | --- | --- |
| host (`ubuntu-latest`) | workspace 전체 | `cargo test --workspace --all-features`, 그리고 `-D warnings`로 도는 clippy |
| host, Rust 1.85 | workspace 전체 | `cargo check --workspace --all-features --all-targets` |
| `wasm32-unknown-unknown` | crate 넷, 아래 참조 | `cargo check --target wasm32-unknown-unknown` |

wasm 행은 검사 다섯 번입니다. 각각에 붙은 feature 집합도 주장의 일부입니다:

```sh
# .github/workflows/ci.yml, job `executor-agnostic`
cargo check -p ag-ui-core   --target wasm32-unknown-unknown --no-default-features
cargo check -p ag-ui-core   --target wasm32-unknown-unknown --all-features
cargo check -p ag-ui-server --target wasm32-unknown-unknown --all-features
cargo check -p ag-ui-client --target wasm32-unknown-unknown --no-default-features
cargo check -p ag-ui-a2ui   --target wasm32-unknown-unknown --all-features
```

`ag-ui-client`는 `--no-default-features`와 함께 나옵니다. 기본 `http` feature가 `reqwest`를
끌어오고, 그것은 이 workspace가 하는 wasm 이야기가 아니기 때문입니다. `ag-ui-axum`은 아예
나오지 않습니다. 빠뜨린 것이 아닙니다. web binding입니다. axum과 tower, tokio로 server를
돌립니다. browser에서 돌 물건이 아닙니다.

이것들은 test 실행이 아니라 `cargo check`입니다. crate가 native 전용 가정 없이 그 target으로
*compile된다는* 것만 증명합니다. browser에서 무언가 실행된다는 증명은 아닙니다. 이
repository는 headless browser를 띄우지 않습니다. "browser에서 검증됨"이 아니라 "type과
dependency graph가 wasm에 깨끗함"으로 읽으십시오.

## web binding 아래는 executor에 종속되지 않습니다

`ag-ui-core`, `ag-ui-server`, `ag-ui-client`는 tokio가 아니라 `futures`의 기본 요소를 씁니다.
emit path가 가장 분명한 사례입니다. emitter handle이
`futures_channel::mpsc::UnboundedSender`에 밀어 넣고 transport 층이 그것을 비웁니다. 뻔한
대안은 `tokio::sync::mpsc`였을 것입니다. tokio는 `ag-ui-axum`에서만 workspace에 들어옵니다.

덕분에 tokio 아닌 executor도, browser도 쓸 만한 host로 남습니다. emit path가 처음부터 끝까지
동기인 이유이기도 합니다. handle은 `Drop`에서 종료 event를 emit합니다. `Drop`은 async일 수
없습니다. 그래서 handle은 emit하면서 `await`할 수 없습니다.

`ag-ui-client`는 **`http` feature가 off일 때만** executor에 종속되지 않습니다. `http`는
`reqwest`를, `reqwest`는 tokio를 끌어옵니다. 사고가 아니라 의도한 기본값입니다. 대부분의
소비자는 HTTP transport를 원하고 이미 tokio 위에 있습니다. crate의 나머지는 평범한 동기 state
machine입니다. event application, normalisation, verification이 그렇습니다. 그래서 `http`를 끄면 transport
자리에 구멍 하나만 난 client가 남습니다. 나머지는 그대로 쓸 수 있습니다.

그 구멍은 trait입니다.
[`Transport`](/ag-ui-rust/api/ag_ui_client/transport/trait.Transport.html)이고, 채우는 것은
여러분입니다:

```rust
use ag_ui_client::transport::{Transport, TransportFuture, boxed_stream};
use ag_ui_client::Result;
use ag_ui_core::{Event, RunAgentInput};
use futures_util::stream;

/// 정해진 script를 재생합니다. `fetch`와 `EventSource` 위에 세운
/// browser transport도 같은 모양입니다.
struct Canned(Vec<Event>);

impl Transport for Canned {
    // 연결 실패는 future가 내는 error입니다. stream 도중의 실패는 stream 안의
    // error item입니다. 그 구분이 interface의 전부입니다.
    fn run(&self, _input: RunAgentInput) -> TransportFuture {
        let events: Vec<Result<Event>> = self.0.iter().cloned().map(Ok).collect();
        Box::pin(async move { Ok(boxed_stream(stream::iter(events))) })
    }
}
```

그 module에는 wasm을 위한 작은 배려가 둘 있습니다. `EventStream`과 `TransportFuture`는 wasm을
*뺀* 모든 곳에서 `Send`입니다. wasm에서 transport를 얹을 browser API는 단일 thread이고
`Send`가 아닙니다. 거기서까지 `Send`를 요구하면 wasm 경우를 만족시킬 수 없습니다. transport를
추상화한 이유가 바로 그 경우입니다. `boxed_stream`에도 짝이 되는 signature 한 쌍이 있습니다.

이 workspace는 browser transport를 제공하지 않습니다. trait와 `cfg`가 배려의 전부입니다.
`fetch` 기반 구현은 여러분 몫입니다.

## tokio 금지는 실제로 어떻게 강제되는가

wasm build는 tokio 금지를 증명하지 **않습니다**. CI도 그런 척하지 않습니다. tokio의 `rt`,
`sync`, `macros`, `io-util`, `time` feature는 모두 `wasm32-unknown-unknown`으로 compile됩니다.
CI 주석에 그 실험이 기록되어 있습니다. `ag-ui-server`의 `[dependencies]`에
`tokio.workspace = true`를 넣고도 위 wasm 검사를 전부 통과했습니다. build는 초록이었습니다.

그래서 이 보증을 떠받치는 것은 dependency graph입니다. CI는 그 graph에 직접 단언합니다:

```sh
# .github/workflows/ci.yml, job `executor-agnostic`
cargo tree -p ag-ui-core   -e normal --prefix none --no-dedupe --all-features
cargo tree -p ag-ui-server -e normal --prefix none --no-dedupe --all-features
cargo tree -p ag-ui-a2ui   -e normal --prefix none --no-dedupe --all-features
cargo tree -p ag-ui-client -e normal --prefix none --no-dedupe --no-default-features
```

각 tree에서 `tokio v`로 시작하는 줄을 찾습니다. 걸리면 job이 실패합니다. message는 grep이
아니라 설계 결정을 가리킵니다.

그 네 줄에서 짚을 것이 셋입니다.

`-e normal`은 dev-dependencies를 뺍니다. test는 tokio를 마음껏 써도 되고, 실제로 씁니다.
`ag-ui-server`의 `[dev-dependencies]`가 `#[tokio::test]`를 위해 끌어옵니다. 이 약속이 말하는
것은 crate의 *소비자*가 받는 것입니다. 그것이 normal graph입니다.

`ag-ui-client`는 `--no-default-features`로 검사합니다. `http`가 on이면 그 단언은 그냥 거짓이기
때문입니다. 검사의 범위가 곧 주장의 범위입니다.

script는 `grep -q`를 피합니다. pipe를 일찍 닫으면 `cargo tree`에 `SIGPIPE`가 갑니다.
`pipefail` 아래에서는 진짜로 걸린 것이 조용한 통과로 바뀝니다. 그래서 tree 전체를 출력하고
`-q` 없이 grep합니다.

같은 검사를 local에서도 돌릴 수 있습니다:

```sh
cargo tree -p ag-ui-server --all-features -e normal --prefix none --no-dedupe | grep '^tokio v'
```

이 workspace에서는 아무것도 나오지 않습니다. `ag-ui-server`의 normal graph는 자기까지 26개
crate이고, 그중 tokio는 없습니다. 같은 명령을 `ag-ui-client --all-features`에 돌리면
`tokio v1.53.1`이 나옵니다. `reqwest`를 거쳐 닿습니다. `ag-ui-client --no-default-features`에
대해서는 아무것도 나오지 않습니다.

## 요약

| 보증 | 강제하는 것 |
| --- | --- |
| Rust 1.85에서 build된다 | `msrv` job, 1.85.0으로 고정한 toolchain의 `cargo check` |
| core / server / client / a2ui가 wasm으로 compile된다 | `executor-agnostic` job, `cargo check --target wasm32-unknown-unknown` |
| 그 crate들의 dependency graph에 tokio가 없다 | `executor-agnostic` job, `cargo tree -e normal` |
| 중요한 feature 조합이 모두 build된다 | `features` job — [feature flag](/ag-ui-rust/ko/reference/features/) 참조 |
