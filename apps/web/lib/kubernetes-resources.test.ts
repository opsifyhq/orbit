import assert from "node:assert/strict";
import { test } from "node:test";

import { allowedKubernetesResources, kubernetesResources } from "./kubernetes-resources.ts";

test("a new connection derives the list from its capabilities", () => {
  assert.deepEqual(allowedKubernetesResources(undefined, ["pods.list", "events.list"], []), ["events", "pods"]);
  assert.deepEqual(allowedKubernetesResources([], ["pods.list"], []), ["pods"]);
});

test("a derived list follows later capability changes", () => {
  const created = kubernetesResources(["pods.list"]);
  assert.deepEqual(created, ["pods"]);
  // Edit: add pods.logs.
  assert.deepEqual(allowedKubernetesResources(created, ["pods.list", "pods.logs"], ["pods.list"]), [
    "pods",
    "pods/log",
  ]);
  // Edit: remove capabilities.
  const wide = kubernetesResources(["pods.list", "deployments.list", "events.list"]);
  assert.deepEqual(
    allowedKubernetesResources(wide, ["pods.list"], ["pods.list", "deployments.list", "events.list"]),
    ["pods"],
  );
  // Order of the stored list does not make it an override.
  assert.deepEqual(
    allowedKubernetesResources(["pods", "events"], ["events.list"], ["pods.list", "events.list"]),
    ["events"],
  );
});

test("an explicit override survives an edit", () => {
  const override = ["pods", "pods/log", "configmaps"];
  assert.deepEqual(allowedKubernetesResources(override, ["pods.list", "events.list"], ["pods.list"]), override);
});
