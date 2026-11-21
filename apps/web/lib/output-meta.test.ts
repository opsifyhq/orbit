import assert from "node:assert/strict";
import { test } from "node:test";

import { eventsLabel, outputInput, outputProblem, outputState } from "./output-meta.ts";
import type { OutputView } from "./outputs.ts";

const base: OutputView = {
  id: "o1",
  name: "Alerts",
  kind: "slack",
  agentId: null,
  teamId: "t1",
  events: ["succeeded", "failed", "cancelled"],
  template: "Ping",
  enabled: true,
  hasWebhook: true,
  createdAt: "2026-01-01T00:00:00Z",
  updatedAt: "2026-01-01T00:00:00Z",
};

test("eventsLabel reads as a sentence in a fixed order", () => {
  assert.equal(eventsLabel(["failed", "succeeded", "cancelled"]), "Every finished run");
  assert.equal(eventsLabel(["failed"]), "Failed");
  assert.equal(eventsLabel(["cancelled", "failed"]), "Failed or cancelled");
  assert.equal(eventsLabel([]), "Never: no events chosen");
});

test("outputState says whether an output will send", () => {
  assert.equal(outputState(base), "sending");
  assert.equal(outputState({ ...base, enabled: false }), "off");
  assert.equal(outputState({ ...base, hasWebhook: false }), "incomplete");
  assert.equal(outputState({ ...base, enabled: false, events: [] }), "incomplete");
  assert.equal(outputProblem({ ...base, hasWebhook: false }), "No webhook URL");
  assert.equal(outputProblem(base), null);
});

test("outputInput carries every field and leaves the webhook URL out", () => {
  const input = outputInput(base, { enabled: false });
  assert.equal(input.enabled, false);
  assert.equal(input.teamId, "t1");
  assert.equal(input.template, "Ping");
  assert.equal("webhookUrl" in input, false);
});
