"use client";

import { useQuery, useQueryClient, type QueryClient } from "@tanstack/react-query";

import { api } from "./api.ts";
import { request } from "./http.ts";
import { plural, probeBody, renderConfig, type ProviderDraft, type TestResult } from "./provider-meta.ts";
import { queryKeys, unwrap, useProviders } from "./queries.ts";
import type { ProviderView } from "./types.ts";
import { errorMessage } from "@orbit/api-client";

/** One provider, found in the list (the API has no single GET). */
export function useProvider(id: string) {
  const providers = useProviders();
  return { ...providers, data: providers.data?.find((provider) => provider.id === id) };
}

export async function createProvider(draft: ProviderDraft): Promise<ProviderView> {
  return unwrap(
    await api.POST("/api/v1/providers", {
      body: {
        name: draft.name.trim(),
        kind: draft.kind,
        config: renderConfig(draft),
        enabled: draft.enabled,
        ...(draft.credential ? { credential: draft.credential } : {}),
      },
    }),
  ) as ProviderView;
}

export async function updateProvider(provider: ProviderView, draft: ProviderDraft): Promise<ProviderView> {
  return unwrap(
    await api.PATCH("/api/v1/providers/{provider_id}", {
      params: { path: { provider_id: provider.id } },
      body: {
        name: draft.name.trim(),
        config: renderConfig(draft, provider.config ?? {}),
        enabled: draft.enabled,
        ...(draft.credential ? { credential: draft.credential } : {}),
      },
    }),
  ) as ProviderView;
}

export async function setProviderEnabled(id: string, enabled: boolean): Promise<void> {
  unwrap(await api.PATCH("/api/v1/providers/{provider_id}", { params: { path: { provider_id: id } }, body: { enabled } }));
}

export async function deleteProvider(id: string): Promise<void> {
  unwrap(await api.DELETE("/api/v1/providers/{provider_id}", { params: { path: { provider_id: id } } }));
}

// ---------------------------------------------------------------------------- tests

/**
 * The API does not store test results, so this session's results live in the query
 * cache: the list and the provider page show the same one.
 */
const TESTS_KEY = ["provider-tests"] as const;

export function useProviderTests(): Record<string, TestResult> {
  const query = useQuery<Record<string, TestResult>>({
    queryKey: TESTS_KEY,
    queryFn: () => ({}),
    staleTime: Infinity,
    gcTime: Infinity,
  });
  return query.data ?? {};
}

function remember(client: QueryClient, id: string, result: TestResult) {
  client.setQueryData<Record<string, TestResult>>(TESTS_KEY, (current) => ({ ...current, [id]: result }));
}

export interface TestOutcome extends TestResult {
  models: string[];
  /** One line for a toast. */
  summary: string;
}

/** Test a saved provider (lists its models) and remember the result. */
export function useTestProvider() {
  const client = useQueryClient();
  return async (id: string): Promise<TestOutcome> => {
    let outcome: TestOutcome;
    try {
      const result = unwrap(
        await api.POST("/api/v1/providers/{provider_id}/test", { params: { path: { provider_id: id } } }),
      );
      outcome = {
        ok: result.ok,
        latencyMs: result.latencyMs,
        modelCount: result.models.length,
        models: result.models,
        error: result.error ?? undefined,
        at: Date.now(),
        summary: result.ok
          ? `Reachable in ${result.latencyMs} ms. ${plural(result.models.length, "model", "models")} found.`
          : `Test failed: ${result.error ?? "the endpoint did not answer"}`,
      };
      if (result.ok) void client.invalidateQueries({ queryKey: queryKeys.providers });
    } catch (error) {
      outcome = { ok: false, error: errorMessage(error), models: [], at: Date.now(), summary: errorMessage(error) };
    }
    remember(client, id, outcome);
    return outcome;
  };
}

/** Test an endpoint draft before it is saved. */
export async function testDraft(draft: ProviderDraft): Promise<TestOutcome> {
  try {
    // `request` adds auth and turns non-JSON error bodies into a readable message; a
    // probe that ran but failed comes back as `{ ok: false }`.
    const result = await request<{
      ok?: boolean;
      latencyMs?: number;
      models?: string[];
      error?: string | { message?: string };
      message?: string;
    }>("/api/v1/providers/probe", { method: "POST", body: JSON.stringify(probeBody(draft)) });
    const failure = typeof result.error === "string" ? result.error : (result.error?.message ?? result.message);
    const models = result.models ?? [];
    return result.ok
      ? {
          ok: true,
          latencyMs: result.latencyMs ?? 0,
          modelCount: models.length,
          models,
          at: Date.now(),
          summary: `Reachable in ${result.latencyMs ?? 0} ms. ${plural(models.length, "model", "models")} found.`,
        }
      : {
          ok: false,
          error: failure ?? "The endpoint did not answer",
          models: [],
          at: Date.now(),
          summary: `Test failed: ${failure ?? "the endpoint did not answer"}`,
        };
  } catch (error) {
    return { ok: false, error: errorMessage(error), models: [], at: Date.now(), summary: errorMessage(error) };
  }
}
