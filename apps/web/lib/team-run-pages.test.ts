import assert from "node:assert/strict";
import { test } from "node:test";

import {
  hasActiveRun,
  hasActiveRunOutsideHead,
  headOutrunsPages,
  mergeRunPages,
} from "./team-run-pages.ts";

type Run = { id: string; createdAt: string; status: string };

/** Run n was created n minutes after 10:00, so a higher n is newer. */
function run(n: number, status = "succeeded"): Run {
  return {
    id: `run-${String(n).padStart(3, "0")}`,
    createdAt: new Date(Date.UTC(2026, 0, 1, 10, n)).toISOString(),
    status,
  };
}

function page(runs: Run[], more = true) {
  return { items: runs, nextCursor: more ? `cursor-${runs.at(-1)?.id}` : null, total: 99 };
}

const ids = (runs: Run[]) => runs.map((r) => Number(r.id.slice(4)));

test("the polled newest page replaces the first loaded page and keeps older pages", () => {
  const loaded = [page([run(9), run(8), run(7)]), page([run(6), run(5), run(4)])];
  // Run 8 finished and run 10 started since the pages were loaded; 7 moved off the head.
  const head = page([run(10, "running"), run(9), run(8, "failed")]);
  const merged = mergeRunPages(head, loaded);
  assert.deepEqual(ids(merged), [10, 9, 8, 7, 6, 5, 4]);
  assert.equal(merged[2].status, "failed");
});

test("a run the newest page no longer lists is dropped, older ones stay", () => {
  const loaded = [page([run(9), run(8), run(7)]), page([run(6), run(5)], false)];
  // Run 8 was archived: the head now reaches back to 6.
  const head = page([run(9), run(7), run(6)]);
  assert.deepEqual(ids(mergeRunPages(head, loaded)), [9, 7, 6, 5]);
});

test("a newest page that is the whole list is shown as it is", () => {
  const loaded = [page([run(3), run(2), run(1)])];
  assert.deepEqual(ids(mergeRunPages(page([run(3), run(1)], false), loaded)), [3, 1]);
  assert.deepEqual(ids(mergeRunPages(undefined, loaded)), [3, 2, 1]);
});

test("active runs: anywhere, and outside the polled page", () => {
  assert.equal(hasActiveRun([run(1), run(2, "running")]), true);
  assert.equal(hasActiveRun([run(1), run(2, "cancelled")]), false);
  const head = page([run(9), run(8)]);
  assert.equal(hasActiveRunOutsideHead(head, [page([run(9), run(8)])]), false);
  assert.equal(hasActiveRunOutsideHead(head, [page([run(9), run(8)]), page([run(5, "running")])]), true);
  assert.equal(hasActiveRunOutsideHead(head, [page([run(9), run(8)]), page([run(5)])]), false);
});

test("a newest page that moved past the loaded pages asks for a full refetch", () => {
  const loaded = [page([run(3), run(2)]), page([run(1)], false)];
  assert.equal(headOutrunsPages(page([run(4), run(3)]), loaded), false);
  // Two new runs arrived and the page holds two: runs 3 and 2 left the head.
  assert.equal(headOutrunsPages(page([run(6), run(5)]), loaded), true);
  assert.equal(headOutrunsPages(page([run(6), run(5)], false), loaded), false);
});
