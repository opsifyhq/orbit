/**
 * Client-side connector rule for Agent Studio (#260), matching the domain rule: a
 * connector binding is either absent, or enabled with a concrete connection. The API
 * validates the same thing and returns `spec.connectors[i].connectionId` on failure.
 * This helper only adds the field-level error; shape validation stays in the shared Zod
 * contract.
 */
export type ConnectorBindingValue = {
  connector: string;
  connectionId?: string | null;
  capabilities?: string[];
};

export type ConnectorFieldIssue = { path: string; message: string };

/** A blank picker value counts as missing, whether it is null, undefined or "". */
export function connectorIssues(
  bindings: readonly ConnectorBindingValue[],
): ConnectorFieldIssue[] {
  const issues: ConnectorFieldIssue[] = [];
  bindings.forEach((binding, index) => {
    if (!binding.connectionId?.trim()) {
      issues.push({
        path: `spec.connectors[${index}].connectionId`,
        message: "Choose a connection for this connector",
      });
    }
  });
  return issues;
}

/** Zod uses dot-separated indices; the API envelope uses `field[0].name`. */
export function formatValidationPath(path: readonly PropertyKey[]): string {
  return path.reduce<string>((result, segment) => {
    if (typeof segment === "number") return `${result}[${segment}]`;
    const key = typeof segment === "symbol" ? (segment.description ?? "") : segment;
    return result ? `${result}.${key}` : key;
  }, "");
}
