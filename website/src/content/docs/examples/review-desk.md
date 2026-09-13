---
title: Review Desk
description: An independent consumer tested over HTTP and against the official A2UI core.
---

Review Desk consumes the public `HttpAgent → Thread → RunStream` API. It runs with a
deterministic async generator and no model key. The server and client communicate over
real HTTP; the browser displays SDK-assembled conversation state and subagent provenance.

```sh
cargo run -p review-desk -- serve
# http://127.0.0.1:8091
cargo run -p review-desk -- demo
cargo test -p review-desk
```

Run local review checks two-thread isolation, rejected partial approval, unique restored
IDs, invalid A2UI output correction, create/edit, and cancellation followed by another run.
Child success and failure include invocation IDs, names and returned results.

The optional official renderer-core check uses npm; ordinary Cargo execution does not.

```sh
npm ci --ignore-scripts --prefix examples/review-desk/interop
npm test --prefix examples/review-desk/interop
```

`@a2ui/web_core@0.11.0` processes unmodified Rust-generated messages. Each step compares
models including null, undefined and array length. This covers the official core's state
and component validation, not complete Lit layout rendering.

See [source and maintenance rules](https://github.com/KimSoungRyoul/ag-ui-rust/tree/main/examples).
