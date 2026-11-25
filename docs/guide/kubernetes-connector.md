# Kubernetes connector

Cluster inspection and controlled recovery for agents: list
namespaces/pods/deployments/events, read pod details and bounded logs, restart a deployment
or delete a pod. No arbitrary apply, exec, port-forward or Secret access.

## Connection

Add a **Kubernetes** connection under **Connectors → New connection** (see
[Connectors](connections.md)). The connection form has an **Authentication** dropdown:

- `service_account` (preferred): paste a reader ServiceAccount token as the credential and
  set `apiServer` (in-cluster: `https://kubernetes.default.svc`).
- `kubeconfig`: upload or paste a kubeconfig as the credential. ORBIT uses the
  selected/current context and supports static `token` or `client-certificate-data` +
  `client-key-data` auth. It **rejects** `exec` auth plugins and
  `insecure-skip-tls-verify`. Set `context` to choose a context other than the current one.

The namespaces field is comma-separated in the form and stored as an array. The
connection also stores `defaultNamespace`, an optional public `caCert`,
`timeoutSeconds` (default 20) and `maxLogBytes` (default 200000). Allowlists are enforced
by ORBIT as well as RBAC, and an empty list denies:

- `allowedResources` is normally derived from the selected capabilities, and the web form
  derives it again on every save, so adding `pods.logs` later adds `pods/log` and removing
  capabilities shrinks the list. A non-empty value in Advanced config that differs from what
  the saved capabilities derive is an explicit override and is kept. The form shows the
  list that will be saved.

- `namespaces` empty: only `defaultNamespace` may be used; with neither set, every
  namespaced tool is refused. Use `["*"]` to let the kubeconfig/RBAC decide which
  namespaces are reachable. `namespaces.list` returns only allowed namespaces unless the
  wildcard is set.
- `allowedResources` empty: every tool is refused.

For example:

```json
{
  "authMode": "service_account",
  "apiServer": "https://kubernetes.default.svc",
  "namespaces": ["payments", "payments-staging"],
  "defaultNamespace": "payments",
  "allowedResources": ["pods", "pods/log", "events", "deployments"]
}
```

> **Changed in the release after 0.27.0.** Empty `namespaces` and `allowedResources`
> lists used to allow everything the credential could reach. Connections that relied on
> that now get "not allowed on this connection" errors; add the namespaces and resources
> the agents need to the connection.

Pod and deployment results have container `env[].value` and the
`kubectl.kubernetes.io/last-applied-configuration` annotation replaced with `[redacted]`,
so plain environment values in a manifest do not reach the agent. `valueFrom` references
(Secret or ConfigMap name and key) are kept.

Object names must be Kubernetes names (lowercase letters, digits, `-` and `.`, starting and
ending with a letter or digit) and namespaces must be DNS labels, so `.`, `..` and empty
values are refused before any request. `defaultNamespace` and every `namespaces` entry
(other than a standalone `*`) are checked the same way when the connection is saved.

## Test connection

The connection **Test** button parses the credential and connects to the API server. It
first lists the namespaces visible to the credential; if that returns 403 it probes a
bounded pod list in the configured/default namespace. A kubeconfig scoped to one namespace
therefore tests successfully without cluster-wide `list namespaces` RBAC. Unsupported
kubeconfig auth, rejected TLS settings and RBAC failures return a readable message. Only
a Kubernetes `Status` message is passed on; any other response body is reported by its
status code alone, so a misdirected `apiServer` cannot be read back through the test. The
result is stored as the connection's check status. The check is advisory: a failed test marks the connection
"Check failed" (status `error`), but runs do not check that status, so agents keep using
the connection, and testing again after a fix sets it back to connected. A test never
overwrites an `expired` or `revoked` status.

The same parser and client are shared by the API (test) and worker (tool calls), so a
connection that tests successfully behaves the same during an execution.

## Capabilities

`namespaces.list`, `pods.list`, `pods.get`, `pods.logs` (tail/previous, capped),
`deployments.list`, `deployments.get`, `events.list`.

Recovery capabilities are explicit external side effects:

- `deployments.restart`: patches `kubectl.kubernetes.io/restartedAt` on the pod template,
  which triggers a normal rolling restart.
- `pods.delete`: deletes one pod so its Deployment/ReplicaSet recreates it. Any pod in an
  allowed namespace can be deleted; ORBIT does not check that the pod is stuck, so limit
  it with approvals and RBAC. `gracePeriodSeconds` is optional (0–300); without it the
  pod's own `terminationGracePeriodSeconds` applies, as with `kubectl delete pod`, and `0`
  force-deletes. The result is `{deleted, namespace, name}`, not the deleted Pod.

They follow autonomy and approvals like any other write: L3 asks, L4 with `auto`
approval runs immediately, and tool policies can deny them. The connection-level
`allowedResources` allowlist still applies; the UI derives it from the selected
capabilities.

## RBAC example

Read-only: `deploy/examples/kubernetes-reader-rbac.yaml`.

Recovery (delete pods and patch deployments in one namespace):
`deploy/examples/kubernetes-recovery-rbac.yaml`.

Do not grant Secrets, pods/exec, port-forward, arbitrary create/patch or cluster-wide access.

## Verification

`crates/orbit-kubernetes/tests/live.rs` has opt-in tests for a real cluster:

```bash
ORBIT_TEST_KUBECONFIG="$(kubectl config view --raw --minify --context kind-orbit)" \
  ORBIT_TEST_KUBECTL_CONTEXT=kind-orbit \
  cargo test -p orbit-kubernetes --test live -- --ignored
```

The second test creates a failing pod in a temporary namespace and verifies pods, warning
events and pod logs through the same client the worker uses. Verified on kind with
client-certificate kubeconfig auth.
