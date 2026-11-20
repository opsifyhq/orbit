import assert from "node:assert/strict";
import { test } from "node:test";

import { mergeProviderConfig } from "./provider-config.ts";

test("create submits the rendered config unchanged", () => {
  const rendered = {
    baseUrl: "https://gateway.example.com/v1",
    headers: {},
    requestParams: {},
    compat: { chatCompletions: true, responsesApi: false, tools: false },
  };
  assert.deepEqual(mergeProviderConfig(null, rendered), rendered);
});

test("edit keeps unrendered keys and merges compat and settings", () => {
  const existing = {
    baseUrl: "https://gateway.example.com/v1",
    headers: { Authorization: "Bearer custom" },
    requestParams: { api_version: "2024-01" },
    compat: { chatCompletions: false, embeddings: true, tools: true },
    extra: { fromAnotherTool: 42 },
  };
  const rendered = {
    baseUrl: "https://gateway.example.com/v1",
    compat: { tools: false, streaming: true },
  };
  assert.deepEqual(mergeProviderConfig(existing, rendered), {
    baseUrl: "https://gateway.example.com/v1",
    headers: { Authorization: "Bearer custom" },
    requestParams: { api_version: "2024-01" },
    compat: { chatCompletions: false, embeddings: true, tools: false, streaming: true },
    extra: { fromAnotherTool: 42 },
  });
});

test("edit merges harness settings instead of replacing them", () => {
  const existing = { harness: "opencode", settings: { models: ["old"], keep: "yes" } };
  const rendered = { harness: "opencode", settings: { models: ["new"] } };
  assert.deepEqual(mergeProviderConfig(existing, rendered), {
    harness: "opencode",
    settings: { models: ["new"], keep: "yes" },
  });
});
