# Vendored AG-UI 1.0 corpus

Copied without modification from [ag-ui-protocol/ag-ui](https://github.com/ag-ui-protocol/ag-ui)
at commit `2330368a2a74573902858867072406638ba3073d`. `SOURCE.json` is the
machine-readable record: the commit, the git tree of each directory there, and
what was left out.

| Here | Upstream | What |
| --- | --- | --- |
| `streams/` | `spec/1.0/conformance/streams/` | 67 of the 68 event streams, each with the outcome the specification requires, and upstream's `MANIFEST.txt` |
| `fixtures/` | `spec/1.0/fixtures/` | 219 documents a definition of the schema must accept or reject, the 84 `.expect.json` files naming the rejecting rule, and `MANIFEST.txt` |
| `CONFORMANCE.md` | `spec/1.0/conformance/README.md` | upstream's description of the stream format |
| `LICENSE` | `LICENSE` | MIT, the licence these files are under |

`streams/unknown-enum-value-role-fatal.json` is not vendored. Upstream's README
says it pins a gap in its reference client: an unrecognised message role fails
the run there, where the specification says to strip it. This SDK strips it, and
`../local/streams/unknown-enum-value-role-stripped.json` asserts that instead.

`tests/conformance_1_0.rs` runs every stream and `tests/fixtures_1_0.rs` every
document. Both check the directory against `MANIFEST.txt` in both directions.

Do not edit these files. To update, copy both directories again from a newer
upstream commit, then update `SOURCE.json`, the counts above and `UPSTREAM_COMMIT`
in `tests/conformance_1_0.rs`. `cargo run -p xtask -- corpus-check` compares the
copy with the recorded commit and says whether upstream has moved since; it
needs the network and runs in the scheduled `upstream freshness` job.
