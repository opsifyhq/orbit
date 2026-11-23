# Kubernetes connector

Read-only cluster inspection for agents: list namespaces/pods/deployments/events, read pod
details and bounded logs. No writes, exec, port-forward or Secret access.

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

- `allowedResources` is normally derived from the selected capabilities; an explicit
  non-empty value in Advanced config overrides that derivation.

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

## Test connection

The connection **Test** button parses the credential and connects to the API server. It
first lists the namespaces visible to the credential; if that returns 403 it probes a
bounded pod list in the configured/default namespace. A kubeconfig scoped to one namespace
therefore tests successfully without cluster-wide `list namespaces` RBAC. Unsupported
kubeconfig auth, rejected TLS settings and RBAC failures return a readable message; the
result is stored as the connection's check status. The check is advisory: a failed test marks the connection
"Check failed" (status `error`), but runs do not check that status, so agents keep using
the connection, and testing again after a fix sets it back to connected. A test never
overwrites an `expired` or `revoked` status.

The same parser and client are shared by the API (test) and worker (tool calls), so a
connection that tests successfully behaves the same during an execution.

## Capabilities

`namespaces.list`, `pods.list`, `pods.get`, `pods.logs` (tail/previous, capped),
`deployments.list`, `deployments.get`, `events.list`.

## RBAC example

See `deploy/examples/kubernetes-reader-rbac.yaml` in the repository.

Do not grant Secrets, pods/exec, port-forward, create/delete or cluster-wide access.

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
