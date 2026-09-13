---
title: 개요
description: A2UI v0.9/v0.9.1의 화면 표현, 비동기 생성, 스키마 검증과 AG-UI 전송 연동.
---

[A2UI](https://a2ui.org/specification/v0.9.1-a2ui/)는 화면의 컴포넌트와 데이터 모델을 JSON으로 표현합니다.
`ag-ui-a2ui`는 이 설명을 만들고, 검증하고, 이력에서 복원합니다. 생성 모델과 화면을 그리는 renderer는 애플리케이션이 제공합니다.

A2UI는 AG-UI와 별도의 프로토콜입니다. 선택적인 AG-UI 연동은 `render_a2ui` 도구 결과에
`a2ui_operations` envelope를 담아 전달합니다. 다른 전송 방식은 프로토콜 타입을 직접 사용할 수 있습니다.
MIME 타입은 `application/a2ui+json`입니다.

## A2UI를 날씨 카드로 이해하기

사용자가 “서울 날씨 알려줘”라고 하면, 일반적인 답변은 “맑음 · 21도”라는 글입니다.
A2UI는 앱에 “카드를 하나 만들고, 제목은 서울 날씨, 내용은 맑음 · 21도로 보여줘”라는
**화면 설명서**를 JSON으로 보냅니다. 앱의 renderer가 설명을 읽고 실제 카드와 버튼을 그립니다.
A2UI 메시지는 실행할 HTML·JavaScript가 아니라 앱이 지원하는 컴포넌트의 선언입니다.

| 담당 | 역할 |
| --- | --- |
| 모델 또는 애플리케이션 코드 | 보여줄 내용과 화면 구성 결정 |
| A2UI | 그 구성을 전달하는 공통 JSON 형식 |
| Renderer | JSON을 읽어 실제 화면 표시 |

화면을 정하는 방법은 두 가지입니다.

| 방식 | 모델이 반환하는 것 | SDK 사용 |
| --- | --- | --- |
| 모델이 UI 구성까지 작성 | Card·Text 등을 포함한 A2UI JSON | `A2uiAuthor.generate()`로 생성 결과 검증·교정 |
| 개발자가 화면 틀을 작성 | 날씨 등 일반 데이터, 또는 템플릿 도구의 입력 인자 | 코드로 컴포넌트와 데이터를 조립하고 검증 |

`generate()`는 SDK의 편의 메서드이며 A2UI 표준 메시지 이름이 아닙니다. 일반 데이터를 받으면
자동으로 화면을 고르는 변환기도 아닙니다. 모델 호출 없이 코드로 작성한 화면도 A2UI입니다.
[템플릿 도구 예제](/ag-ui-rust/ko/a2ui/authoring/#템플릿을-도구로-제공하기)에서 두 번째 방식을 볼 수 있습니다.

## 필요한 작업으로 시작하기

| 작업 | API |
| --- | --- |
| 정해진 화면 직접 작성 | `AgentMessage`, `Component`, `SurfaceSpec` |
| 컴포넌트 연결과 데이터 바인딩 검사 | `Validator` |
| 전체 스키마와 여러 surface의 메시지 검증 | `schema_validation::SurfaceValidator` |
| 비동기 모델에 화면 생성·수정 요청 | `A2uiAuthor` |
| 검증된 화면을 AG-UI로 전송 | `A2uiRunContextExt::send_a2ui` |
| 데이터 모델을 손실 없이 보관·복원 | `DataModel`, `SurfaceStore` |

```rust
use ag_ui_a2ui::{Catalog, Component, Validator};
use serde_json::json;

let catalog = Catalog::basic();
let components = [
    Component::new("root", "Column").with("children", json!(["title"])),
    Component::new("title", "Text").with("text", json!({"path":"/title"})),
];
let report = Validator::new(&catalog)
    .validate_surface(&components, Some(&json!({"title":"Your cart"})));
assert!(report.is_valid());
```

컴포넌트는 평평한 목록으로 전달하고 부모는 자식의 ID를 참조합니다. 각 surface는 자기 `root`와
컴포넌트 ID를 가집니다. 후속 메시지에서 같은 ID를 정의하면 교체하지만, 한 메시지 내부의 중복 ID는 오류입니다.

## 버전과 feature

`v0.9`와 `v0.9.1`을 수용합니다. 저수준 생성자의 기본값은 `v0.9`이며,
`.with_version(A2uiVersion::V0_9_1)`로 송신 버전을 지정합니다.
`A2uiAuthor::basic(version)`은 두 버전을 모두 허용하는 고정된 공식 v0.9.1 스키마를 사용합니다.

서버 메시지는 `createSurface`, `updateComponents`, `updateDataModel`, `deleteSurface` 네 종류입니다.
renderer 응답은 `action` 또는 `error`입니다. 후보 RPC 타입은 남아 있지만 v0.9 계열 메시지로 직렬화·역직렬화할 수 없습니다.
타입이 존재한다는 이유로 v1.0 지원을 의미하지 않습니다.

| Feature | 기본 | 추가 기능 |
| --- | --- | --- |
| `toolkit` | 켜짐 | 수동 조립, prompt/parser, history, 동기·비동기 recovery |
| `ag-ui` | 켜짐 | AG-UI 이력과 도구 정의 연동 |
| `schema-validation` | 꺼짐 | Draft 2020-12, 공식 스키마, 로컬 참조 registry |
| `author` | 꺼짐 | 공통 생성 설정과 불변 검증 결과 |
| `ag-ui-server` | 꺼짐 | author와 AG-UI 서버 이벤트 전송 |

새 feature는 HTTP, axum, Tokio를 자동으로 추가하지 않습니다. 기본 feature를 끄면 프로토콜·모델 사용자는
AG-UI나 스키마 엔진에 의존하지 않습니다. Rust 1.85와 wasm 빌드를 검사하며, 스키마 엔진의 브라우저 난수 지원도
해당 feature·target에만 적용합니다.

## Catalog 합의

`A2uiAuthor::basic`은 공식 catalog가 선언한
`https://a2ui.org/specification/v0_9/catalogs/basic/catalog.json`을 사용합니다.
기존 `BASIC_CATALOG_ID`는 수동 연동을 위해 유지합니다. 두 ID가 같은 renderer 구현을 뜻한다고
자동으로 가정하지 않으며, 실제로 같을 때 애플리케이션이 alias를 등록합니다.

Capabilities는 v0.9.1 메시지를 써도 `v0.9` namespace를 사용합니다. Inline catalog는 광고한 자기 ID로
문서 전체를 선택합니다. 일치하는 ID가 없거나 정의가 충돌하면 오류입니다.
[화면 작성](/ag-ui-rust/ko/a2ui/authoring/)에서 사용 흐름을 설명합니다.

## Null 저장과 삭제

```rust
use ag_ui_a2ui::AgentMessage;
use serde_json::Value;

let set_null = AgentMessage::update_data_model("cart", "/memo", Value::Null);
let remove = AgentMessage::remove_data_model_value("cart", "/memo");
assert!(serde_json::to_value(&set_null).unwrap()["updateDataModel"].get("value").is_some());
assert!(serde_json::to_value(&remove).unwrap()["updateDataModel"].get("value").is_none());
```

삭제는 객체 키를 제거하고, 배열 길이를 유지한 채 해당 슬롯을 Undefined로 만들며,
root 삭제는 Undefined root를 남깁니다. `DataModel`은 이 차이를 로컬 snapshot으로 보존합니다.
`to_json()`은 Undefined를 null로 바꾸지 않고 오류를 반환합니다.
[검증과 복원](/ag-ui-rust/ko/a2ui/validation/)을 참고하세요.
