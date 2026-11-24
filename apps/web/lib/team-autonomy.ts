/**
 * Pure helpers for the team form's fixed autonomy policy (#385). Kept out of the React
 * component so `node --test` can check that the form's defaults never ask the platform to
 * run an agent less strictly than its own policy.
 */
export type TeamApprovalChoice = "agent" | "auto" | "always" | "writes";

export type TeamAutonomyValue = {
  mode?: "follow_primary" | "fixed";
  level?: number | null;
  approvalMode?: "auto" | "always" | "writes" | null;
};

/** Level 3 (Prepare) asks a human before any write; it never removes an approval. */
export const DEFAULT_FIXED_LEVEL = "3";
/** Keep each step agent's own approval mode unless the author picks one. */
export const DEFAULT_FIXED_APPROVAL: TeamApprovalChoice = "agent";

export function autonomyDraft(autonomy?: TeamAutonomyValue | null): {
  mode: "follow_primary" | "fixed";
  level: string;
  approval: TeamApprovalChoice;
} {
  if (autonomy?.mode !== "fixed") {
    return { mode: "follow_primary", level: DEFAULT_FIXED_LEVEL, approval: DEFAULT_FIXED_APPROVAL };
  }
  return {
    mode: "fixed",
    level: String(autonomy.level ?? DEFAULT_FIXED_LEVEL),
    approval: autonomy.approvalMode ?? "agent",
  };
}

/** The API body for the form's autonomy fields. `agent` leaves `approvalMode` out. */
export function teamAutonomyInput(
  mode: "follow_primary" | "fixed",
  level: string,
  approval: TeamApprovalChoice,
): { mode: "fixed"; level: number; approvalMode?: "auto" | "always" | "writes" } | null {
  if (mode !== "fixed") return null;
  return approval === "agent"
    ? { mode: "fixed", level: Number(level) }
    : { mode: "fixed", level: Number(level), approvalMode: approval };
}
