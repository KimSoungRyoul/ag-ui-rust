---
title: crate와 feature 선택
description: 만들 애플리케이션에 필요한 crate와 feature를 선택합니다.
---

`ag-ui`는 AG-UI protocol type과 server/client API를 제공합니다.
필요한 역할에 맞춰 feature를 켭니다. `ag-ui-a2ui`는 별도 A2UI protocol을 다룹니다.

## 만들 대상에 맞춰 선택하기

| 만들 대상 | 의존성과 feature | 주요 컴포넌트 |
| --- | --- | --- |
| HTTP agent server | `ag-ui`, `features = ["axum"]` | `Agent`, `RunContext`, `RouterExt` |
| HTTP agent client | `ag-ui`, `features = ["http"]` | `HttpAgent`, `Thread`, `Update` |
| HTTP 외의 server | `ag-ui`, `features = ["server"]` | `run`, `Runner`, event stream |
| 직접 transport를 구현하는 client | `ag-ui`, `features = ["client"]` | `Thread`, `Transport` |
| 다른 agent를 호출하는 server | `ag-ui`, `features = ["axum", "http"]` | server와 client 컴포넌트 모두 |
| A2UI surface 작성·검증 | `ag-ui-a2ui` | [A2UI 가이드](/ag-ui-rust/ko/a2ui/)에서 feature 선택 |

`axum`은 `server`와 `sse`를, `http`는 `client`와 `sse`를 포함합니다.
`Message`, `Tool`, `Event`, `RunAgentInput` 같은 protocol type은 crate root에 있습니다.

## 의존성 추가하기

이 프로젝트는 `ag-ui`와 `ag-ui-a2ui`를 사용합니다. crates.io의 `ag-ui-core`,
`ag-ui-server`, `ag-ui-client`는 별개 community SDK의 이름입니다.

[서버 구축 시작](/ag-ui-rust/ko/server/)과 [클라이언트 구축 시작](/ag-ui-rust/ko/client/)에
각 역할의 `Cargo.toml`과 실행 코드가 있습니다. 두 프로그램을 따로 만들면 각자의 의존성만 추가합니다.

## 런타임과 플랫폼

`server`와 `client` API는 transport와 executor를 분리합니다. `axum`은 Tokio 기반 HTTP 서버를,
`http`는 reqwest 기반 HTTP client를 연결합니다. `http` 대신 `client`만 사용하면 transport를 직접 제공해야 합니다.

Cargo는 같은 의존성에 요청한 feature를 합칩니다. 한 dependency graph에서 server와 client를
모두 요청하면 두 기능이 함께 빌드됩니다.

전체 목록은 [Feature 선택](/ag-ui-rust/ko/reference/features/), 지원 환경은
[플랫폼과 Rust 버전](/ag-ui-rust/ko/reference/platforms/)에서 확인합니다.
