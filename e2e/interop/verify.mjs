import assert from "node:assert/strict";
import { execFileSync } from "node:child_process";
import { readFileSync } from "node:fs";
import { fileURLToPath } from "node:url";
import {
  EventSchemas, RunAgentInputSchema, RunFinishedOutcomeSchema,
  SubagentFinishedOutcomeSchema,
} from "@ag-ui/core";

const root = fileURLToPath(new URL("../../", import.meta.url));
const cases = JSON.parse(readFileSync(new URL("cases.json", import.meta.url), "utf8"));
const rust = JSON.parse(execFileSync("cargo", [
  "run", "--locked", "--quiet", "-p", "ag-ui", "--example", "semantic_probe",
], {cwd: root, input: JSON.stringify(cases), encoding: "utf8", stdio: ["pipe", "pipe", "inherit"]}));
const schemas = {
  event: EventSchemas, input: RunAgentInputSchema, outcome: RunFinishedOutcomeSchema,
  subagentOutcome: SubagentFinishedOutcomeSchema,
};
const failures = [];
let differences = 0;
assert.equal(rust.length, cases.length);
assert.equal(new Set(cases.map(c => c.name)).size, cases.length);
for (const [index, fixture] of cases.entries()) {
  try {
    const parsed = schemas[fixture.kind].safeParse(fixture.input);
    const actual = rust[index];
    assert.equal(actual.name, fixture.name);
    assert.equal(parsed.success, fixture.accepted, "official schema verdict");
    assert.equal(actual.accepted, parsed.success, "Rust acceptance differs from official schema");
    if (parsed.success) {
      const upstream = JSON.parse(JSON.stringify(parsed.data));
      if (Object.hasOwn(fixture, "rustValue")) {
        assert(fixture.difference?.length, "a representation difference needs an explanation");
        assert.deepEqual(upstream, fixture.upstreamValue, "upstream representation changed");
        assert.notDeepEqual(fixture.rustValue, fixture.upstreamValue);
        assert.deepEqual(actual.value, fixture.rustValue, "documented Rust representation changed");
        differences += 1;
      } else {
        assert.deepEqual(actual.value, upstream, "normalized wire values differ");
      }
    }
  } catch (error) {
    failures.push(`${fixture.name}: ${error.message}`);
  }
}
assert.equal(failures.length, 0, failures.join("\n"));
console.log(`PASS ${cases.length} cases against @ag-ui/core@0.0.59 (${differences} documented typed representation differences)`);
