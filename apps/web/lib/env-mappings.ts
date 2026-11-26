"use client";

import { useMemo } from "react";
import { useQueries } from "@tanstack/react-query";

import { request } from "./http";
import { useEnvMappings, useWorkerProfiles } from "./queries";

export interface EnvMappingEntry {
  envVar: string;
  connectionId?: string | null;
  secretKey?: string | null;
  secretRef?: string | null;
}

export interface EnvMappingRecord {
  id: string;
  name: string;
  description: string;
  mappings: EnvMappingEntry[];
  ownerTeamId?: string | null;
  visibility?: string | null;
  createdAt: string;
  updatedAt: string;
}

export function listEnvMappings(): Promise<EnvMappingRecord[]> {
  return request<{ items: EnvMappingRecord[] }>("/api/v1/env-mappings?limit=500").then(
    (page) => page.items,
  );
}

export function workerProfileEnvMappings(profileId: string): Promise<EnvMappingRecord[]> {
  return request<{ items: EnvMappingRecord[] }>(
    `/api/v1/worker-profiles/${profileId}/env-mappings?limit=500`,
  ).then((page) => page.items);
}

export function createEnvMapping(body: {
  name: string;
  description: string;
  mappings: EnvMappingEntry[];
}): Promise<EnvMappingRecord> {
  return request<EnvMappingRecord>("/api/v1/env-mappings", {
    method: "POST",
    body: JSON.stringify(body),
  });
}

export function deleteEnvMapping(id: string): Promise<void> {
  return request<void>(`/api/v1/env-mappings/${id}`, { method: "DELETE" });
}

export function updateEnvMapping(
  id: string,
  body: { name: string; description: string; mappings: EnvMappingEntry[] },
): Promise<EnvMappingRecord> {
  return request<EnvMappingRecord>(`/api/v1/env-mappings/${id}`, {
    method: "PATCH",
    body: JSON.stringify(body),
  });
}

// ---------------------------------------------------------------------------- hooks

/** One environment, found in the list (the API has no single GET). */
export function useEnvMapping(id: string) {
  const mappings = useEnvMappings();
  return { ...mappings, data: mappings.data?.find((mapping) => mapping.id === id) };
}

/**
 * Which worker profiles use each environment. The API answers per profile, so this asks
 * each one; there are only ever a handful. `known` is false until every answer is in.
 */
export function useEnvMappingUsage() {
  const profiles = useWorkerProfiles();
  const queries = useQueries({
    queries: (profiles.data ?? []).map((profile) => ({
      queryKey: ["worker-profile-env-mappings", profile.id],
      queryFn: () => workerProfileEnvMappings(profile.id),
      staleTime: 30_000,
    })),
  });
  const known = profiles.isSuccess && queries.every((query) => query.isSuccess);
  // The query results change identity every render; their data timestamps do not.
  const stamp = queries.map((query) => query.dataUpdatedAt).join(",");
  const usedBy = useMemo(() => {
    const result = new Map<string, Array<{ id: string; name: string }>>();
    (profiles.data ?? []).forEach((profile, index) => {
      for (const mapping of queries[index]?.data ?? []) {
        result.set(mapping.id, [...(result.get(mapping.id) ?? []), { id: profile.id, name: profile.name }]);
      }
    });
    return result;
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [profiles.data, stamp]);
  const allProfiles = useMemo(
    () => (profiles.data ?? []).map((profile) => ({ id: profile.id, name: profile.name })),
    [profiles.data],
  );
  return { usedBy, known, profiles: allProfiles };
}
