import assert from "node:assert/strict";
import { test } from "node:test";

import {
  EMPTY_DRAFT,
  draftFrom,
  draftIssues,
  endpointLabel,
  isOrbitManagedProvider,
  parseModels,
  probeBody,
  providerState,
  renderConfig,
} from "./provider-meta.ts";

const stored = {
  id: "p1",
  name: "Gateway",
  kind: "openai_compatible",
  enabled: true,
  hasCredential: true,
  config: {
    baseUrl: "https://gw.example.com/v1",
    model: "qwen3",
    headers: { "x-team": "a" },
    requestParams: { temperature: 0.2 },
    compat: { chatCompletions: true, tools: true, streaming: false, embeddings: true },
    contextWindow: 128000,
  },
};

test("a stored provider round-trips through the draft without losing hidden fields", () => {
  const draft = draftFrom(stored);
  assert.equal(draft.baseUrl, "https://gw.example.com/v1");
  assert.equal(draft.tools, true);
  assert.equal(draft.streaming, false);
  assert.equal(draft.contextWindow, "128000");
  assert.equal(draft.credential, "", "credentials are write-only");
  const config = renderConfig({ ...draft, model: "llama", vision: true }, stored.config);
  assert.equal(config.model, "llama");
  assert.deepEqual(config.headers, { "x-team": "a" });
  assert.deepEqual(config.requestParams, { temperature: 0.2 });
  const compat = config.compat as Record<string, boolean>;
  assert.equal(compat.vision, true);
  assert.equal(compat.embeddings, true, "flags the form does not show are kept");
});

test("a new endpoint gets the request shape the API expects", () => {
  const config = renderConfig({ ...EMPTY_DRAFT, name: "x", baseUrl: "https://a.example/v1/", model: "m" });
  assert.equal(config.baseUrl, "https://a.example/v1");
  assert.deepEqual(config.headers, {});
  assert.equal((config.compat as Record<string, boolean>).chatCompletions, true);
  assert.equal("settings" in config, false, "no empty catalog on create");
  const withCatalog = renderConfig({ ...EMPTY_DRAFT, name: "x", baseUrl: "https://a.example/v1", model: "m", models: ["m", "n"] });
  assert.deepEqual(withCatalog.settings, { models: ["m", "n"] });
});

test("an edit always sends the catalog, so clearing it sticks, and keeps other settings", () => {
  const existing = { ...stored.config, settings: { models: ["a"], note: "kept" } };
  const config = renderConfig({ ...draftFrom({ ...stored, config: existing }), models: [] }, existing);
  assert.deepEqual(config.settings, { models: [], note: "kept" });
});

test("Anthropic defaults its base URL; harness-managed stores the catalog in settings", () => {
  assert.equal(renderConfig({ ...EMPTY_DRAFT, kind: "anthropic" }).baseUrl, "https://api.anthropic.com/v1");
  const harness = renderConfig({ ...EMPTY_DRAFT, kind: "harness_managed", models: ["a", "b"] });
  assert.deepEqual(harness.settings, { models: ["a", "b"] });
  assert.equal(harness.harness, "opencode");
  assert.equal(endpointLabel({ kind: "harness_managed", config: harness }), "OpenCode · opencode-go");
  assert.equal(endpointLabel({ kind: "anthropic", config: { model: "c" } }), "https://api.anthropic.com/v1");
});

test("parseModels splits on commas and new lines and drops duplicates", () => {
  assert.deepEqual(parseModels("a, b\nc,, a "), ["a", "b", "c"]);
});

test("draftIssues names what blocks a save", () => {
  const issues = draftIssues({ ...EMPTY_DRAFT, contextWindow: "1.5" }).map((issue) => issue.message);
  assert.ok(issues.includes("Enter a name."));
  assert.ok(issues.includes("Enter the base URL."));
  assert.ok(issues.includes("Context window must be a whole number of tokens."));
  assert.ok(issues.includes("Choose a default model. Test connection can list them."));
  assert.ok(
    draftIssues({ ...EMPTY_DRAFT, name: "a", kind: "anthropic", model: "c", contextWindow: "64" })
      .map((issue) => issue.message)
      .includes("Context window must be at least 128 tokens."),
  );
  const azure = draftIssues({ ...EMPTY_DRAFT, name: "a", model: "m", baseUrl: "https://YOUR-RESOURCE.openai.azure.com/openai/v1" });
  assert.equal(azure.length, 1);
  assert.deepEqual(draftIssues({ ...EMPTY_DRAFT, name: "a", kind: "anthropic", model: "claude" }), []);
  assert.deepEqual(draftIssues({ ...EMPTY_DRAFT, name: "a", kind: "harness_managed" }), [], "harness default is fine");
});

test("probeBody discovers models when none is set", () => {
  const body = probeBody({ ...EMPTY_DRAFT, baseUrl: "https://a.example/v1", credential: " key " });
  assert.equal(body.config.model, "discover");
  assert.equal(body.credential, "key");
});

test("providerState: disabled wins, then a failed test", () => {
  assert.equal(providerState({ enabled: true }), "ready");
  assert.equal(providerState({ enabled: true }, { ok: false, at: 1 }), "failing");
  assert.equal(providerState({ enabled: false }, { ok: false, at: 1 }), "disabled");
});

test("every non-harness provider kind is ORBIT-managed", () => {
  assert.equal(isOrbitManagedProvider("openai_compatible"), true);
  assert.equal(isOrbitManagedProvider("anthropic"), true);
  assert.equal(isOrbitManagedProvider("a_future_kind"), true);
  assert.equal(isOrbitManagedProvider("harness_managed"), false);
});
