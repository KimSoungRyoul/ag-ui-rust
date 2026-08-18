# Translating the guide

English is the source. Every page under `src/content/docs/` is written in English
first, and a translation is a translation of a specific version of it — not a
second, independent document that happens to cover the same ground.

That distinction is the whole reason this file exists. Two language editions that
are each edited freely stop agreeing within a few months, and a reader has no way
to tell which one is lying. So:

**If the English is wrong, fix the English.** A translator who notices a mistake
should report it and let it be corrected upstream, then translate the corrected
text. Silently fixing it in one language only is how the two editions start
disagreeing.

## Where translations live

`src/content/docs/ko/<same path as the English page>`. English stays at the root
of that directory and keeps its URLs; Korean is served under `/ko/`. Starlight
falls back to the English page when a translation is missing, so a partial
translation is a valid state and never a broken site.

## What is translated, and what is not

Translate the prose, the page `title` and `description`, headings, table cells
that are prose, and code comments that explain something.

Leave these exactly as they are in English:

| Kind | Examples |
| --- | --- |
| Identifiers and paths | `RunContext::assistant_message`, `ag_ui_server`, `crates/ag-ui-core/src/event/` |
| Crate and feature names | `ag-ui-axum`, `verify`, `http`, `toolkit` |
| Wire values | `TEXT_MESSAGE_START`, `a2ui_operations`, `text/event-stream` |
| Anything typed into a terminal | `cargo test --doc --workspace --all-features` |
| Manifest keys and values | `rust-version = "1.85"`, `default-features = false` |
| Compiler output quoted verbatim | `error[E0499]`, `the trait bound 'str: Transport' is not satisfied` |

The rule behind the table: **if a reader would search for the string, or type it,
or see it in their own terminal, it stays in English.** A translated error message
cannot be pasted into a search engine, which is the one thing someone reading an
error message is about to do.

## Code blocks

The Rust in a Korean page is compiled, exactly like the Rust in an English page —
`e2e/src/website.rs` includes both, so `cargo test --doc` is the gate. A
translated snippet that no longer compiles is a red build.

Keep the code itself byte-identical to the English page. Only comments change.
That is not pedantry: identical code makes it a mechanical diff to check whether
a translation has fallen behind an edit to the original, and nothing else does.

## Links

Internal links carry the site's base explicitly — `/ag-ui-rust/server/tools/`.
On a Korean page they carry the locale too: `/ag-ui-rust/ko/server/tools/`.

The link checker fails the build on a link to a page that does not exist *and* on
a link to a heading anchor that does not exist. Translating a heading changes its
anchor, so a link into a translated heading has to be updated with it.

## Register

Korean technical prose, `합니다`체 — the same plain, direct voice the English
uses. Not `해요`체, and not the stiff translationese that comes from rendering
every English clause boundary as a Korean one.

Established loanwords stay loanwords. `트레이트`, `스트림`, `핸들`, `이벤트`,
`렌더러` read better to a Rust programmer than a coined native equivalent, and
the audience for this document is people who already read Rust.

## Terminology

Settled, so that five translators do not each pick a different word:

| English | Korean |
| --- | --- |
| agent | 에이전트 |
| run | 실행 |
| event | 이벤트 |
| event stream | 이벤트 스트림 |
| emit | 방출 |
| handle (RAII) | 핸들 |
| typestate | 타입스테이트 |
| borrow checker | 대여 검사기 |
| ordering (of events) | 순서 |
| verifier / verification | 검증기 / 검증 |
| drift check | 드리프트 검사 |
| transport | 트랜스포트 |
| session | 세션 |
| update | 업데이트 |
| interrupt | 인터럽트 |
| human in the loop | 사람 개입 |
| shared state | 공유 상태 |
| state delta | 상태 델타 |
| tool call | 도구 호출 |
| surface (A2UI) | 서피스 |
| feature flag | 기능 플래그 |
| doctest | doctest |
| workspace | 워크스페이스 |
| crate | 크레이트 |
