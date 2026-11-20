import assert from "node:assert/strict";
import { test } from "node:test";

import { setConfigField } from "./connection-config.ts";

test("sets only the changed key and coerces by schema type", () => {
  const text = JSON.stringify({ url: "https://example.com", retries: 1, enabled: false }, null, 2);
  const number = setConfigField(text, "retries", "5", { type: "integer" });
  assert.equal(number.error, undefined);
  assert.deepEqual(JSON.parse(number.configText), {
    url: "https://example.com",
    retries: 5,
    enabled: false,
  });

  const boolean = setConfigField(number.configText, "enabled", "true", { type: "boolean" });
  assert.deepEqual(JSON.parse(boolean.configText), {
    url: "https://example.com",
    retries: 5,
    enabled: true,
  });

  const object = setConfigField(boolean.configText, "options", '{"timeout":30}', { type: "object" });
  assert.deepEqual(JSON.parse(object.configText), {
    url: "https://example.com",
    retries: 5,
    enabled: true,
    options: { timeout: 30 },
  });
});

test("invalid raw JSON leaves the config text untouched and reports an error", () => {
  const text = '{ "url": "https://example.com", "broken": ';
  const result = setConfigField(text, "token", "abc");
  assert.equal(result.configText, text);
  assert.match(result.error ?? "", /valid JSON/);
});

test("a non-object config is rejected instead of being replaced", () => {
  const result = setConfigField("[1, 2]", "url", "https://example.com");
  assert.equal(result.configText, "[1, 2]");
  assert.match(result.error ?? "", /JSON object/);
});

test("clearing a field removes only that key", () => {
  const result = setConfigField('{"a":1,"b":2}', "a", "");
  assert.deepEqual(JSON.parse(result.configText), { b: 2 });
});
