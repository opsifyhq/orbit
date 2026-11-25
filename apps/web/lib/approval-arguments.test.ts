import assert from "node:assert/strict";
import { test } from "node:test";

import { approvalArguments, rowValue, truncationNotice } from "./approval-arguments.ts";

test("complete arguments are shown whole without a notice", () => {
  const value = approvalArguments({
    arguments: { command: "ls" },
    argumentsBytes: 16,
    argumentsTruncated: false,
  });
  assert.deepEqual(value.args, { command: "ls" });
  assert.equal(value.truncated, false);
  assert.match(value.text, /"command": "ls"/);
  assert.equal(truncationNotice(value), null);
});

test("truncated arguments carry the size in a visible notice", () => {
  const value = approvalArguments({
    arguments: '{"command":"echo … [4000 of 70000 bytes left out here] …\nrm -rf /"}',
    argumentsBytes: 70000,
    argumentsTruncated: true,
  });
  assert.equal(value.truncated, true);
  assert.deepEqual(value.args, {});
  assert.match(value.text, /rm -rf/);
  assert.match(truncationNotice(value) ?? "", /too long to show in full, 70,000 bytes/);
});

test("a string from an older worker counts as truncated", () => {
  const value = approvalArguments({ arguments: '{"command":"echo hi…' });
  assert.equal(value.truncated, true);
  assert.match(truncationNotice(value) ?? "", /too long to show in full\. /);
});

test("long summary values are marked, short ones are not", () => {
  assert.equal(rowValue("short"), "short");
  const long = rowValue("x".repeat(1500));
  assert.ok(long.startsWith("x".repeat(1000)));
  assert.match(long, /truncated; see Full arguments\)$/);
});
