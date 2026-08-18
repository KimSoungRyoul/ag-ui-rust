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

## Keep the sentences short

Shorter than the English, usually. The English here is written to be read
slowly; Korean carrying the same clause structure reads as translationese, and
the meaning is what has to survive, not the rhythm.

One idea per sentence. Split a long English sentence into two or three Korean
ones rather than reproducing its subordinate clauses. Cut every word that is
doing no work — Korean lets you drop subjects and connectives that English needs,
so take that.

`합니다`체 throughout. Not `해요`체, not `한다`체.

## Technical terms stay in English

Do not translate them, and do not transliterate them into Hangul either. Write
the English word in Latin script and attach Korean particles to it, which is how
Korean developers write when they are actually working.

    Good:  emit한 event가 순서대로 도착합니다.
           borrow checker가 두 번째 handle을 거부합니다.
           tool call은 client가 실행합니다.

    Bad:   방출한 이벤트가 순서대로 도착합니다.
           대여 검사기가 두 번째 핸들을 거부합니다.
           도구 호출은 클라이언트가 실행합니다.

The reader already reads Rust. A coined Korean equivalent makes them translate it
back before they can use it, and it will not match anything they can search for.

These are the ones a translator is most tempted to convert. Leave every one of
them as it is written here:

    agent · run · event · event stream · emit · handle · typestate
    borrow checker · ordering · verifier · verification · drift check
    transport · session · update · interrupt · human in the loop
    shared state · state delta · tool call · surface · feature flag
    doctest · workspace · crate · trait · stream · renderer · client · server

Ordinary words are still Korean. `상태를 바꿉니다`, `순서가 중요합니다`,
`검증합니다` are fine as plain prose; what must not happen is a *term of art*
being replaced by an invented Korean one. When you cannot tell which you are
looking at, keep the English — it is the cheaper mistake.
