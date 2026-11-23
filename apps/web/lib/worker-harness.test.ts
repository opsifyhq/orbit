import assert from "node:assert/strict";
import { test } from "node:test";

import {
  harnessLabel,
  harnessMismatch,
  providerHarness,
  providerOptionValue,
  resolveProvider,
  type WorkerProviderLike,
} from "./worker-harness.ts";

const providers: WorkerProviderLike[] = [
  {
    name: "OpenCode Go",
    kind: "harness_managed",
    config: { harness: "opencode", provider: "opencode-go", settings: { models: ["deepseek-v4-pro"] } },
  },
  {
    name: "Pi Go",
    kind: "harness_managed",
    config: { harness: "pi", provider: "pi-go", settings: { models: ["gpt-5"] } },
  },
  { name: "Gateway", kind: "openai_compatible", config: { model: "gpt-5", baseUrl: "https://gw.example/v1" } },
];

test("provider options and model catalogues come from the provider config", () => {
  assert.equal(providerOptionValue(providers[0]), "opencode-go");
  assert.equal(providerOptionValue(providers[2]), "Gateway");
  assert.equal(providerHarness(providers[0]), "opencode");
  assert.equal(providerHarness(providers[2]), null, "ORBIT-managed providers have no harness of their own");
  assert.deepEqual(resolveProvider(providers, "pi-go").providerModels, ["gpt-5"]);
});

test("a mismatched harness and provider name both harnesses and block the save", () => {
  const mismatch = harnessMismatch("pi", "opencode-go", providers);
  assert.equal(mismatch?.workerHarness, "pi");
  assert.equal(mismatch?.providerHarness, "opencode");
  assert.equal(mismatch?.title, "Pi worker with an OpenCode-harness model");
  assert.match(
    mismatch?.message ?? "",
    /Pi worker with an OpenCode-harness model\. Switch the worker to OpenCode or choose a Pi-harness provider before saving\./,
  );
});

test("a matching harness, another provider kind or no provider is not a mismatch", () => {
  assert.equal(harnessMismatch("opencode", "opencode-go", providers), null);
  assert.equal(harnessMismatch("pi", "Gateway", providers), null);
  assert.equal(harnessMismatch("pi", "", providers), null);
  assert.equal(harnessMismatch("pi", "custom-provider", providers), null);
});

test("unknown harnesses get a readable label", () => {
  assert.equal(harnessLabel("pi"), "Pi");
  assert.equal(harnessLabel("opencode"), "OpenCode");
  assert.equal(harnessLabel("my_harness"), "My Harness");
});
