# Official A2UI web core interoperability

이 선택적 검증은 Rust SDK가 실제로 만든 A2UI 메시지를 공식 npm 패키지 `@a2ui/web_core@0.15.0`의 `MessageProcessor`에 그대로 입력한다. npm 패키지 버전은 `0.15.0`이고, 같은 단계를 A2UI wire profile `v0.9.1`과 `1.0`으로 각각 한 번씩 검사한다.

저장소 루트에서 Node.js 22.12 이상과 Cargo를 사용한다.

```sh
npm ci --ignore-scripts --prefix examples/review-desk/interop
npm test --prefix examples/review-desk/interop
```

`npm test`가 `cargo run --locked --quiet -p review-desk -- export-a2ui`와 `… export-a2ui 1.0`을 실행하므로 별도의 서버나 수동 fixture 준비가 필요 없다. 버전 인자가 없으면 지금까지와 같은 `v0.9.1` fixture를 내보낸다. 원본을 확인하려면 다음 명령을 쓴다.

```sh
cargo run --locked --quiet -p review-desk -- export-a2ui > /tmp/review-a2ui.json
cargo run --locked --quiet -p review-desk -- export-a2ui 1.0 > /tmp/review-a2ui-v1.json
```

`src/interop.rs`는 `A2uiAuthor`로 create/edit를 검증하고, `RunContext::send_a2ui()`가 실제로 발행한 tool result에서 operations를 추출한다. 삭제·배열·컴포넌트 교체는 공개 `AgentMessage` helper로 만들며 전체 시퀀스는 Rust의 `SurfaceStore`에도 적용한다. JavaScript는 ID·version·payload를 수정하지 않는다. catalog ID는 SDK의 `A2uiVersion::basic_catalog_id()`와 해당 버전의 공식 `basicCatalog.id`가 정확히 같은지 확인한다.

각 단계 직후의 SDK DataModel snapshot과 공식 core의 모델도 비교한다. JavaScript의 sparse slot·undefined를 손실 없이 구분하므로 이후 surface 재생성으로 앞선 데이터 오류가 가려지지 않는다. snapshot은 비교용 metadata이며 renderer 메시지에 포함하지 않는다.

검증 항목:

- create 이후 Column root, Text 바인딩과 데이터 확인.
- edit 이후 제목 갱신과 memo null 보존.
- 명시적 null 저장과 객체 키 존재 확인.
- 없는 `/list/0` 갱신으로 객체의 숫자 키가 아닌 배열이 생성되는지 확인.
- null 부모 아래의 object/array 갱신, null root에서 object 복원 확인.
- 기존·새 배열의 건너뛴 인덱스 갱신에서 Undefined와 길이 보존 확인. 배열 끝을 넘는 슬롯 삭제는 아무것도 바꾸지 않는다(배열을 그 인덱스까지 늘리지 않는다).
- `/matrix/0/1` 같은 중첩 숫자 경로에서 각 깊이에 배열이 생성되는지 확인.
- 삭제 메시지(`v0.9.1`은 `value` 생략, `1.0`은 `"value": null`)에서 객체 키 제거 확인.
- 배열 슬롯 삭제 후 길이와 다른 슬롯 유지, 삭제 슬롯은 `undefined` 확인.
- 같은 component ID 갱신 시 교체되고 개수는 증가하지 않는지 확인.
- surface 삭제 후 조회 불가, 같은 ID 재생성 후 이전 component/data가 남지 않는지 확인.

## 이 renderer와 v0.9 명세가 갈리는 한 곳

v0.9에서 삭제는 `value`를 생략해서 하고 `null`은 값이다(`specification/v0_9/docs/a2ui_protocol.md`, "Server to client updates"). SDK는 v0.9 surface에서 `null`을 저장한다. `@a2ui/web_core@0.15.0`은 surface의 버전과 관계없이 `updateDataModel`을 `DataModel.set(path, value)`로 적용하고(`src/processing/message-processor.js`, `processUpdateDataModelOp`), `set`은 `preserveNull`을 받지 않으면 `null`을 삭제로 다룬다(`src/state/data-model.js`). 그래서 v0.9 surface의 어떤 경로에 직접 쓴 `null`은 이 renderer에서는 사라지고 SDK에는 남는다. 값 안에 들어 있는 `null`(`[1, null, 3]`)은 양쪽 모두 저장한다.

`verify.mjs`는 이 차이 하나만을 이름 붙여 다룬다. 메시지가 `null`을 쓴 경로를 추적하고, SDK snapshot을 이 renderer가 들고 있을 모습으로 바꿔 비교하며, `explicit-null` 단계에서 양쪽이 실제로 그렇게 다르다는 것을 단언한다. `1.0`에서는 `null`이 양쪽 모두에서 삭제이므로 맞출 것이 없다. surface activity 하나만으로 그린 결과는 두 버전 모두 SDK와 정확히 같다.

기본 Cargo 앱·테스트는 npm을 필요로 하지 않는다. 이 검증은 공식 web core의 컴포넌트 schema 검사와 상태 처리까지 다루며, 브라우저의 전체 Lit widget 레이아웃·입력 동작을 검증했다고 주장하지 않는다. Review Desk 브라우저 UI는 별도 HTTP 테스트와 수동 브라우저 검사로 확인한다.
