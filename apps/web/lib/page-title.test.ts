import assert from "node:assert/strict";
import { test } from "node:test";

import { pageTitle } from "./page-title.ts";

test("create and edit pages get their own title", () => {
  assert.equal(pageTitle("/agents/new", "Agents"), "New agent");
  assert.equal(pageTitle("/agent-teams/new", "Agent teams"), "New agent team");
  assert.equal(pageTitle("/models/abc/edit", "Model providers"), "Edit model provider");
  assert.equal(pageTitle("/skills/abc/edit", "Skills"), "Edit skill");
});

test("other pages use their section label", () => {
  assert.equal(pageTitle("/agents", "Agents"), "Agents");
  assert.equal(pageTitle("/agents/abc", "Agents"), "Agents");
  assert.equal(pageTitle("/agent-teams/runs", "Agent teams"), "Agent teams");
  assert.equal(pageTitle("/workers/new", "Workers"), "Workers");
  assert.equal(pageTitle("/", "Overview"), "Overview");
});
