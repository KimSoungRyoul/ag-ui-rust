# Review Desk

AG-UI SDK로 만든 독립 리뷰 애플리케이션이다. 브라우저와 CLI에서 같은 Rust client workflow를 사용하며, 서버에는 실제 loopback HTTP/SSE로 연결한다. 정해진 로컬 리뷰 자료를 사용하므로 API 키가 필요 없다.

## 실행

```sh
# 브라우저: http://127.0.0.1:8091
cargo run -p review-desk -- serve

# 임시 HTTP 서버를 띄우고 전체 흐름 실행 후 종료
cargo run -p review-desk -- demo

# snapshot을 애플리케이션 파일로도 저장
cargo run -p review-desk -- demo /tmp/review-snapshot.json

# 이미 실행 중인 이 앱 서버를 별도 client process에서 사용
cargo run -p review-desk -- run http://127.0.0.1:8091/agent

cargo test -p review-desk --all-targets
```

공식 A2UI renderer core와의 선택적 상호운용 검증은 [interop/README.md](interop/README.md)에 있다. `@a2ui/web_core@0.11.0`에 실제 Rust 송신 메시지를 수정 없이 넣어 create/edit/null/생략 삭제/배열 슬롯/동일 ID 재생성을 확인한다. 기본 Cargo 실행에는 npm이 필요 없다.

`serve`에 `127.0.0.1:8092` 같은 주소를 넘기면 포트를 바꿀 수 있다. 테스트는 임시 포트를 사용한다.

## 화면에서 확인하는 흐름

1. `Run local review`를 누르면 하나의 `HttpAgent`에서 서로 독립적인 두 `Thread`를 만든다.
2. 첫 대화는 release notes를 검토한다. `checklist`와 `risk-check`의 텍스트·공유 state·반환 결과가 각 invocation ID에 연결된다. progress는 고수준 Thread의 `on_event`로 관찰한다.
3. 두 번째 대화는 broken dependency를 검토한다. `risk-check`가 실패했다고 명시하고, 부모 리뷰는 수동 확인 항목을 남긴다. 첫 대화의 상태는 변하지 않는다.
4. 첫 대화가 두 결정을 요청한다. 하나만 답하면 요청 전에 거부되고 pending 두 개가 보존된다. 다음에 승인 하나와 거절 하나를 함께 보내면 완료된다. 여기서 publish는 로컬 상태의 라벨이며 외부 게시를 수행하지 않는다.
5. snapshot을 JSON bytes로 저장·복원하고 다음 run을 실행한다. 이전 메시지 ID와 새 ID의 중복 여부를 확인한다.
6. 비동기 로컬 provider로 A2UI를 만든다. 첫 잘못된 응답은 SDK가 교정 재시도하고, 유효한 결과만 전송한다. 같은 author로 제목만 수정해 보내고 memo의 명시적 null은 유지한다.
7. 오래 대기하는 run의 메시지 시작을 실제로 받은 뒤 `abort_handle().abort()`로 중단하고, 다음 run이 정상 실행되는지 확인한다.

화면의 Wire events에는 decode된 원본 이벤트가, 최종 대화와 subagent 패널에는 SDK가 조립한 상태가 표시된다. 카드의 최종 데이터도 `find_prior_surface_in(thread.messages())`로 복원한다. UI 코드가 AG-UI state reducer를 재구현하지 않는다.

## 사용 API

```rust
let agent = HttpAgent::new(url)?;
let mut thread = agent.thread_with_state("review-primary", ReviewState::default())?;
thread.on_event(|event| {
    // 동기 observer에서는 큐에 넣고 실제 I/O는 큐 소비자가 수행한다.
});

let report = thread.send("review release notes")?.collect_report().await;
// end와 diagnostics를 모두 확인한다.
let saved = serde_json::to_vec(&thread.snapshot())?;
let mut restored = agent.restore_thread_with_state::<ReviewState>(
    serde_json::from_slice(&saved)?,
)?;
```

실제 전체 코드는 [src/client.rs](src/client.rs), 서버는 [src/agent.rs](src/agent.rs)다. 서버의 `subagent_events()`는 이벤트 출처와 lifecycle만 담당하며, checklist를 수행하는 함수와 도메인 판단은 이 애플리케이션에 있다.

A2UI는 `A2uiAuthor::basic(A2uiVersion::V0_9_1)`에서 `create().generate(...).await`와 `edit().generate(...).await`를 사용하고, `A2uiRunContextExt::send_a2ui()`로 전달한다. 수동 재시도 루프나 tool-result envelope 조립은 필요하지 않다.

## 검증과 범위

| 테스트 | 실제 검증 |
|---|---|
| `review_workflow_uses_real_http_and_preserves_conversation_contracts` | 두 thread, progress, 하위 성공/실패, 부분 승인 거부, 승인+거절, JSON snapshot, 고유 ID, A2UI create/edit와 null 보존, abort 이후 정상 run |
| `a_thread_outlives_its_agent_and_unpolled_requests_leave_no_history` | agent drop 이후 thread 사용, poll하지 않은 send의 무효과 |
| `application_validates_approval_payload_before_changing_domain_state` | resolved라는 상태만 신뢰하지 않고 서버가 실제 답을 검사하여 잘못된 payload의 도메인 변경을 거부 |
| `uncertain_approval_survives_disk_roundtrip_and_blocks_resubmission` | 서버가 승인 응답을 받았지만 RunError를 반환한 경우 Unconfirmed 보존, JSON 복원 뒤 재제출 차단 |
| `browser_stream_includes_live_events_and_sdk_assembled_card` | 브라우저 HTML, 실제 SSE 이벤트, child error와 완료 카드 데이터 |

사용 feature는 `ag-ui/{client,http,server,axum}`, `ag-ui-a2ui/ag-ui-server`다. Tokio는 앱 서버·테스트가 선택한 runtime이다. provider callback 자체는 공개 author의 generic Future API를 사용한다.

브라우저 카드 미리보기는 이 예제의 bound title과 memo를 표시한다. 범용 A2UI widget renderer, 실제 LLM 품질, 원격 서버 취소 확인, durable storage와 재실행 보장은 검증하지 않는다. `Unconfirmed`를 해소하는 서버 조회도 별도 앱 계약이 필요하다.

소비자로 사용해 본 결과, HttpTransport를 직접 만들거나 SDK reducer·교정 재시도를 다시 작성할 필요는 없었다. 다중 승인 사전 검사, report의 진단 보존, 복원 API와 명시적 abort를 한 client 흐름에서 사용할 수 있었다. 공유 transport의 인증·도구 설정과 실제 보관소는 애플리케이션이 별도로 구성한다.
