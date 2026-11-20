---
title: 개요
description: A2UI 프로토콜과 agent 계층, surface 검증 및 전송 선택을 설명합니다.
---

[A2UI](https://a2ui.org/)는 component와 data model로 UI를 표현합니다.
`ag-ui-a2ui`는 이 메시지를 만들고 검증하고 재생합니다. 앱이 모델, 전송, renderer를 제공합니다.
프로토콜은 `v0.9`, `v0.9.1`, `1.0`을 지원하며 기본값은 `v0.9`입니다.

AG-UI adapter는 surface operation을 전달합니다. A2UI 1.0 함수 호출 메시지는
A2A처럼 A2UI 메시지를 그대로 전달하는 전송 경로를 사용합니다.

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
| 모델이 UI 구성까지 작성 | Card·Text 등을 포함한 A2UI JSON | `A2uiAuthor`로 생성 결과 검증·교정 |
| 개발자가 화면 틀을 작성 | 날씨 등 일반 데이터, 또는 템플릿 도구의 입력 인자 | 코드로 컴포넌트와 데이터를 조립하고 검증 |

`generate()`는 SDK의 편의 메서드이며 A2UI 표준 메시지 이름이 아닙니다. 일반 데이터를 받으면
자동으로 화면을 고르는 변환기도 아닙니다. 모델 호출 없이 코드로 작성한 화면도 A2UI입니다.
[템플릿 도구 예제](/ag-ui-rust/ko/v1.0.0/a2ui/authoring/#템플릿을-도구로-제공하기)에서 두 번째 방식을 볼 수 있습니다.

## 필요한 작업으로 시작하기

| 작업 | API |
| --- | --- |
| 정해진 화면 직접 작성 | `AgentMessage`, `Component`, `SurfaceSpec` |
| 컴포넌트 연결과 데이터 바인딩 검사 | `Validator` |
| 전체 스키마와 여러 surface의 메시지 검증 | `schema_validation::SurfaceValidator` |
| 비동기 모델에 화면 생성·수정 요청 | `A2uiAuthor` |
| 검증된 화면을 AG-UI로 전송 | `adapters::agui::SurfacePainter` |
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

## Feature 선택

| Feature | 기본값 | 기능 |
| --- | --- | --- |
| `agent` | 켜짐 | Catalog 합의, prompt, 답변 파싱과 streaming, renderer 상태, 재시도 판단 |
| `schema-validation` | 꺼짐 | 로컬 Draft 2020-12 JSON Schema 검증 |
| `author` | 꺼짐 | 정해진 surface를 생성·수정하는 API. `agent`와 `schema-validation`을 켭니다. |
| `ag-ui` | 꺼짐 | AG-UI envelope, history, catalog context, 도구 정의. `agent`를 켭니다. |
| `ag-ui-server` | 꺼짐 | `RunContext`에서 surface 전송. `ag-ui`와 `ag-ui/server`를 켭니다. |
| `a2a` | 꺼짐 | A2A extension과 data part에 쓰는 JSON helper. `agent`를 켭니다. |

`default-features = false`로 프로토콜 모듈만 사용할 수 있습니다. AG-UI 서버에서
모델로 surface를 작성해 보내려면 `author`와 `ag-ui-server`를 모두 켭니다.
[버전과 catalog 선택](/ag-ui-rust/ko/v1.0.0/a2ui/versions/),
[agent 계층 사용](/ag-ui-rust/ko/v1.0.0/a2ui/agent/), [함수 호출](/ag-ui-rust/ko/v1.0.0/a2ui/functions/)에서
각 연결 방법을 확인할 수 있습니다.

## Null 저장과 삭제

다음 예제는 기본값인 `v0.9`를 사용합니다. `1.0`에서는 `null`이 삭제를 뜻합니다.

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
[검증과 복원](/ag-ui-rust/ko/v1.0.0/a2ui/validation/)을 참고하세요.
