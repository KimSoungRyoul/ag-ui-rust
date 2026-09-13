---
title: Feature 선택
description: 두 crate의 feature, 기본값과 의존성을 확인합니다.
---

manifest에 선언된 feature와 직접 포함하는 항목입니다. `dep:`는 optional dependency이며,
feature가 포함하는 다른 feature의 의존성도 함께 활성화됩니다.

| Crate | Feature | 기본 | 직접 포함 | 기능 |
| --- | --- | --- | --- | --- |
| `ag-ui` | `sse` | on | — | SSE framing |
| `ag-ui` | `protobuf` | off | — | 미디어 타입만 제공; encoding 미지원 |
| `ag-ui` | `schemars` | off | `dep:schemars` | JSON Schema derive |
| `ag-ui` | `utoipa` | off | `dep:utoipa` | OpenAPI schema derive |
| `ag-ui` | `server` | off | `dep:futures-core`, `dep:futures-channel`, `dep:futures-util`, `dep:json-patch` | Agent adapter와 event 생성 |
| `ag-ui` | `verify` | on | — | 서버 event 순서 검사 |
| `ag-ui` | `client` | off | `dep:futures-core`, `dep:futures-util`, `dep:json-patch`, `dep:getrandom`, `dep:time`, `dep:js-sys` | Thread, Update와 custom transport |
| `ag-ui` | `http` | off | `client`, `sse`, `dep:reqwest` | HttpAgent와 reqwest transport |
| `ag-ui` | `axum` | off | `server`, `sse`, `dep:axum`, `dep:tokio`, `dep:futures-util` | Axum HTTP endpoint |
| `ag-ui-a2ui` | `toolkit` | on | — | 수동 A2UI 작성과 복구 helper |
| `ag-ui-a2ui` | `schema-validation` | off | `toolkit`, `dep:jsonschema`, `dep:schema-getrandom` | 로컬 schema를 이용한 전체 검증 |
| `ag-ui-a2ui` | `author` | off | `toolkit`, `schema-validation` | 검증된 A2UI 작성 |
| `ag-ui-a2ui` | `ag-ui-server` | off | `author`, `ag-ui`, `ag-ui/server` | 검증된 A2UI를 AG-UI로 전달 |
| `ag-ui-a2ui` | `ag-ui` | on | `dep:ag-ui`, `toolkit` | AG-UI type·history 연동 |

## 역할에 맞춰 선택

- 서버: `features = ["axum"]`으로 `server`와 `sse`도 활성화합니다.
- HTTP client: `features = ["http"]`가 필요합니다. `http`는 기본 feature가 아닙니다.
- 직접 transport를 제공하는 client: `features = ["client"]`를 사용합니다.
- 서버의 선택적 ordering 검사를 끄려면 `default-features = false`와 필요한 feature를 함께 지정합니다.

`verify`를 꺼도 종료 event 중복 방지와 subagent 종료 추적은 남습니다.
Client 검증은 별도로 `ThreadBuilder::verify(false)` 또는 `set_verify(false)`로 조절합니다.

## 의존성과 한계

`http`는 reqwest를 통해 Tokio를 사용하며, `axum`도 Tokio에 의존합니다.
`client`만 켜면 ID 생성과 시간 처리를 위한 의존성은 있지만 executor는 추가하지 않습니다.
A2UI의 `ag-ui-server`는 HTTP나 axum을 활성화하지 않습니다.

`protobuf`는 binary event encoder를 제공하지 않습니다. `ProtobufFormatter::encode`는
`UnsupportedTransport`를 반환합니다. 실제 통신에는 SSE를 사용합니다.

Cargo는 같은 의존성의 feature를 합칩니다. 한 곳에서 켠 기능을 다른 곳의
`default-features = false`가 취소할 수는 없습니다.

[crate 선택](/ag-ui-rust/ko/start/crates/), [플랫폼](/ag-ui-rust/ko/reference/platforms/), [검증](/ag-ui-rust/ko/design/verification/)을 함께 참고합니다.
