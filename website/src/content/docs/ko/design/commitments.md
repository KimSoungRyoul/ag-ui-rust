---
title: 설계 원칙과 책임 범위
description: Protocol SDK의 책임, 보장과 한계를 설명합니다.
---

이 SDK는 agent와 UI 사이의 protocol 연결을 담당합니다.
`Agent::run`에 기존 실행 코드를 연결하고, `RunContext`로 출력을 전달합니다.
`Thread`는 수신한 event를 대화·상태로 반영합니다.

## 애플리케이션과의 경계

모델 loop, 실행 가능한 tool 등록, graph routing, 하위 agent 실행과 영속 저장은 framework나 애플리케이션의 책임입니다.
SDK의 `Tool`은 설명 데이터이며, `subagent_events()`는 출력의 출처를 지정하는 scope입니다.
A2UI의 선택적 authoring helper는 별도 `ag-ui-a2ui` crate에 있습니다.

## Handle과 protocol 검증

메시지·도구 handle은 start/end 생성을 돕습니다. 하나의 context에서 message/tool handle을 겹쳐 빌릴 수 없습니다.
이는 편의를 위한 Rust API 제약입니다. Protocol은 서로 다른 ID의 메시지·호출 interleaving을 허용하며,
`ctx.emit`으로 그 순서를 직접 표현할 수 있습니다.

메시지·도구·step handle은 drop에서 닫힘을 시도합니다. 실패를 확인하려면 `end()`를 호출합니다.
Subagent handle은 drop으로 성공을 추정하지 않습니다. `finish`, `fail`, `suspend`로 종료를 명시해야 합니다.

서버의 선택적 verifier와 client의 verifier가 ID·순서 규칙을 검사합니다.
이 검증은 업무 권한이나 tool 인자 JSON Schema 검증을 대신하지 않습니다.
[검증 가이드](/ag-ui-rust/ko/design/verification/)에 한계가 설명되어 있습니다.

## 공개 타입과 호환성

`Event`와 `EventType`은 exhaustive합니다. SDK를 올려 새 variant가 추가되면 exhaustive match를 수정해야 합니다.
이러한 변경에는 호환성을 깨는 version 증가가 필요합니다. `0.x`에서는 다음 minor 버전이 이에 해당합니다.
`Update`와 주요 error enum은 non-exhaustive이므로 알 수 없는 variant를 처리하는 경로를 둡니다.

ID는 문자열 기반 newtype입니다. Protocol은 UUID 형식을 요구하지 않습니다.
서버의 기본 출력 ID는 run ID와 counter에서 생성되며, client의 자동 ID 생성은 별도 구현입니다.

## 실행과 transport

`server`와 `client`는 HTTP 없이 사용할 수 있습니다. `axum`과 `http`를 활성화하면 Tokio 기반 transport를 사용합니다.
[Feature 선택](/ag-ui-rust/ko/reference/features/)과 [플랫폼](/ag-ui-rust/ko/reference/platforms/)에서 실제 의존성을 확인합니다.

Emit은 동기이며 event를 unbounded queue에 넣습니다. 느린 소비자에 대한 backpressure를 제공하지 않으므로
애플리케이션이 출력 크기와 생성 속도를 관리해야 합니다. 취소는 외부 작업의 rollback이나 원격 중단 확인을 뜻하지 않습니다.

## 명세 추적과 검증 범위

Offline drift check는 vendored TypeScript schema snapshot과 Rust event 이름·필드를 비교합니다.
현재 upstream 전체와 같다는 보장은 아닙니다. 별도의 upstream freshness 검사가 snapshot의 갱신 필요성을 확인합니다.

Protocol type, server/client runtime, A2UI는 [테스트](/ag-ui-rust/ko/design/testing/)로 검증합니다.
Protobuf event encoding은 지원하지 않습니다. A2UI의 `render_a2ui` envelope는 이 SDK의 통합 방식이며,
상대 renderer가 같은 방식을 이해해야 합니다.
