import assert from "node:assert/strict";
import { test } from "node:test";

import { autonomyDraft, DEFAULT_FIXED_APPROVAL, DEFAULT_FIXED_LEVEL, teamAutonomyInput } from "./team-autonomy.ts";

test("a new fixed policy keeps each agent's approval mode and asks before writes", () => {
  const draft = autonomyDraft(null);
  assert.equal(draft.mode, "follow_primary");
  assert.equal(draft.level, DEFAULT_FIXED_LEVEL);
  assert.equal(DEFAULT_FIXED_APPROVAL, "agent");
  assert.deepEqual(teamAutonomyInput("fixed", draft.level, draft.approval), { mode: "fixed", level: 3 });
});

test("an explicit approval mode is sent, and follow-primary sends nothing", () => {
  assert.deepEqual(teamAutonomyInput("fixed", "4", "always"), { mode: "fixed", level: 4, approvalMode: "always" });
  assert.equal(teamAutonomyInput("follow_primary", "4", "auto"), null);
});

test("a saved policy round-trips, including one without an approval mode", () => {
  assert.deepEqual(autonomyDraft({ mode: "fixed", level: 4, approvalMode: "auto" }), {
    mode: "fixed",
    level: "4",
    approval: "auto",
  });
  assert.deepEqual(autonomyDraft({ mode: "fixed", level: 2 }), { mode: "fixed", level: "2", approval: "agent" });
});
