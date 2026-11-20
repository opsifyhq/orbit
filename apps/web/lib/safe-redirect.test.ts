import assert from "node:assert/strict";
import { test } from "node:test";

import { currentLocationPath, loginPathFor, safeNextPath } from "./safe-redirect.ts";

test("next URLs keep the query string and fragment", () => {
  assert.equal(
    loginPathFor("/agent-teams?run=42#queue"),
    "/login?next=%2Fagent-teams%3Frun%3D42%23queue",
  );
  assert.equal(safeNextPath("/agent-teams?run=42#queue"), "/agent-teams?run=42#queue");
});

test("next URLs still reject unsafe targets", () => {
  for (const target of ["//evil.example/?x=1", "https://evil.example", "/\\evil.example", null]) {
    assert.equal(loginPathFor(target), "/login?next=%2F", String(target));
  }
});

test("currentLocationPath is SSR-safe", () => {
  assert.equal(currentLocationPath(), "/");
});
