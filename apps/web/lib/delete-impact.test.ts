import assert from "node:assert/strict";
import { test } from "node:test";

import { ApiError } from "./api-error.ts";
import { deleteFailureMessage, stillUsedMessage, usedByFromError } from "./delete-impact.ts";

test("usedByFromError reads only usedBy details of a 409", () => {
  const conflict = new ApiError(409, "provider is still used by agent Triage", {
    code: "conflict",
    details: [
      { path: "usedBy", message: "agent Triage" },
      { path: "usedBy", message: "worker profile ci" },
      { path: "other", message: "ignored" },
    ],
  });
  assert.deepEqual(usedByFromError(conflict), ["agent Triage", "worker profile ci"]);
  assert.deepEqual(
    usedByFromError(new ApiError(422, "bad", { details: [{ path: "usedBy", message: "x" }] })),
    [],
  );
  assert.deepEqual(usedByFromError(new Error("plain")), []);
});

test("deleteFailureMessage names the dependants or falls back", () => {
  const conflict = new ApiError(409, "in use", {
    details: [{ path: "usedBy", message: "agent team Release" }],
  });
  assert.equal(
    deleteFailureMessage("connection", conflict, "fallback"),
    "This connection is still used by agent team Release. Remove it there first.",
  );
  assert.equal(deleteFailureMessage("connection", new Error("boom"), "boom"), "boom");
  assert.equal(stillUsedMessage("agent", ["a", "b"]), "This agent is still used by a, b. Remove it there first.");
});
