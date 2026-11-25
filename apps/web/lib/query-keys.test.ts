import assert from "node:assert/strict";
import { test } from "node:test";

import { queryKeys, sharingInvalidationKeys } from "./query-keys.ts";

/** Whether invalidating `key` refreshes `query` (React Query matches by key prefix). */
const covers = (keys: (readonly unknown[])[], query: readonly unknown[]) =>
  keys.some((key) => key.every((part, index) => JSON.stringify(part) === JSON.stringify(query[index])));

test("an access change refreshes the resource's real list and detail queries (#400)", () => {
  const id = "11111111-1111-1111-1111-111111111111";
  assert.ok(covers(sharingInvalidationKeys("agent_teams", id), queryKeys.agentTeams));
  assert.ok(covers(sharingInvalidationKeys("agent_teams", id), queryKeys.agentTeam(id)));
  assert.ok(covers(sharingInvalidationKeys("worker_profiles", id), queryKeys.workerProfiles));
  assert.ok(covers(sharingInvalidationKeys("env_mappings", id), ["env-mappings"]));
  assert.ok(covers(sharingInvalidationKeys("env_mappings", id), ["worker-profile-env-mappings", "p1"]));
  assert.ok(covers(sharingInvalidationKeys("agents", id), queryKeys.agents({ status: "current" })));
  assert.ok(covers(sharingInvalidationKeys("agents", id), queryKeys.agent(id)));
  assert.ok(covers(sharingInvalidationKeys("skills", id), queryKeys.skill(id)));
  assert.ok(covers(sharingInvalidationKeys("providers", id), queryKeys.providers));
  assert.ok(covers(sharingInvalidationKeys("connections", id), queryKeys.connections));
  for (const resource of ["agents", "agent_teams", "worker_profiles", "env_mappings", "providers", "connections", "skills"]) {
    const keys = sharingInvalidationKeys(resource, id);
    assert.ok(covers(keys, ["sharing", resource, id]), `${resource}: its sharing query`);
    assert.ok(covers(keys, ["team-resources", "t1"]), `${resource}: access team resource lists`);
    assert.ok(!keys.some((key) => key[0] === resource && resource.includes("_")), `${resource}: no snake_case key`);
  }
});
