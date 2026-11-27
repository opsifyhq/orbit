import { brandName } from "@/lib/presentation";

import { targetLabel } from "./shared";
import { nativeToolsError, type WorkerFormValues } from "./worker-form";

export type SectionId =
  | "runtime"
  | "model"
  | "scope"
  | "permissions"
  | "environment"
  | "resources"
  | "sharing";

export const SECTION_ORDER: SectionId[] = [
  "runtime",
  "model",
  "scope",
  "permissions",
  "environment",
  "resources",
  "sharing",
];

export const SECTION_TITLES: Record<SectionId, string> = {
  runtime: "Runtime",
  model: "Harness auth",
  scope: "Agents and skills",
  permissions: "Permissions",
  environment: "Environment",
  resources: "Resources",
  sharing: "Access control",
};

export const SECTION_DESCRIPTIONS: Record<SectionId, string> = {
  runtime: "What the worker is, where it runs and which image it starts from.",
  model: "The secret the harness signs in with. Provider and model are chosen on each agent or team.",
  scope: "Which agents this worker may claim, and the skills it prepares at startup.",
  permissions: "Which tools the worker may use. Everything is off until you allow it.",
  environment: "Credentials the worker fetches at startup and injects into its process and every script it spawns.",
  resources: "Requests and limits for the worker container.",
  sharing: "Who can see and use this profile. Saved together with the profile.",
};

export function isSectionId(value: string): value is SectionId {
  return (SECTION_ORDER as string[]).includes(value);
}

export type EditorIssue = { section: SectionId; message: string };

/** Problems that stop a save. Everything else the API accepts is left to the API. */
export function validateWorkerForm(values: WorkerFormValues): EditorIssue[] {
  const issues: EditorIssue[] = [];
  if (!values.name.trim()) issues.push({ section: "runtime", message: "Give the profile a name." });
  if (!values.harness) issues.push({ section: "runtime", message: "Choose a harness." });
  if (nativeToolsError(values.nativeToolsText)) {
    issues.push({ section: "permissions", message: "Native tool permissions are not valid JSON." });
  }
  return issues;
}

function countJsonKeys(text: string): number {
  try {
    const parsed: unknown = JSON.parse(text || "{}");
    return parsed && typeof parsed === "object" ? Object.keys(parsed).length : 0;
  } catch {
    return 0;
  }
}

/** What each section currently holds. The section nav doubles as the summary. */
export function sectionSummaries(
  values: WorkerFormValues,
  isEditing: boolean,
): Record<SectionId, { value: string; missing?: boolean }> {
  const allowed = [
    values.allowWebSearch ? "web search" : null,
    values.allowWebFetch ? "web fetch" : null,
    values.allowGlab ? "glab" : null,
    values.allowGh ? "gh" : null,
  ].filter((item): item is string => Boolean(item));
  const rules = countJsonKeys(values.nativeToolsText);
  const permissionParts = [
    allowed.length ? `Allows ${allowed.join(", ")}` : null,
    rules ? `${rules} tool ${rules === 1 ? "rule" : "rules"}` : null,
  ].filter(Boolean);
  const inline = values.envMappings.length;
  const reusable = values.envMappingIds.length;
  const selectedNone = values.agentScope === "selected" && values.agentIds.length === 0;

  return {
    runtime: {
      value: values.name.trim()
        ? [
            values.harness ? brandName(values.harness) : "Harness not set",
            targetLabel(values.target),
            values.tag ? `tag ${values.tag}` : "follows platform",
          ].join(" · ")
        : "Name not set",
      missing: !values.name.trim() || !values.harness,
    },
    model: {
      value: values.authSecret || "No harness auth secret",
    },
    scope: {
      value: [
        values.agentScope === "all"
          ? "All agents"
          : `${values.agentIds.length} ${values.agentIds.length === 1 ? "agent" : "agents"}`,
        `${values.skillSlugs.length} ${values.skillSlugs.length === 1 ? "skill" : "skills"}`,
      ].join(" · "),
      missing: selectedNone,
    },
    permissions: { value: permissionParts.length ? permissionParts.join(" · ") : "Nothing extra allowed" },
    environment: {
      value:
        inline || reusable
          ? [inline ? `${inline} inline` : null, reusable ? `${reusable} reusable` : null]
              .filter(Boolean)
              .join(" · ")
          : "None",
    },
    resources: {
      value: `${values.cpuLimit || "no"} CPU · ${values.memoryLimit || "no"} memory limit`,
    },
    sharing: { value: isEditing ? "Who can see and use it" : "After the first save" },
  };
}
