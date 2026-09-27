---
title: 검증과 복원
description: 로컬 전체 스키마 검증, surface별 원자적 메시지 적용, 전방 참조, 데이터 모델의 손실 없는 snapshot.
---

`Validator`는 컴포넌트 관계, 바인딩, 기본 속성·envelope 제약을 검사합니다.
`SchemaValidator`와 `SurfaceValidator`는 고정된 공식 스키마의 Draft 2020-12 전체 검증을 추가합니다.
`A2uiAuthor`는 항상 전체 검증을 사용하며 feature를 끈다고 약한 검사로 대체하지 않습니다.

## SDK 안의 JSON 파일은 무엇인가

`schemas/v0_9_1/`은 사용자 데이터나 화면 템플릿을 저장하는 곳이 아니라 공식 규칙집입니다.
`server_to_client.json`은 메시지 형식, `common_types.json`은 바인딩·함수 호출 같은 공통 형식,
`catalog.json`은 Text·Button 등 컴포넌트 속성을 정의합니다.

SDK는 특정 upstream 버전을 원문 그대로 포함합니다. `include_str!`로 컴파일할 때 포함하므로
실행 중 인터넷에서 내려받지 않습니다. `A2uiAuthor`는 같은 규칙을 모델 프롬프트와 결과 검증에
사용합니다. 수동 템플릿도 이 규칙으로 검증할 수 있습니다.

| 단계 | 하는 일 | 예시 |
| --- | --- | --- |
| JSON 파싱 | JSON 문법 확인 | 쉼표·괄호가 잘못된 문서 거부 |
| JSON Schema 검증 | 규칙 파일에 맞는 필드·타입·구조 확인 | catalogId 누락, 숫자 surfaceId 거부 |
| 의미·상태 검증 | 컴포넌트 연결과 순차 갱신 확인 | 없는 자식 참조, 중복 create 거부 |

즉 JSON 스키마 파일은 검사 로직 자체가 아닙니다. `jsonschema` 라이브러리가 읽는 규칙이며,
화면 수명과 관계는 SDK의 Rust 코드가 검사합니다. `generate()`에서는 모델이 UI JSON을 만들고
SDK가 이를 검사합니다. 일반 날씨 데이터를 자동으로 화면으로 바꾸는 규칙은 아닙니다.

## 메시지 스트림 검증

```rust
use ag_ui_a2ui::schema_validation::SurfaceValidator;
use ag_ui_a2ui::toolkit::schema::SchemaBundle;
use serde_json::json;
use std::collections::BTreeMap;

let bundle = SchemaBundle::basic().unwrap();
let mut stream = SurfaceValidator::new();
stream.register(&bundle, &BTreeMap::new()).unwrap();
stream.push(&json!({"version":"v0.9.1","createSurface":{
    "surfaceId":"cart","catalogId":bundle.catalog_id().unwrap()
}})).unwrap();
assert!(stream.finish().is_err()); // The root has not arrived yet.
stream.push(&json!({"version":"v0.9.1","updateComponents":{
    "surfaceId":"cart",
    "components":[{"id":"root","component":"Text","text":"Your cart"}]
}})).unwrap();
stream.finish().unwrap();

// Retained state is explicit when validating a later partial stream.
let mut update = SurfaceValidator::from_state(stream.state().clone());
update.register(&bundle, &BTreeMap::new()).unwrap();
update.push(&json!({"version":"v0.9.1","updateDataModel":{
    "surfaceId":"cart","path":"/memo","value":null
}})).unwrap();
update.finish().unwrap();
```

각 `push`는 serde가 기본값을 넣거나 필드를 지우기 전에 raw JSON을 검사합니다.
성공한 임시 상태만 반영하므로, 잘못된 pointer·메시지 내부 중복 ID·스키마 오류가 나도 이미 수용한 상태는 유지됩니다.
renderer에 앞서 보낸 메시지까지 되돌렸다는 뜻은 아닙니다.

스트림이 열려 있을 때 없는 root·아직 도착하지 않은 자식은 대기 상태입니다.
`finish()`에서 완전한 그래프와 바인딩을 검사합니다. 후속 메시지의 같은 ID는 교체이고,
서로 다른 surface는 각각 자기 `root`를 가질 수 있습니다. Update·delete 전에 active surface가 필요합니다.
중복 create는 거부하며 delete 후 create로 ID를 다시 사용할 수 있습니다.

여러 catalog를 쓰면 각각 전체 문서를 등록합니다. 상태는 renderer·대화 문맥 하나에 속하므로 사용자 사이에서 공유하지 않습니다.
미등록 ID·참조는 로컬 오류이며, catalog URL이나 `$ref` 때문에 네트워크·파일시스템 조회를 하지 않습니다.

