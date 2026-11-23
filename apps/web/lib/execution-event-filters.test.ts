import assert from "node:assert/strict";
import { test } from "node:test";

import type { ExecutionEventRecord } from "@/lib/types";
import {
  DEFAULT_EVENT_FILTERS,
  eventFilterParams,
  eventType,
  filterEvents,
  groupTimelineEvents,
  matchesEventFilters,
  parseEventFilters,
  toolLabel,
  toolOptions,
} from "./execution-event-filters.ts";

function event(
  seq: number,
  kind: string,
  message: string,
  payload: Record<string, unknown> = {},
  level = "info",
): ExecutionEventRecord {
  return {
    id: seq,
    seq,
    orgId: "org",
    executionId: "execution",
    kind,
    level,
    message,
    data: payload,
    createdAt: "2026-01-01T10:00:00Z",
  };
}

const output = event(1, "output", "I found three issues");
const tool = event(2, "tool_call", "github.issues_list executed", {
  tool: "github_issues_list",
  effect: "read",
  connector: "github",
  arguments: { token: "super-secret" },
});
const capability = event(3, "tool_call", "issues.comment executed", {
  capability: "issues.comment",
  effect: "write",
  connectionName: "ORBIT on-call",
});
const approval = event(4, "tool_call", "approval requested for issues.comment (approval 123)", {
  capability: "issues.comment",
  effect: "external_side_effect",
  level: "warn",
});
const failure = event(5, "error", "worker heartbeat expired", {}, "error");
const leaseFailure = event(7, "error", "worker stopped reporting this execution; outcome is unknown", {}, "error");
const bootFailure = event(8, "error", "worker restarted; execution outcome is unknown", {}, "error");
const status = event(6, "status", "execution dispatched to harness");
const all = [output, tool, capability, approval, failure, status];

test("filter state is parsed from and written to the URL", () => {
  const parsed = parseEventFilters(
    new URLSearchParams("events=tools&tool=github_issues_list&effect=read"),
  );
  assert.deepEqual(parsed, { type: "tools", tool: "github_issues_list", effect: "read" });
  assert.deepEqual(eventFilterParams(parsed), {
    events: "tools",
    tool: "github_issues_list",
    effect: "read",
  });
  assert.deepEqual(parseEventFilters(new URLSearchParams("events=bad&effect=一切")), DEFAULT_EVENT_FILTERS);
  assert.deepEqual(eventFilterParams(DEFAULT_EVENT_FILTERS), {});
  assert.equal(
    parseEventFilters(new URLSearchParams(`tool=${"x".repeat(400)}`)).tool.length,
    200,
  );
});

test("event types classify approvals and errors before tools", () => {
  assert.equal(eventType(output), "messages");
  assert.equal(eventType(tool), "tools");
  assert.equal(eventType(approval), "approvals");
  assert.equal(eventType(failure), "errors");
  assert.equal(eventType(leaseFailure), "errors");
  assert.equal(eventType(bootFailure), "errors");
  assert.equal(eventType(status), "system");
  assert.equal(eventType(event(7, "log", "debug", {}, "error")), "errors");
});

test("tool text matches names, capabilities, connectors, connections and messages", () => {
  for (const query of [
    "GITHUB_ISSUES_LIST",
    "issues.comment",
    "on-call",
    "github",
    "found three issues",
  ]) {
    const filters = { ...DEFAULT_EVENT_FILTERS, tool: query };
    assert.ok(all.some((item) => matchesEventFilters(item, filters)), query);
  }
  assert.equal(matchesEventFilters(tool, { ...DEFAULT_EVENT_FILTERS, tool: "kubernetes" }), false);
  assert.equal(matchesEventFilters(tool, { ...DEFAULT_EVENT_FILTERS, tool: "super-secret" }), false);
});

test("effect filter only matches events that carry that effect", () => {
  assert.equal(matchesEventFilters(tool, { ...DEFAULT_EVENT_FILTERS, effect: "read" }), true);
  assert.equal(matchesEventFilters(tool, { ...DEFAULT_EVENT_FILTERS, effect: "write" }), false);
  assert.equal(matchesEventFilters(approval, { ...DEFAULT_EVENT_FILTERS, effect: "external_side_effect" }), true);
  assert.equal(matchesEventFilters(status, { ...DEFAULT_EVENT_FILTERS, effect: "read" }), false);
});

test("Story view, type and text filters compose without changing order", () => {
  const isKey = (item: ExecutionEventRecord) => item.kind !== "log";
  const result = filterEvents(
    all,
    { type: "tools", tool: "issues.comment", effect: "all" },
    isKey,
    false,
  );
  assert.deepEqual(result.visible.map((item) => item.seq), [3]);
  assert.equal(result.hiddenCount, 5);

  const cleared = filterEvents(all, DEFAULT_EVENT_FILTERS, isKey, true);
  assert.deepEqual(cleared.visible.map((item) => item.seq), all.map((item) => item.seq));
  assert.equal(cleared.hiddenCount, 0);
});

test("approval and error filters do not include ordinary tool calls", () => {
  const isKey = () => true;
  assert.deepEqual(
    filterEvents(all, { ...DEFAULT_EVENT_FILTERS, type: "approvals" }, isKey, true).visible,
    [approval],
  );
  assert.deepEqual(
    filterEvents(all, { ...DEFAULT_EVENT_FILTERS, type: "errors" }, isKey, true).visible,
    [failure],
  );
});

test("repeated tool calls fold into one group; errors and approvals stay expanded", () => {
  const events = [
    tool,
    { ...tool, seq: 7 },
    { ...tool, seq: 8 },
    approval,
    failure,
    { ...capability, seq: 9 },
    { ...capability, seq: 10 },
  ];
  const entries = groupTimelineEvents(events, true);
  assert.deepEqual(
    entries.map((entry) => entry.kind),
    ["group", "event", "event", "group"],
  );
  const group = entries[0];
  assert.equal(group.kind, "group");
  if (group.kind === "group") {
    assert.equal(group.label, "Github issues list");
    assert.deepEqual(group.events.map((item) => item.seq), [2, 7, 8]);
  }
  assert.deepEqual(
    groupTimelineEvents(events, false).map((entry) => entry.kind),
    events.map(() => "event"),
  );
});

test("datalist labels come only from identifiers, never payload values", () => {
  assert.deepEqual(toolOptions([tool, capability, output]), [
    { value: "github_issues_list", label: "Github issues list" },
    { value: "github", label: "Github" },
    { value: "issues.comment", label: "Comment Issues" },
    { value: "ORBIT on-call", label: "ORBIT on call" },
  ]);
  assert.equal(toolLabel("pods.logs"), "Logs Pods");
  assert.ok(!JSON.stringify(toolOptions([tool])).includes("super-secret"));
});
