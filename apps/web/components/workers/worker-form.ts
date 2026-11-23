import { useState } from "react";

import type { WorkerProfile } from "@/lib/types";

export type InlineMapping = { envVar: string; connectionId: string; secretKey: string };

export type WorkerFormValues = {
  name: string;
  harness: string;
  target: string;
  driver: string;
  image: string;
  tag: string;
  provider: string;
  model: string;
  authSecret: string;
  envMappings: InlineMapping[];
  envMappingIds: string[];
  agentScope: "all" | "selected";
  agentIds: string[];
  skillSlugs: string[];
  cpuRequest: string;
  memoryRequest: string;
  cpuLimit: string;
  memoryLimit: string;
  workspaceSize: string;
  allowWebSearch: boolean;
  allowWebFetch: boolean;
  allowGlab: boolean;
  allowGh: boolean;
  nativeToolsText: string;
};

export type WorkerFormApi = {
  values: WorkerFormValues;
  /** Merge a patch into the form. */
  set: (patch: Partial<WorkerFormValues>) => void;
};

type Resources = {
  requests?: { cpu?: string; memory?: string };
  limits?: { cpu?: string; memory?: string };
  workspace?: { sizeLimit?: string };
};

/**
 * Harness-neutral starting point. Image and tag stay empty so the API applies the
 * installation's registry and version; provider and model come from the catalog, never
 * from a hardcoded default.
 */
export const DEFAULT_WORKER_FORM: WorkerFormValues = {
  name: "New worker profile",
  harness: "",
  target: "docker",
  driver: "process",
  image: "",
  tag: "",
  provider: "",
  model: "",
  authSecret: "",
  envMappings: [],
  envMappingIds: [],
  agentScope: "all",
  agentIds: [],
  skillSlugs: [],
  cpuRequest: "100m",
  memoryRequest: "256Mi",
  cpuLimit: "1",
  memoryLimit: "1Gi",
  workspaceSize: "1Gi",
  allowWebSearch: false,
  allowWebFetch: false,
  allowGlab: false,
  allowGh: false,
  nativeToolsText: "{}",
};

/** Form values for an existing profile. */
export function valuesFromProfile(profile: WorkerProfile): WorkerFormValues {
  // Older installs stored `{}` here instead of a list; treat anything but a list as empty.
  const rawMappings: unknown = (profile as { envMappings?: unknown }).envMappings;
  const mappings = Array.isArray(rawMappings)
    ? (rawMappings as Array<{ envVar?: string; connectionId?: string | null; secretKey?: string | null }>)
    : [];
  const scoped = (profile as WorkerProfile & { agentScope?: string }).agentScope === "selected";
  const resources = ((profile as WorkerProfile & { resources?: Record<string, unknown> }).resources ??
    {}) as Resources;
  return {
    name: profile.name,
    harness: profile.harnessKind,
    target: profile.target,
    driver: profile.executionDriver,
    image: profile.image,
    tag: profile.imageTag,
    provider: profile.provider ?? "",
    model: profile.model ?? "",
    authSecret: profile.authSecretName ?? "",
    envMappings: mappings.map((m) => ({
      envVar: m.envVar ?? "",
      connectionId: m.connectionId ?? "",
      secretKey: m.secretKey ?? "",
    })),
    envMappingIds: [],
    agentScope: scoped ? "selected" : "all",
    agentIds: profile.agentIds,
    skillSlugs: profile.skillSlugs,
    cpuRequest: resources.requests?.cpu ?? "100m",
    memoryRequest: resources.requests?.memory ?? "256Mi",
    cpuLimit: resources.limits?.cpu ?? "1",
    memoryLimit: resources.limits?.memory ?? "1Gi",
    workspaceSize: resources.workspace?.sizeLimit ?? "1Gi",
    allowWebSearch: profile.allowWebSearch,
    allowWebFetch: profile.allowWebFetch,
    allowGlab: profile.allowGlab,
    allowGh: profile.allowGh,
    nativeToolsText: JSON.stringify(profile.nativeToolPermissions ?? {}, null, 2),
  };
}

export function useWorkerForm(initial: () => WorkerFormValues): WorkerFormApi {
  const [values, setValues] = useState<WorkerFormValues>(initial);
  return {
    values,
    set: (patch) => setValues((current) => ({ ...current, ...patch })),
  };
}

export function nativeToolsError(text: string): string | null {
  try {
    JSON.parse(text || "{}");
    return null;
  } catch {
    return "This is not valid JSON.";
  }
}

function resourcesFrom(values: WorkerFormValues) {
  return {
    requests: { cpu: values.cpuRequest || undefined, memory: values.memoryRequest || undefined },
    limits: { cpu: values.cpuLimit || undefined, memory: values.memoryLimit || undefined },
    workspace: { sizeLimit: values.workspaceSize || undefined },
  };
}

/** The request body for creating or updating a profile. Every field the form owns is sent. */
export function buildProfileBody(values: WorkerFormValues, editing: WorkerProfile | null) {
  return {
    name: values.name,
    harnessKind: values.harness,
    target: values.target,
    executionDriver: values.driver,
    image: values.image,
    imageTag: values.tag,
    agentScope: values.agentScope,
    agentIds: values.agentScope === "selected" ? values.agentIds : [],
    skillSlugs: values.skillSlugs,
    provider: values.provider || undefined,
    model: values.model || undefined,
    authSecretName: values.authSecret || undefined,
    // Stored values are never sent back by the API. `null` keeps each one as it is; this
    // form does not edit them (they are set through the API or an import).
    env: Object.fromEntries(Object.keys(editing?.env ?? {}).map((name) => [name, null])),
    envMappings: values.envMappings.map((m) => ({
      envVar: m.envVar,
      connectionId: m.connectionId || null,
      secretKey: m.secretKey || null,
    })),
    envMappingIds: values.envMappingIds,
    allowWebSearch: values.allowWebSearch,
    allowWebFetch: values.allowWebFetch,
    allowGlab: values.allowGlab,
    allowGh: values.allowGh,
    nativeToolPermissions: JSON.parse(values.nativeToolsText || "{}"),
    resources: resourcesFrom(values),
  };
}

/** True when a saved change needs the running worker restarted to take effect. */
export function restartFieldsChanged(editing: WorkerProfile | null, values: WorkerFormValues): boolean {
  if (!editing) return false;
  const existingResources = ((editing as WorkerProfile & { resources?: Record<string, unknown> }).resources ??
    {}) as Resources;
  return (
    editing.image !== values.image ||
    editing.imageTag !== values.tag ||
    editing.target !== values.target ||
    editing.executionDriver !== values.driver ||
    editing.harnessKind !== values.harness ||
    (editing.authSecretName ?? "") !== values.authSecret ||
    (editing.provider ?? "") !== (values.provider || "") ||
    (editing.model ?? "") !== (values.model || "") ||
    JSON.stringify(existingResources) !== JSON.stringify(resourcesFrom(values))
  );
}
