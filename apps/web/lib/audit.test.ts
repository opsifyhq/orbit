import assert from "node:assert/strict";
import { test } from "node:test";

import { AUDIT_AREAS, auditMetadataText, auditNextCursor, auditTargetLabel } from "./audit.ts";

test("the next cursor is the numeric id, and absent on the last page", () => {
  assert.equal(auditNextCursor({ nextCursor: "4711" }), 4711);
  assert.equal(auditNextCursor({ nextCursor: null }), undefined);
  assert.equal(auditNextCursor({}), undefined);
  assert.equal(auditNextCursor({ nextCursor: "not-a-number" }), undefined);
});

test("a row names its target, falling back to the short id", () => {
  const id = "0b7c6f9e-1d2a-4c55-9a77-1f00aa11bb22";
  assert.equal(auditTargetLabel({ targetId: id, metadata: { name: "Nightly triage" } }), "Nightly triage");
  assert.equal(auditTargetLabel({ targetId: id, metadata: { slug: "triage" } }), "triage");
  assert.equal(auditTargetLabel({ targetId: id, metadata: { name: "  " } }), "0b7c6f9e");
  assert.equal(auditTargetLabel({ targetId: id, metadata: {} }), "0b7c6f9e");
  assert.equal(auditTargetLabel({ targetId: null, metadata: [] }), undefined);
});

test("empty metadata shows nothing; other metadata is pretty JSON", () => {
  assert.equal(auditMetadataText({}), undefined);
  assert.equal(auditMetadataText(null), undefined);
  assert.equal(auditMetadataText({ fields: ["name"] }), '{\n  "fields": [\n    "name"\n  ]\n}');
});

test("area values are action families the API accepts", () => {
  for (const area of AUDIT_AREAS) assert.match(area.value, /^[a-z0-9_]+$/);
  assert.equal(new Set(AUDIT_AREAS.map((area) => area.value)).size, AUDIT_AREAS.length);
});