## 의미 검증 진단

```rust
use ag_ui_a2ui::{Catalog, Component, ErrorCode, Validator};
use serde_json::json;

let report = Validator::new(&Catalog::basic()).validate(&[
    Component::new("root", "Card").with("child", json!("missing")),
]);
assert!(report.errors.iter().any(|error| error.code == ErrorCode::UnresolvedChild));
```

`ValidationReport`는 기계가 읽을 코드, 위치, 교정 메시지를 보존합니다.
Root에서 도달할 수 없는 컴포넌트는 별도 경고 목록에 둡니다.
`Validator::incremental().validate(components)`는 컴포넌트 조각을 검사하며 renderer 수명이 유효하다고 보장하지 않습니다.
전체 메시지는 `validate_messages`, 부분 메시지는 명시적인 prior를 전달하는 `validate_updates`로 검사합니다.

전체 스키마 검사는 enum, 중첩 속성, 추가 필드, 함수 인자를 검사합니다.
그래프·바인딩 검사는 JSON Schema가 표현하지 못하는 관계를 검사하고,
전체 surface validator는 custom catalog의 composition 제약도 검사합니다.
저수준 의미 검증기에서는 root·깊이·바인딩 정책을 조정할 수 있습니다.

## Undefined를 보존하기

```rust
use ag_ui_a2ui::{DataModel, DataModelUpdate, ModelValue};
use serde_json::{json, Value};

let mut model = DataModel::from(json!({"items":[null,2]}));
model.apply("/items/1", &DataModelUpdate::Remove).unwrap();
assert_eq!(model.lookup("/items/0").unwrap(), Some(&ModelValue::Null));
assert_eq!(model.lookup("/items/1").unwrap(), None);
assert!(model.to_json().is_err());

let saved = serde_json::to_vec(&model).unwrap();
let restored: DataModel = serde_json::from_slice(&saved).unwrap();
assert_eq!(restored, model);
model.apply("/", &DataModelUpdate::Set(Value::Null)).unwrap();
assert_eq!(model.to_json().unwrap(), Value::Null);
```

Upsert는 없거나 null인 중간 컨테이너를 생성하며, 숫자 path는 배열을 만들고 빈 슬롯은 Undefined로 둡니다.
기본 갱신 하나는 새로운 배열 슬롯 65,536개까지 생성하며 `DataModel::apply_with_array_growth_limit`으로
예산을 명시할 수 있습니다. 잘못된 path나 할당 한도 오류는 이전 모델을 보존합니다.

Snapshot은 포맷 버전과 Undefined를 보존하는 내부 저장 표현이며 A2UI wire 값이 아닙니다.
지원하지 않는 snapshot 버전은 거부합니다. Undefined root·슬롯이 하나라도 있으면
일반 JSON export는 오류를 반환합니다. 전송에는 원래 update operation을 사용합니다.

`binding::ModelScope`는 null과 missing/undefined를 구분하고 collection scope를 지원합니다.
`Validator::validate_model`은 이 손실 없는 상태에서 바인딩을 검사하며, template의 정의된 item을 확인합니다.
모델 전체를 JSON으로 표현할 수 있는 경우 기존 `Scope`와 `validate_surface`도 사용할 수 있습니다.

## 검증 범위

공식 v0.9.1 스키마를 upstream commit `1c45c809`로 고정했습니다.
`jsonschema 0.29.1`의 기본 리소스 조회 feature를 끄고 외부 조회를 거부하는 retriever를 사용합니다.
Rust 1.85와 wasm 빌드를 검사합니다.

`tests/author.rs`는 실제 스키마 엔진, 로컬 참조, 비동기 교정, target·catalog·version 검사,
여러 catalog의 원자적 메시지 적용을 검사합니다.
`tests/protocol_091.rs`는 null·삭제·Undefined, snapshot, history 복원을 검사합니다.
`examples/`의 독립 애플리케이션은 공개 SDK 사용 흐름을 검증합니다.

이전 toolkit conformance 결과는 **119 통과, 74 제외, 0 실패**입니다.
네 개의 이름이 지정된 사례는 기본 catalog로의 암묵적 fallback이나 다른 catalog의 정의를 같은 ID로 병합하는
이전 정책을 요구합니다. 이 사례를 명시적으로 제외하고 현재의 명시적 선택·충돌 거부 회귀 검사로 대체했습니다.
나머지는 v0.8·renderer·example 파일 검사이며 vendored conformance README에 사유를 기록했습니다.
이전 컴포넌트 조각 사례는 fragment 검증을 사용하며 존재하지 않는 renderer 이력을 만들어 내지 않습니다.
