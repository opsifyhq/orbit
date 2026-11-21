import type { EnvMappingEntry, EnvMappingRecord } from "./env-mappings.ts";

/**
 * Pure helpers for environments (env mappings): the editable rows, their validation and
 * the status of a stored environment. No runtime imports, so `node --test` can load it.
 */

export type Source = "connection" | "secret";

export interface VariableRow {
  envVar: string;
  source: Source;
  connectionId: string;
  secretKey: string;
  /** Carried through untouched so editing never drops a field the form does not show. */
  secretRef?: string | null;
  /** True once the name was typed by hand, so suggestions stop overwriting it. */
  named: boolean;
}

export const emptyRow = (): VariableRow => ({
  envVar: "",
  source: "connection",
  connectionId: "",
  secretKey: "",
  named: false,
});

/** A name a shell and a process environment will both accept. */
export const ENV_NAME = /^[A-Za-z_][A-Za-z0-9_]*$/;

/** "github" → "GITHUB_TOKEN": a sensible starting name for a connection's credential. */
export function suggestName(connectorSlug: string): string {
  return `${connectorSlug.replace(/[^A-Za-z0-9]+/g, "_").toUpperCase()}_TOKEN`;
}

export function toRows(mapping: Pick<EnvMappingRecord, "mappings">): VariableRow[] {
  return mapping.mappings.map((entry) => ({
    envVar: entry.envVar,
    source: entry.connectionId ? "connection" : "secret",
    connectionId: entry.connectionId ?? "",
    secretKey: entry.secretKey ?? "",
    secretRef: entry.secretRef,
    named: true,
  }));
}

/**
 * The entries a set of rows saves. Empty rows are dropped. A variable has one source:
 * whatever the other source held is not sent, so a stale secret never rides along.
 */
export function toEntries(rows: VariableRow[]): EnvMappingEntry[] {
  return rows
    .filter((row) => row.envVar.trim())
    .map((row) =>
      row.source === "connection"
        ? { envVar: row.envVar.trim(), connectionId: row.connectionId || null, secretKey: null }
        : {
            envVar: row.envVar.trim(),
            connectionId: null,
            secretKey: row.secretKey.trim() || null,
            ...(row.secretRef ? { secretRef: row.secretRef } : {}),
          },
    );
}

/** Why one row's name is wrong, or undefined. `names` are all filled names, in order. */
export function nameError(row: VariableRow, names: string[]): string | undefined {
  const name = row.envVar.trim();
  if (!name) return undefined;
  if (!ENV_NAME.test(name)) return "Letters, digits and underscores only, not starting with a digit";
  if (names.indexOf(name) !== names.lastIndexOf(name)) return "This name is used twice";
  return undefined;
}

export type SectionId = "details" | "variables" | "sharing";

/** Problems that stop a draft from saving, by form section. */
export function draftIssues(name: string, rows: VariableRow[]): Array<{ section: SectionId; message: string }> {
  const issues: Array<{ section: SectionId; message: string }> = [];
  if (!name.trim()) issues.push({ section: "details", message: "Give the environment a name." });
  const filled = rows.filter((row) => row.envVar.trim());
  const names = filled.map((row) => row.envVar.trim());
  if (!filled.length) issues.push({ section: "variables", message: "Add at least one variable." });
  if (filled.some((row) => !ENV_NAME.test(row.envVar.trim()))) {
    issues.push({ section: "variables", message: "Fix the variable names marked in red." });
  }
  const duplicate = names.find((value, index) => names.indexOf(value) !== index);
  if (duplicate) issues.push({ section: "variables", message: `${duplicate} appears twice.` });
  for (const row of filled) {
    if (row.source === "connection" ? !row.connectionId : !row.secretKey.trim() && !row.secretRef) {
      issues.push({ section: "variables", message: `Choose where ${row.envVar.trim()} comes from.` });
    }
  }
  return issues;
}

export interface ConnectionLike {
  id: string;
  name: string;
  connectorSlug: string;
  hasCredential: boolean;
}

/** Variables whose value would be empty: their connection is gone or has no credential. */
export function brokenVariables(
  mapping: Pick<EnvMappingRecord, "mappings">,
  connections: Map<string, ConnectionLike> | null,
): Array<{ envVar: string; reason: "missing" | "no_credential" }> {
  if (!connections) return [];
  const broken: Array<{ envVar: string; reason: "missing" | "no_credential" }> = [];
  for (const entry of mapping.mappings) {
    if (!entry.connectionId) continue;
    const connection = connections.get(entry.connectionId);
    if (!connection) broken.push({ envVar: entry.envVar, reason: "missing" });
    else if (!connection.hasCredential) broken.push({ envVar: entry.envVar, reason: "no_credential" });
  }
  return broken;
}

export type EnvState = "in_use" | "unused" | "attention";

/** A broken variable needs attention whether or not a worker uses it yet. */
export function envState(broken: number, users: number): EnvState {
  if (broken) return "attention";
  return users ? "in_use" : "unused";
}

export const ENV_STATUS: Record<
  EnvState,
  { label: string; tone: "success" | "neutral" | "warning"; shape: "solid" | "dashed" | "alert" }
> = {
  in_use: { label: "In use", tone: "success", shape: "solid" },
  unused: { label: "Unused", tone: "neutral", shape: "dashed" },
  attention: { label: "Needs attention", tone: "warning", shape: "alert" },
};

/** "GITLAB_TOKEN, SLACK_URL and 2 more". */
export function variableSummary(mapping: Pick<EnvMappingRecord, "mappings">, max = 2): string {
  const names = mapping.mappings.map((entry) => entry.envVar);
  if (!names.length) return "No variables";
  if (names.length <= max + 1) return names.join(", ");
  return `${names.slice(0, max).join(", ")} and ${names.length - max} more`;
}
