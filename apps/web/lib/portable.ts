"use client";

import { ApiError } from "./api-error";
import { requestRaw } from "./http";

/*
 * Portable export/import (#80).
 *
 * These types mirror `orbit_contracts::portable` and are written by hand because the
 * generated client does not know the new endpoint yet; replace them with the generated
 * types after `pnpm api:generate` has run against a build that includes /api/v1/import.
 *
 * Requests deliberately use relative URLs so they go through the same-origin web proxy
 * (`NEXT_PUBLIC_ORBIT_API_URL` may point at an API host the browser cannot reach).
 */

export type ImportMode = "create" | "update" | "upsert";
export type ImportAction = "create" | "update";
export type ImportItemStatus = "ok" | "conflict" | "invalid" | "unresolved_references";
export type ExportFormat = "yaml" | "markdown" | "json";

export interface UnresolvedReference {
  kind: string;
  reference: string;
  path: string;
}

export interface ImportIssue {
  path: string;
  message: string;
}

export interface ImportItemReport {
  kind: string;
  name: string;
  status: ImportItemStatus;
  action: ImportAction | null;
  id: string | null;
  unresolvedReferences: UnresolvedReference[];
  issues: ImportIssue[];
  warnings: string[];
  credentialsRequired: boolean;
}

export interface ImportReport {
  dryRun: boolean;
  mode: ImportMode;
  valid: boolean;
  applied: boolean;
  items: ImportItemReport[];
}

/** An error envelope response, with the per-field details the API returns. */
export class PortableError extends Error {
  status: number;
  code: string;
  details: ImportIssue[];

  constructor(status: number, code: string, message: string, details: ImportIssue[]) {
    super(message);
    this.status = status;
    this.code = code;
    this.details = details;
  }
}

/** Keep the import dialog's error shape while using the shared auth and 401 handling. */
function toError(error: unknown): PortableError {
  if (error instanceof ApiError) {
    return new PortableError(
      error.status,
      error.code ?? "error",
      error.message,
      (error.details ?? []) as ImportIssue[],
    );
  }
  return new PortableError(0, "error", error instanceof Error ? error.message : "Request failed", []);
}

/** Same-origin requests so a server-only `NEXT_PUBLIC_ORBIT_API_URL` still works in the browser. */
function requestPortable(path: string, init?: RequestInit): Promise<Response> {
  return requestRaw(path, { ...init, sameOrigin: true });
}

/** Validate (dryRun) or apply an import of YAML, JSON, SKILL.md or a bundle. */
export async function runImport(
  content: string,
  filename: string,
  mode: ImportMode,
  dryRun: boolean,
): Promise<ImportReport> {
  const params = new URLSearchParams({ mode, dryRun: String(dryRun) });
  try {
    const response = await requestPortable(`/api/v1/import?${params}`, {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify({ content, filename }),
    });
    return (await response.json()) as ImportReport;
  } catch (error) {
    throw toError(error);
  }
}

function filenameFrom(response: Response, fallback: string): string {
  const header = response.headers.get("content-disposition") ?? "";
  const match = /filename="?([^";]+)"?/i.exec(header);
  return match?.[1] ?? fallback;
}

/** Download a secret-free export through the same-origin proxy. */
export async function downloadExport(
  resource: string,
  resourceId: string,
  name: string,
  format: ExportFormat = "yaml",
): Promise<void> {
  let response: Response;
  try {
    response = await requestPortable(`/api/v1/export/${resource}/${resourceId}?format=${format}`);
  } catch (error) {
    throw toError(error);
  }
  const safe = name.replace(/[^a-z0-9._-]+/gi, "-").toLowerCase() || "export";
  const extension = format === "markdown" ? "md" : format;
  const filename = filenameFrom(response, `${safe}.${extension}`);
  const url = URL.createObjectURL(await response.blob());
  const anchor = document.createElement("a");
  anchor.href = url;
  anchor.download = filename;
  anchor.click();
  URL.revokeObjectURL(url);
}
