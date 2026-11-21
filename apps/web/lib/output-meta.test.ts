import assert from "node:assert/strict";
import { test } from "node:test";

import {
  deliveryFailed,
  deliveryReason,
  deliveryStatus,
  eventsLabel,
  outputInput,
  outputProblem,
  outputState,
} from "./output-meta.ts";
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

test("a failed or abandoned last delivery marks an output that is on as failing", () => {
  const delivery = { id: "d", attempts: 1, createdAt: "", updatedAt: "" };
  assert.equal(outputState({ ...base, lastDelivery: { ...delivery, status: "failed" } }), "failing");
  assert.equal(outputState({ ...base, lastDelivery: { ...delivery, status: "dead" } }), "failing");
  assert.equal(outputState({ ...base, lastDelivery: { ...delivery, status: "succeeded" } }), "sending");
  assert.equal(outputState({ ...base, lastDelivery: { ...delivery, status: "pending" } }), "sending");
  // Off and incomplete say more than a stale failure.
  assert.equal(outputState({ ...base, enabled: false, lastDelivery: { ...delivery, status: "failed" } }), "off");
  assert.equal(deliveryFailed(null), false);
});

test("delivery status and reason read plainly", () => {
  assert.equal(deliveryStatus({ status: "succeeded", attempts: 1 }).label, "Delivered");
  assert.equal(deliveryStatus({ status: "pending", attempts: 0 }).label, "Queued");
  assert.equal(deliveryStatus({ status: "pending", attempts: 2 }).label, "Retrying");
  assert.equal(deliveryStatus({ status: "dead", attempts: 5 }).label, "Gave up");
  assert.equal(deliveryReason({ status: "failed", responseStatus: 404, error: "webhook returned HTTP 404" }), "HTTP 404");
  assert.equal(deliveryReason({ status: "pending", responseStatus: null, error: "timed out" }), "timed out");
  assert.equal(deliveryReason({ status: "succeeded", responseStatus: 200, error: null }), null);
});

test("outputInput carries every field and leaves the webhook URL out", () => {
  const input = outputInput(base, { enabled: false });
  assert.equal(input.enabled, false);
  assert.equal(input.teamId, "t1");
  assert.equal(input.template, "Ping");
  assert.equal("webhookUrl" in input, false);
});
