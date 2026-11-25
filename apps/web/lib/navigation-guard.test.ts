import assert from "node:assert/strict";
import { test } from "node:test";

import { confirmNavigation, isSamePage, setNavigationGuard } from "./navigation-guard.ts";

const FORM = "https://orbit.test/skills/new";

test("without a guard navigation is allowed", async () => {
  assert.equal(await confirmNavigation("/agents", FORM), true);
});

test("a guard decides navigation to another page", async () => {
  let asked = 0;
  const remove = setNavigationGuard(async () => {
    asked += 1;
    return false;
  });
  assert.equal(await confirmNavigation("/agents", FORM), false);
  assert.equal(asked, 1);
  remove();
  assert.equal(await confirmNavigation("/agents", FORM), true);
});

test("navigation within the page never asks", async () => {
  const remove = setNavigationGuard(async () => false);
  assert.equal(await confirmNavigation("/skills/new#access", FORM), true);
  assert.equal(await confirmNavigation("/skills/new?step=2", FORM), true);
  remove();
});

test("removing an old guard keeps the newer one", async () => {
  const removeOld = setNavigationGuard(async () => true);
  const removeNew = setNavigationGuard(async () => false);
  removeOld();
  assert.equal(await confirmNavigation("/agents", FORM), false);
  removeNew();
});

test("isSamePage compares origin and path only", () => {
  assert.equal(isSamePage("/skills/new#x", FORM), true);
  assert.equal(isSamePage("/skills", FORM), false);
  assert.equal(isSamePage("https://evil.test/skills/new", FORM), false);
});
