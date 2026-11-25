import assert from "node:assert/strict";
import { test } from "node:test";

import { resourceSaveOutcome } from "./resource-save.ts";

test("a full save reports success and goes to the resource", () => {
  assert.deepEqual(
    resourceSaveOutcome({ mode: "edit", saved: "Agent saved", href: "/agents/a1", accessSaved: true }),
    { tone: "success", message: "Agent saved", href: "/agents/a1" },
  );
  assert.equal(
    resourceSaveOutcome({ mode: "create", saved: "Skill created", href: "/skills/s1", accessSaved: true }).href,
    "/skills/s1",
  );
});

test("a refused access change on edit keeps the user on the form", () => {
  const outcome = resourceSaveOutcome({
    mode: "edit",
    saved: "Agent team saved",
    href: "/agent-teams/t1",
    accessSaved: false,
  });
  assert.equal(outcome.tone, "warning");
  assert.equal(outcome.href, null);
  assert.match(outcome.message, /^Agent team saved, but access could not be updated/);
});

test("a refused access change on create goes to the edit page, not the create form", () => {
  const outcome = resourceSaveOutcome({
    mode: "create",
    saved: "Connection created",
    href: "/connections/c1",
    accessSaved: false,
  });
  assert.equal(outcome.tone, "warning");
  assert.equal(outcome.href, "/connections/c1/edit");
  assert.match(outcome.message, /access could not be updated/);
});
