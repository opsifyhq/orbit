import type { AuditLogPage, AuditLogRecord } from "./types";

/** Entries per audit log page in the web app. */
export const AUDIT_PAGE_SIZE = 25;

/**
 * Action families the audit log can be filtered by. The value is the part of the action
 * before the first dot (`agent.deleted` → `agent`); the API matches it exactly.
 */
export const AUDIT_AREAS: ReadonlyArray<{ value: string; label: string }> = [
  { value: "agent", label: "Agents" },
  { value: "team", label: "Agent teams" },
  { value: "team_run", label: "Team runs" },
  { value: "team_queue", label: "Team queue" },
  { value: "execution", label: "Executions" },
  { value: "approval", label: "Approvals" },
  { value: "connection", label: "Connections" },
  { value: "provider", label: "Model providers" },
  { value: "skill", label: "Skills" },
  { value: "trigger", label: "Triggers" },
  { value: "output", label: "Outputs" },
  { value: "env_mapping", label: "Environment mappings" },
  { value: "tool_policy", label: "Tool policies" },
  { value: "worker", label: "Workers" },
  { value: "worker_profile", label: "Worker profiles" },
  { value: "worker_token", label: "Worker tokens" },
  { value: "api_token", label: "API tokens" },
  { value: "role", label: "Roles" },
  { value: "user", label: "Users" },
  { value: "settings", label: "Settings" },
  { value: "resource", label: "Sharing" },
  { value: "import", label: "Imports" },
  { value: "license", label: "Licence" },
];

/** The cursor for the page after `page`, or undefined on the last page. */
export function auditNextCursor(page: Pick<AuditLogPage, "nextCursor">): number | undefined {
  if (!page.nextCursor) return undefined;
  const id = Number(page.nextCursor);
  return Number.isSafeInteger(id) && id > 0 ? id : undefined;
}

function metadataText(metadata: unknown, key: string): string | undefined {
  if (!metadata || typeof metadata !== "object" || Array.isArray(metadata)) return undefined;
  const value = (metadata as Record<string, unknown>)[key];
  return typeof value === "string" && value.trim() ? value.trim() : undefined;
}

/**
 * What an audit entry changed, for its row: the name recorded with the entry where there is
 * one, otherwise the short form of the target id. Undefined when the entry has no target.
 */
export function auditTargetLabel(
  entry: Pick<AuditLogRecord, "targetId" | "metadata">,
): string | undefined {
  const named =
    metadataText(entry.metadata, "name") ??
    metadataText(entry.metadata, "slug") ??
    metadataText(entry.metadata, "email");
  if (named) return named;
  if (entry.targetId) return entry.targetId.slice(0, 8);
  return undefined;
}

/** Metadata worth showing in the detail row, or undefined when it is empty. */
export function auditMetadataText(metadata: unknown): string | undefined {
  if (metadata === null || metadata === undefined) return undefined;
  if (typeof metadata === "object" && !Array.isArray(metadata) && !Object.keys(metadata).length) {
    return undefined;
  }
  return JSON.stringify(metadata, null, 2);
}
