import assert from "node:assert/strict";
import test from "node:test";

import { canStopTeamRun, launchEndActions, launchesForView } from "./run-controls.ts";

test("a team run can be stopped only while running and with executions:cancel", () => {
  assert.equal(canStopTeamRun("running", ["executions:cancel"]), true);
  assert.equal(canStopTeamRun("running", ["executions:read"]), false);
  assert.equal(canStopTeamRun("running", undefined), false);
  for (const status of ["succeeded", "failed", "cancelled", "timed_out"]) {
    assert.equal(canStopTeamRun(status, ["executions:cancel"]), false, status);
  }
});

test("archive and delete are offered for ended launches with settings:write", () => {
  const write = ["settings:write"];
  assert.deepEqual(launchEndActions({ status: "running" }, write), { archive: false, remove: false });
  assert.deepEqual(launchEndActions({ status: "pending" }, write), { archive: false, remove: false });
  assert.deepEqual(launchEndActions({ status: "stopped" }, write), { archive: true, remove: true });
  assert.deepEqual(launchEndActions({ status: "failed", archivedAt: null }, write), { archive: true, remove: true });
  assert.deepEqual(launchEndActions({ status: "failed", archivedAt: "2026-11-25T10:00:00Z" }, write), {
    archive: false,
    remove: true,
  });
  assert.deepEqual(launchEndActions({ status: "stopped" }, ["harnesses:read"]), { archive: false, remove: false });
});

test("history and archived split the full launch list", () => {
  const launches = [{ id: "a" }, { id: "b", archivedAt: "2026-11-25T10:00:00Z" }, { id: "c", archivedAt: null }];
  assert.deepEqual(launchesForView(launches, "history").map((l) => l.id), ["a", "c"]);
  assert.deepEqual(launchesForView(launches, "archived").map((l) => l.id), ["b"]);
  assert.deepEqual(launchesForView(launches, "active").map((l) => l.id), ["a", "b", "c"]);
});
