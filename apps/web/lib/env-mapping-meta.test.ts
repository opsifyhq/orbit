import assert from "node:assert/strict";
import { test } from "node:test";

import {
  brokenVariables,
  draftIssues,
  emptyRow,
  envState,
  nameError,
  suggestName,
  toEntries,
  toRows,
  variableSummary,
} from "./env-mapping-meta.ts";

const mapping = {
  mappings: [
    { envVar: "GITLAB_TOKEN", connectionId: "c1" },
    { envVar: "SLACK_URL", secretKey: "slack", secretRef: "secret://env/1" },
    { envVar: "OLD_TOKEN", connectionId: "gone" },
    { envVar: "BARE", connectionId: "c2" },
  ],
};

test("rows round-trip to entries with one source each, keeping the secret ref", () => {
  const rows = toRows(mapping);
  assert.equal(rows[1].source, "secret");
  rows[0] = { ...rows[0], secretKey: "stale" };
  rows.push(emptyRow());
  const entries = toEntries(rows);
  assert.equal(entries.length, 4, "empty rows are dropped");
  assert.deepEqual(entries[0], { envVar: "GITLAB_TOKEN", connectionId: "c1", secretKey: null });
  assert.equal(entries[1].secretRef, "secret://env/1");
});

test("names are checked for shape and duplicates", () => {
  const names = ["A", "A", "1BAD"];
  assert.equal(nameError({ ...emptyRow(), envVar: "A" }, names), "This name is used twice");
  assert.match(nameError({ ...emptyRow(), envVar: "1BAD" }, names) ?? "", /not starting with a digit/);
  assert.equal(suggestName("gitlab-ce"), "GITLAB_CE_TOKEN");
});

test("draftIssues lists what blocks a save by section", () => {
  const issues = draftIssues("", [{ ...emptyRow(), envVar: "TOKEN" }]);
  assert.deepEqual(
    issues.map((issue) => issue.section),
    ["details", "variables"],
  );
  assert.equal(issues[1].message, "Choose where TOKEN comes from.");
  assert.deepEqual(draftIssues("x", [emptyRow()]).map((issue) => issue.message), ["Add at least one variable."]);
  assert.deepEqual(draftIssues("x", toRows(mapping)), []);
});

test("variables from a missing or credential-less connection make it need attention", () => {
  const connections = new Map([
    ["c1", { id: "c1", name: "GitLab", connectorSlug: "gitlab", hasCredential: true }],
    ["c2", { id: "c2", name: "Bare", connectorSlug: "webhook", hasCredential: false }],
  ]);
  assert.deepEqual(brokenVariables(mapping, connections), [
    { envVar: "OLD_TOKEN", reason: "missing" },
    { envVar: "BARE", reason: "no_credential" },
  ]);
  assert.deepEqual(brokenVariables(mapping, null), [], "unknown until connections load");
  assert.equal(envState(2, 0), "attention");
  assert.equal(envState(0, 1), "in_use");
  assert.equal(envState(0, 0), "unused");
  assert.equal(variableSummary(mapping), "GITLAB_TOKEN, SLACK_URL and 2 more");
  assert.equal(variableSummary({ mappings: mapping.mappings.slice(0, 3) }), "GITLAB_TOKEN, SLACK_URL, OLD_TOKEN");
});
