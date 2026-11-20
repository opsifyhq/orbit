import assert from "node:assert/strict";
import { test } from "node:test";

import { buildUpstreamPath } from "./proxy-path.ts";

test("accepts safe segments and encodes them once", () => {
  assert.deepEqual(buildUpstreamPath(["v1", "agents", "a b"]), {
    ok: true,
    path: "v1/agents/a%20b",
  });
  assert.deepEqual(buildUpstreamPath(["v1", "agents", "naïve+name"]), {
    ok: true,
    path: "v1/agents/na%C3%AFve%2Bname",
  });
});

test("rejects empty paths and empty segments", () => {
  for (const segments of [[], ["v1", "", "agents"]]) {
    const result = buildUpstreamPath(segments);
    assert.equal(result.ok, false);
  }
});

test("rejects dot segments and path traversal", () => {
  for (const segments of [
    ["v1", "."],
    ["v1", ".."],
    ["v1", "a/b"],
    ["v1", "..%2f..%2fmetrics"],
    ["v1", "%2e%2e"],
  ]) {
    const result = buildUpstreamPath(segments);
    assert.equal(result.ok, false, JSON.stringify(segments));
  }
});

test("rejects decoded query, fragment, backslash and control characters", () => {
  for (const segments of [
    ["v1", "x?y=1"],
    ["v1", "x#y"],
    ["v1", "x\\y"],
    ["v1", "x\u0000y"],
    ["v1", "x%3Fy"],
    ["v1", "x%23y"],
    ["v1", "x%5Cy"],
  ]) {
    const result = buildUpstreamPath(segments);
    assert.equal(result.ok, false, JSON.stringify(segments));
  }
});
