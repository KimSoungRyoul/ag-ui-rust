# SDK consumer applications

`examples/`는 SDK의 공개 API를 사용하는 독립 애플리케이션 모음이다. 각 프로젝트는 자체 `Cargo.toml`, 실행 방법, HTTP 통합 테스트를 유지한다. workspace의 `examples/*`에 자동으로 포함되며 출판하지 않는다.

| 프로젝트 | 역할 | 실행 | 검증하는 사용 흐름 |
|---|---|---|---|
| [review-desk](review-desk/README.md) | 새 API의 기본 사용 흐름과 브라우저 관찰 | `cargo run -p review-desk -- serve` → `http://127.0.0.1:8091` | `HttpAgent → Thread`, 두 대화, progress observer, 승인/거절, snapshot 복원, abort, subagent 출처/결과, 비동기 A2UI create/edit |
| [task-board](task-board/README.md) | 상태를 가진 서버와 대화형 terminal client | `cargo run -p task-board -- serve` / 별도 터미널에서 `cargo run -p task-board -- chat` | 작업 CRUD, 도구 실행, state snapshot/delta, 승인, 명시적 subagent 이벤트 종료, 수동 A2UI 작성 |
| [board-watch](board-watch/README.md) | 다른 팀이 작성한 client 및 잘못된 입력을 내보내는 서버 | `cargo run -p board-watch -- serve-fake` / `cargo run -p board-watch -- watch --url http://127.0.0.1:8090/agent` | chunk 정규화, 병렬 tool call, 다중 승인, 손상/절단 스트림, replay, task-board와 교차 실행 |

## 공통 검증

저장소 루트에서 실행한다. 기본 검증은 API 키 없이 loopback HTTP만 사용한다.

```sh
cargo test --locked -p task-board -p board-watch -p review-desk --all-targets
cargo run --locked -p review-desk -- demo
```

`review-desk demo /tmp/review-snapshot.json`은 실제 HTTP 서버를 임시 포트에 띄우고, SDK snapshot의 JSON 저장·복원까지 실행한다. 브라우저와 CLI는 같은 Rust workflow를 사용한다. `review-desk`의 카드 미리보기는 SDK가 복원한 한 종류의 카드만 표시하며 범용 A2UI renderer 호환성 검증을 대신하지 않는다.

`board-watch/tests/live.rs`의 실제 모델 테스트는 기본적으로 제외된다. [해당 프로젝트 문서](board-watch/README.md)의 환경 변수를 명시한 경우에만 `--ignored`로 실행한다. `task-board --llm`도 선택 사항이다. 키와 외부 모델 성공 여부는 기본 dogfood 통과 조건에 포함하지 않는다.

[review-desk/interop](review-desk/interop/README.md)는 공식 `@a2ui/web_core@0.11.0`과의 별도 상호운용 gate다. Node.js가 있는 환경에서 고정한 lockfile로 `npm ci --ignore-scripts --prefix examples/review-desk/interop` 후 `npm test --prefix examples/review-desk/interop`를 실행한다. Node는 기본 Rust 앱 의존성이 아니다.

## 프로젝트 추가·유지 규칙

- 새 프로젝트는 기존 예제로 검증할 수 없는 사용자 흐름이 있을 때 추가한다. 앱별 README에 목적, 실행 명령, 포트, SDK feature, 테스트와 한계를 기록한다.
- domain state와 도구의 실제 동작은 앱이 소유한다. 메시지 reducer, approval bookkeeping, ID 생성, A2UI 검증·교정 재시도는 SDK API를 사용한다. 앱에서 SDK 결함을 우회하는 공통 로직을 만들지 않는다.
- SDK 변경 시 모든 프로젝트를 함께 컴파일·검증한다. 같은 Rust state 타입을 공유하는 자체 서버/client 검사에 더해 `board-watch → task-board`의 독립 소비자 검증도 유지한다.
- 정상 replay fixture는 `ReplayTransport::matching_requests()`로 새 요청 ID에 대응시킨다. 잘못된 protocol ID를 검사하는 fixture는 원본 ID를 유지한다.
- 실제 HTTP 테스트는 운영 서비스에 연결하지 않고 임시 loopback 포트를 사용한다. 대기 취소 테스트는 event를 확인한 뒤 취소하며, 임의 sleep으로 성공을 추정하지 않는다.

SDK의 feature 경계와 breaking migration은 [마이그레이션 문서](../docs/migration-0.4.md)를 참고한다.
