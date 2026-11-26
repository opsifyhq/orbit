import assert from "node:assert/strict";
import { test } from "node:test";

import { kubernetesAuthMode, kubernetesCredentialIssue } from "./kubernetes-credential.ts";

test("the auth mode defaults to service account", () => {
  assert.equal(kubernetesAuthMode({}), "service_account");
  assert.equal(kubernetesAuthMode(null), "service_account");
  assert.equal(kubernetesAuthMode({ authMode: "kubeconfig" }), "kubeconfig");
  assert.equal(kubernetesAuthMode({ authMode: "other" }), "service_account");
});

test("an edit that changes the auth mode needs a new credential (#400)", () => {
  const base = { editing: true, storedMode: "kubeconfig", mode: "service_account", credential: "" } as const;
  assert.match(kubernetesCredentialIssue(base) ?? "", /service account: enter its token/);
  assert.match(
    kubernetesCredentialIssue({ ...base, storedMode: "service_account", mode: "kubeconfig" }) ?? "",
    /kubeconfig: load or paste/,
  );
  assert.equal(kubernetesCredentialIssue({ ...base, credential: "token" }), null);
  assert.equal(kubernetesCredentialIssue({ ...base, credential: "   " }) !== null, true, "whitespace is no credential");
  assert.equal(kubernetesCredentialIssue({ ...base, mode: "kubeconfig" }), null, "same mode keeps the stored one");
  assert.equal(kubernetesCredentialIssue({ ...base, editing: false }), null, "a new connection has nothing stored");
});
