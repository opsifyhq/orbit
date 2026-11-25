/** Kubernetes capabilities map to the resources the connection may touch. */
const KUBERNETES_RESOURCES: Record<string, string[]> = {
  "namespaces.list": ["namespaces"],
  "pods.list": ["pods"],
  "pods.get": ["pods"],
  "pods.logs": ["pods/log"],
  "pods.delete": ["pods"],
  "deployments.list": ["deployments"],
  "deployments.get": ["deployments"],
  "deployments.restart": ["deployments"],
  "events.list": ["events"],
};

/**
 * Resource-level allowlist for a Kubernetes connection. The connector still checks
 * agent-level capability scopes; this list prevents a connection from ever touching a
 * resource class it was not set up for. Advanced config can override it.
 */
export function kubernetesResources(scopes: string[]): string[] {
  return [...new Set(scopes.flatMap((scope) => KUBERNETES_RESOURCES[scope] ?? []))].sort();
}

function sameSet(a: string[], b: string[]): boolean {
  const left = [...new Set(a)].sort();
  const right = [...new Set(b)].sort();
  return left.length === right.length && left.every((value, index) => value === right[index]);
}

/**
 * The `allowedResources` to save. The list follows the selected capabilities unless it is
 * a deliberate override: a non-empty stored list that differs from what the capabilities
 * the connection was loaded with would derive. A list that was itself derived (the usual
 * case after the first save) is derived again, so adding `pods.logs` adds `pods/log` and
 * removing capabilities shrinks the list.
 */
export function allowedKubernetesResources(
  stored: unknown,
  scopes: string[],
  previousScopes: string[],
): string[] {
  const derived = kubernetesResources(scopes);
  if (!Array.isArray(stored) || stored.length === 0) return derived;
  const list = stored.filter((value): value is string => typeof value === "string");
  if (sameSet(list, kubernetesResources(previousScopes))) return derived;
  return list;
}
