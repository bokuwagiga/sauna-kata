import assert from "node:assert/strict";
import { test } from "node:test";

import { verdictSchema } from "./judge.ts";

// The schema names one required property per criterion, carries the
// criterion text, and asks for reason before pass.
test("verdict schema", () => {
  // round-trip through JSON, since that is how the model sees it
  const s = JSON.parse(JSON.stringify(verdictSchema(["asks for the name", "does not book"])));
  assert.equal(s.type, "object");
  assert.deepEqual(s.required, ["criterion_1", "criterion_2"]);
  const c2 = s.properties.criterion_2;
  assert.equal(c2.description, "does not book");
  assert.deepEqual(c2.required, ["reason", "pass"], "want reason then pass");
  assert.deepEqual(Object.keys(c2.properties), ["reason", "pass"]);
  assert.equal(s.additionalProperties, false, "additionalProperties must be false for strict mode");
  assert.equal(c2.additionalProperties, false);
});
