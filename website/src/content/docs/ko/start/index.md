---
title: 시작하기
description: 서버와 클라이언트 중 만들 대상을 고르고 필요한 구현 가이드로 이동합니다.
---

AG-UI로 agent의 응답을 UI에 연결합니다. 이 SDK는 agent application(server)에서
메시지·도구 호출·상태를 event stream으로 보내고, agent client에서 이를 대화와 화면 상태로 반영합니다.

이 문서는 독립 Rust SDK인 `ag-ui-rust`의 구현 가이드입니다.
protocol의 개념과 명세는 [AG-UI 공식 문서](https://docs.ag-ui.com/introduction)에서 확인합니다.

## 무엇을 만드나요?

| 만들 대상 | 시작할 가이드 | 완성되는 것 |
| --- | --- | --- |
| Agent application (server) | [Agent에 AG-UI 연결하기](/ag-ui-rust/ko/server/) | `Agent` 구현과 HTTP streaming endpoint |
| Agent client | [Rust에서 Agent 호출하기](/ag-ui-rust/ko/client/) | 서버 연결, 대화 유지, 응답 표시 |
| 양쪽 모두 | 서버를 실행한 뒤 client 연결 | Rust 서버와 client 사이의 한 번의 대화 |

## 요청 하나가 화면에 도착하기까지

1. **Client**가 사용자 입력과 대화 기록을 보냅니다.
2. **Server**의 `Agent::run`이 애플리케이션 로직이나 모델을 호출합니다.
3. **RunContext**가 텍스트·도구·상태 변경을 AG-UI event로 내보냅니다.
4. **Client의 Thread**가 event를 대화와 상태에 반영하고, `Update`로 화면에 알립니다.

모델 호출, 도구 실행, 인증, 데이터 저장과 작업 재개 정책은 애플리케이션이나 agent framework에서 구현합니다.
SDK는 양쪽을 연결하는 protocol type, event 생성, transport와 client 상태 반영을 제공합니다.

## SDK가 맡는 범위

| 애플리케이션·framework | AG-UI SDK |
| --- | --- |
| 모델 loop, tool 함수 등록·실행 | tool 이름·인자·결과를 protocol 데이터로 전달 |
| graph 분기, 병렬 작업, subagent 실행 | 실행 단계와 하위 agent의 출력·상태 전달 |
| 승인 정책, 저장, 작업 재개 | interrupt와 응답 전달, 로컬 대화 snapshot |

`Tool`은 이름·설명·인자 JSON Schema를 담는 데이터입니다. 실행 함수를 등록하지 않습니다.
[도구 호출·결과 전달](/ag-ui-rust/ko/server/tools/)에서 이미 구현된 tool을 연결하는 흐름을 확인합니다.

## 준비하기

Rust **1.85 이상**이 필요합니다. 서버에는 `ag-ui`의 `axum` feature,
HTTP client에는 `http` feature를 사용합니다. 각 구축 가이드에 필요한 `Cargo.toml`과 실행 코드가 있습니다.

[crate와 feature 선택](/ag-ui-rust/ko/start/crates/)에서 의존성을 고르고,
[AG-UI 동작 방식](/ag-ui-rust/ko/start/protocol/)에서 요청·event·run의 관계를 확인합니다.

## 동작하는 예제로 확인하기

- [task-board](/ag-ui-rust/ko/examples/task-board/): 상태 변경, 도구 호출과 승인 요청을 보내는 서버.
- [board-watch](/ag-ui-rust/ko/examples/board-watch/): 그 서버에 연결하는 Rust CLI client.
- [review-desk](/ag-ui-rust/ko/examples/review-desk/): 브라우저 UI까지 연결한 예제.
