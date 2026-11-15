# AG-UI 1.0 schema baseline

`events.json` is generated from upstream `spec/1.0/schema.json`. It records the normative
event union, field names and requiredness, and stable signatures for the schema
root and every `$defs` shape. Signatures exclude descriptive text, so a field
type, union or root constraint change demands review even when the event and
field names stay the same.

Despite its name it also records every other `$defs` shape — capabilities,
messages, content parts, outcomes, tools, patches — with field names,
requiredness, wire kinds, union members and enum values. Those are compared
with the Rust types under `crates/ag-ui/src/` (the crate root, read flat, and
`event/`) through the table in `xtask/src/drift/shapes.rs`, which names the
Rust type for each definition. A definition with no row there fails the check,
and so does a difference not listed among that file's named exceptions. The
exceptions are printed on every run, with the reason each is kept.

From this standalone repository, run:

    cargo run -p xtask -- drift-check
    cargo run -p xtask -- drift-check --local

The default command compares the reviewed baseline with the Rust types. The
`--local` command also compares the vendored schema at
`crates/ag-ui/src/protocol/schema-1.0.json` with that baseline, so it checks
everything the default does and is the one CI runs. Both compare event names,
fields, requiredness and wire types with `crates/ag-ui/src/event/`, and every
other shape as above. The five retired
`THINKING_*` Rust variants are retained for older recordings and explicitly
excluded from the normative 1.0 union comparison. A missing local schema is a
check error, never a clean result.

Other modes:

    cargo run -p xtask -- drift-check              # offline baseline vs Rust
    cargo run -p xtask -- drift-check --upstream   # also check latest upstream schema
    cargo run -p xtask -- drift-check --draft      # also report what spec/draft adds
    cargo run -p xtask -- drift-check --refresh    # regenerate reviewed baseline

`--upstream`, `--draft` and `--refresh` need network access. An unavailable
upstream freshness check exits with a check error; it does not report success.
`--draft` is informational: what the unreleased draft adds never changes the
exit code. After a schema change, review the baseline diff and update the Rust
implementation in the same pull request.

The Rust source scanner classifies supported Rust field types by their JSON
wire kind and fails when a type cannot be classified. An integer the schema
bounds at `minimum: 0` is its own kind, `integer>=0`, which only an unsigned
Rust integer matches. Schema signatures catch deeper changes for review. This
check does not prove full nested serialization equivalence; wire round-trip and
conformance tests cover that separately.
