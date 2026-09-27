---
title: 문서 버전 선택
description: 사용하는 Rust SDK 버전에 맞는 가이드를 선택합니다.
---

# SDK 버전 선택

두 버전은 Rust API와 AG-UI wire 동작이 다릅니다. 애플리케이션의 `Cargo.lock`에
기록된 버전에 맞는 문서를 사용하세요.

| 버전 | 상태 | 가이드 | 주요 차이 |
| --- | --- | --- | --- |
| `0.4.5` | 안정 릴리스 | [0.4.5 가이드](/ag-ui-rust/ko/v0.4.5/start/) | 기존 대화·A2UI API |
| `0.5.0-alpha.1` | 프리릴리스 | [0.5.0-alpha.1 가이드](/ag-ui-rust/ko/v0.5.0-alpha.1/start/) | AG-UI 1.0 이벤트, 구조화된 도구 결과와 새 run 결과 |

두 가이드 모두 `ag-ui`와 별도 crate인 `ag-ui-a2ui`를 설명합니다. 버전별 문서는
해당 소스 버전의 동작을 기준으로 합니다. 레지스트리 설치 명령을 사용하기 전에는
[crates.io](https://crates.io/crates/ag-ui)에서 게시 여부를 확인하세요.

[0.5 마이그레이션 노트](https://github.com/KimSoungRyoul/ag-ui-rust/blob/main/docs/migration-0.5.md)는
소스·wire 변경을 요약합니다. 0.4.5 페이지는
[`v0.4.5` 태그](https://github.com/KimSoungRyoul/ag-ui-rust/tree/v0.4.5/website/src/content/docs)의
내용을 보존했습니다.
