import assert from "node:assert/strict";
import { test } from "node:test";

import { connectorIssues, formatValidationPath } from "./connector-validation.ts";

test("an enabled connector with no connection is invalid on its own row", () => {
  assert.deepEqual(
    connectorIssues([
      { connector: "github", connectionId: null, capabilities: ["issues.list"] },
      { connector: "telegram", capabilities: [] },
    ]),
    [
      {
        path: "spec.connectors[0].connectionId",
        message: "Choose a connection for this connector",
      },
      {
        path: "spec.connectors[1].connectionId",
        message: "Choose a connection for this connector",
      },
    ],
  );
});

test("a blank connection id is invalid too", () => {
  assert.deepEqual(
    connectorIssues([{ connector: "github", connectionId: "   " }]).map((issue) => issue.path),
    ["spec.connectors[0].connectionId"],
  );
});

test("disabled and empty connector lists stay valid", () => {
  assert.deepEqual(connectorIssues([]), []);
  assert.deepEqual(connectorIssues([{ connector: "github", connectionId: "conn-1" }]), []);
});

test("validation paths match the API envelope", () => {
  assert.equal(
    formatValidationPath(["spec", "connectors", 1, "connectionId"]),
    "spec.connectors[1].connectionId",
  );
  assert.equal(formatValidationPath(["metadata", "name"]), "metadata.name");
});
