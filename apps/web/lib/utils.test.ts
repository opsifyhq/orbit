import assert from "node:assert/strict";
import { test } from "node:test";

import { formatDuration, formatElapsed, formatTokens } from "./utils.ts";

test("elapsed time never shows 60 of a smaller unit", () => {
  assert.equal(formatElapsed(0), "0ms");
  assert.equal(formatElapsed(605), "605ms");
  assert.equal(formatElapsed(4_200), "4.2s");
  assert.equal(formatElapsed(59_940), "59.9s");
  assert.equal(formatElapsed(59_960), "1m 0s");
  assert.equal(formatElapsed(119_600), "2m 0s");
  assert.equal(formatElapsed(125_000), "2m 5s");
  assert.equal(formatElapsed(3_599_600), "1h 0m");
});

test("long runs use hours and days", () => {
  assert.equal(formatElapsed(3 * 3_600_000), "3h 0m");
  assert.equal(formatElapsed(3 * 3_600_000 + 12 * 60_000), "3h 12m");
  assert.equal(formatElapsed(26 * 3_600_000), "1d 2h");
});

test("durations tolerate missing and invalid dates", () => {
  assert.equal(formatDuration(null), "—");
  assert.equal(formatDuration("not a date", "2026-01-01T00:00:00Z"), "—");
  assert.equal(formatDuration("2026-01-01T00:00:00Z", "2026-01-01T00:02:05Z"), "2m 5s");
  // A clock skew that puts the end before the start is not a negative duration.
  assert.equal(formatDuration("2026-01-01T00:00:10Z", "2026-01-01T00:00:00Z"), "0ms");
});

test("token counts switch unit before they overflow it", () => {
  assert.equal(formatTokens(undefined), "—");
  assert.equal(formatTokens(0), "0");
  assert.equal(formatTokens(999), "999");
  assert.equal(formatTokens(1_500), "1.5k");
  assert.equal(formatTokens(999_940), "999.9k");
  assert.equal(formatTokens(999_960), "1.0M");
  assert.equal(formatTokens(2_500_000), "2.5M");
});
