---
title: Review Desk
description: 실제 HTTP와 공식 A2UI core로 검증하는 독립 소비자 앱입니다.
---

Review Desk는 새 `HttpAgent → Thread → RunStream` API를 사용하는 독립 앱입니다.
모델 API 키 없이 결정적인 비동기 생성기로 실행합니다. 서버와 클라이언트는 실제 HTTP로
통신하고, 브라우저는 SDK가 조립한 대화·상태·subagent 출처와 결과를 표시합니다.

```sh
cargo run -p review-desk -- serve
# http://127.0.0.1:8091
cargo run -p review-desk -- demo
cargo test -p review-desk
```

화면의 Run local review를 실행하면 두 thread의 격리, 일부 승인 응답 거부,
복원 후 ID 유일성, A2UI 생성 오류 교정과 create/edit, run 중단 후 다음 실행을 확인합니다.
subagent의 성공과 실패는 호출 ID·이름·반환 결과와 함께 표시됩니다.

공식 renderer core 검증은 npm을 사용하는 선택적 검사이며 일반 Cargo 실행에는 필요하지 않습니다.

```sh
npm ci --ignore-scripts --prefix examples/review-desk/interop
npm test --prefix examples/review-desk/interop
```

`@a2ui/web_core@0.11.0`이 Rust에서 생성한 메시지를 수정 없이 처리합니다.
각 단계에서 null·undefined·배열 길이를 포함한 모델을 비교합니다. 이 검사는 공식 core의
상태·컴포넌트 검증이며 전체 Lit 화면 배치 검증은 아닙니다.

[소스와 관리 규칙](https://github.com/KimSoungRyoul/ag-ui-rust/tree/main/examples)을 참고하세요.
