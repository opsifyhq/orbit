// Which stop / archive / delete controls the web app offers. The API enforces the same
// rules; these only keep the UI from offering an action the server will refuse.

/** A team run can be stopped while it is running, by a caller who may cancel executions. */
export function canStopTeamRun(status: string, permissions: readonly string[] | undefined): boolean {
  return status === "running" && Boolean(permissions?.includes("executions:cancel"));
}

const ENDED_LAUNCH = new Set(["stopped", "failed"]);

export type LaunchView = "active" | "history" | "archived";

/** Archive and delete exist only for ended launches and need settings:write. */
export function launchEndActions(
  launch: { status: string; archivedAt?: string | null },
  permissions: readonly string[] | undefined,
): { archive: boolean; remove: boolean } {
  const allowed = ENDED_LAUNCH.has(launch.status) && Boolean(permissions?.includes("settings:write"));
  return { archive: allowed && !launch.archivedAt, remove: allowed };
}

/**
 * "Active" comes from the server already filtered. "History" and "Archived" share the
 * full list (the server returns archived launches too) and split it on `archivedAt`.
 */
export function launchesForView<T extends { archivedAt?: string | null }>(launches: T[], view: LaunchView): T[] {
  if (view === "archived") return launches.filter((launch) => launch.archivedAt);
  if (view === "history") return launches.filter((launch) => !launch.archivedAt);
  return launches;
}
