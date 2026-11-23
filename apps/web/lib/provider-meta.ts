import { mergeProviderConfig } from "./provider-config.ts";

/**
 * Pure helpers for model providers: labels, the form draft and the config it renders,
 * and status. No runtime imports beyond `provider-config`, so `node --test` can load it.
 */

export type ProviderKind = "openai_compatible" | "anthropic" | "harness_managed";

/** The fields of a provider the API returns that these helpers read. */
export interface ProviderLike {
  id: string;
  name: string;
  kind: string;
  enabled: boolean;
  hasCredential: boolean;
  config: unknown;
}

export const KIND_LABEL: Record<ProviderKind, string> = {
  openai_compatible: "OpenAI-compatible",
  anthropic: "Anthropic",
  harness_managed: "Harness-managed",
};

export const kindLabel = (kind: string) => KIND_LABEL[kind as ProviderKind] ?? kind;

/**
 * A provider ORBIT talks to itself. Every kind except `harness_managed` qualifies,
 * including endpoints and Anthropic providers; do not narrow this to OpenAI-compatible.
 */
export function isOrbitManagedProvider(kind: string): boolean {
  return kind !== "harness_managed";
}

export const HARNESS_LABEL: Record<string, string> = {
  opencode: "OpenCode",
  claude_code: "Claude Code",
  pi: "Pi",
};

export const ANTHROPIC_BASE_URL = "https://api.anthropic.com/v1";

export const PROVIDER_PRESETS = [
  { id: "custom", label: "Custom endpoint", baseUrl: "", hint: "Any OpenAI-compatible /v1 API." },
  {
    id: "openrouter",
    label: "OpenRouter",
    baseUrl: "https://openrouter.ai/api/v1",
    hint: "Use an OpenRouter API key. Model ids are the ones OpenRouter lists.",
  },
  {
    id: "deepseek",
    label: "DeepSeek",
    baseUrl: "https://api.deepseek.com",
    hint: "Use a DeepSeek API key; the OpenAI-compatible endpoint is the root URL.",
  },
  {
    id: "gemini",
    label: "Google Gemini",
    baseUrl: "https://generativelanguage.googleapis.com/v1beta/openai",
    hint: "Gemini's OpenAI-compatible endpoint with a Gemini API key.",
  },
  {
    id: "azure",
    label: "Azure OpenAI",
    baseUrl: "https://YOUR-RESOURCE.openai.azure.com/openai/v1",
    hint: "Replace YOUR-RESOURCE. Azure's v1 API accepts the key as a bearer token.",
  },
] as const;

export type CompatKey = "tools" | "streaming" | "jsonMode" | "vision" | "modelsEndpoint";

export const COMPAT_FLAGS: Array<{ key: CompatKey; title: string; description: string }> = [
  { key: "tools", title: "Tool calling", description: "The endpoint supports function and tool calls." },
  { key: "streaming", title: "Streaming", description: "Responses can be streamed token by token." },
  { key: "jsonMode", title: "JSON mode", description: "The endpoint can force valid JSON output." },
  { key: "vision", title: "Vision", description: "Accepts images as input." },
  { key: "modelsEndpoint", title: "Models endpoint", description: "Lists models at /models." },
];

export interface ProviderDraft {
  name: string;
  kind: ProviderKind;
  baseUrl: string;
  model: string;
  /** The model catalog, one id per entry. */
  models: string[];
  credential: string;
  contextWindow: string;
  maxOutputTokens: string;
  tools: boolean;
  streaming: boolean;
  jsonMode: boolean;
  vision: boolean;
  modelsEndpoint: boolean;
  harnessProvider: string;
  harnessKind: string;
  enabled: boolean;
}

export const EMPTY_DRAFT: ProviderDraft = {
  name: "",
  kind: "openai_compatible",
  baseUrl: "",
  model: "",
  models: [],
  credential: "",
  contextWindow: "",
  maxOutputTokens: "",
  tools: false,
  streaming: true,
  jsonMode: false,
  vision: false,
  modelsEndpoint: true,
  harnessProvider: "opencode-go",
  harnessKind: "opencode",
  enabled: true,
};

const asRecord = (value: unknown): Record<string, unknown> =>
  value && typeof value === "object" && !Array.isArray(value) ? (value as Record<string, unknown>) : {};

/** The model catalog stored under `settings.models` (harness-managed) or fetched earlier. */
export function catalogOf(config: unknown): string[] {
  const models = asRecord(asRecord(config).settings).models;
  return Array.isArray(models) ? models.filter((item): item is string => typeof item === "string") : [];
}

/** "a, b ,, c" → ["a", "b", "c"], without duplicates. */
export function parseModels(value: string): string[] {
  return [...new Set(value.split(/[,\n]/).map((item) => item.trim()).filter(Boolean))];
}

/** A draft for editing a stored provider. The credential is write-only and starts empty. */
export function draftFrom(provider: ProviderLike): ProviderDraft {
  const config = asRecord(provider.config);
  const compat = asRecord(config.compat);
  return {
    ...EMPTY_DRAFT,
    name: provider.name,
    kind: (provider.kind as ProviderKind) ?? "openai_compatible",
    baseUrl: String(config.baseUrl ?? ""),
    model: String(config.model ?? ""),
    models: catalogOf(config),
    contextWindow: config.contextWindow ? String(config.contextWindow) : "",
    maxOutputTokens: config.maxOutputTokens ? String(config.maxOutputTokens) : "",
    tools: Boolean(compat.tools),
    streaming: compat.streaming !== false,
    jsonMode: Boolean(compat.jsonMode),
    vision: Boolean(compat.vision),
    modelsEndpoint: compat.modelsEndpoint !== false,
    harnessProvider: String(config.provider ?? "opencode-go"),
    harnessKind: String(config.harness ?? "opencode"),
    enabled: provider.enabled,
  };
}

const trimUrl = (value: string) => value.trim().replace(/\/$/, "");

/**
 * The config a draft saves. On edit it is merged into the stored config, so fields the
 * form does not show (headers, request params, other flags and settings) survive.
 */
export function renderConfig(draft: ProviderDraft, existing?: unknown): Record<string, unknown> {
  const editing = existing !== undefined && existing !== null;
  const limits = {
    ...(draft.contextWindow ? { contextWindow: Number(draft.contextWindow) } : {}),
    ...(draft.maxOutputTokens ? { maxOutputTokens: Number(draft.maxOutputTokens) } : {}),
  };
  const compatFlags = {
    tools: draft.tools,
    streaming: draft.streaming,
    jsonMode: draft.jsonMode,
    vision: draft.vision,
    modelsEndpoint: draft.modelsEndpoint,
  };
  // The catalog lives in `settings.models` for every kind. On edit it is always sent, so
  // clearing the list sticks; the merge keeps any other settings.
  const catalog = editing || draft.models.length ? { settings: { models: draft.models } } : {};
  const rendered =
    draft.kind === "anthropic"
      ? { baseUrl: trimUrl(draft.baseUrl) || ANTHROPIC_BASE_URL, model: draft.model.trim(), ...limits, ...catalog }
      : draft.kind === "openai_compatible"
        ? {
            baseUrl: trimUrl(draft.baseUrl),
            model: draft.model.trim(),
            ...limits,
            // On create these empty maps are part of the request shape; on edit they
            // would overwrite stored values, so leave them to the merge.
            ...(editing ? {} : { headers: {}, requestParams: {} }),
            compat: editing
              ? compatFlags
              : { chatCompletions: true, responsesApi: false, embeddings: false, ...compatFlags },
            ...catalog,
          }
        : {
            harness: draft.harnessKind,
            provider: draft.harnessProvider.trim() || undefined,
            model: draft.model.trim() || undefined,
            ...catalog,
          };
  return mergeProviderConfig(editing ? asRecord(existing) : null, rendered);
}

/** The body of `POST /providers/probe` for an endpoint draft that is not saved yet. */
export function probeBody(draft: ProviderDraft) {
  return {
    kind: draft.kind,
    config:
      draft.kind === "anthropic"
        ? { baseUrl: trimUrl(draft.baseUrl) || ANTHROPIC_BASE_URL, model: draft.model.trim() || "discover" }
        : {
            baseUrl: trimUrl(draft.baseUrl),
            model: draft.model.trim() || "discover",
            headers: {},
            compat: {
              chatCompletions: true,
              responsesApi: false,
              tools: draft.tools,
              streaming: draft.streaming,
              jsonMode: draft.jsonMode,
              vision: draft.vision,
              embeddings: false,
              modelsEndpoint: draft.modelsEndpoint,
            },
          },
    credential: draft.credential.trim() || undefined,
  };
}

/** Problems that stop a draft from saving, by form section. */
export function draftIssues(draft: ProviderDraft): Array<{ section: "type" | "connection" | "models" | "limits"; message: string }> {
  const issues: Array<{ section: "type" | "connection" | "models" | "limits"; message: string }> = [];
  if (!draft.name.trim()) issues.push({ section: "connection", message: "Enter a name." });
  if (draft.kind === "openai_compatible") {
    const url = draft.baseUrl.trim();
    if (!url) issues.push({ section: "connection", message: "Enter the base URL." });
    else if (!/^https?:\/\/\S+$/i.test(url)) {
      issues.push({ section: "connection", message: "The base URL must start with https:// or http://." });
    } else if (url.includes("YOUR-RESOURCE")) {
      issues.push({ section: "connection", message: "Replace YOUR-RESOURCE in the base URL with your Azure resource name." });
    }
  }
  if (draft.kind !== "harness_managed" && !draft.model.trim()) {
    issues.push({ section: "models", message: "Choose a default model. Test connection can list them." });
  }
  if (draft.kind === "harness_managed" && !draft.harnessKind) {
    issues.push({ section: "connection", message: "Choose the harness." });
  }
  for (const [key, label] of [
    ["contextWindow", "Context window"],
    ["maxOutputTokens", "Max output"],
  ] as const) {
    const value = draft[key].trim();
    if (value && !(Number.isInteger(Number(value)) && Number(value) > 0)) {
      issues.push({ section: "limits", message: `${label} must be a whole number of tokens.` });
    } else if (key === "contextWindow" && value && Number(value) < 128) {
      issues.push({ section: "limits", message: "Context window must be at least 128 tokens." });
    }
  }
  return issues;
}

/** Result of the last connection test in this browser session. */
export interface TestResult {
  ok: boolean;
  latencyMs?: number;
  modelCount?: number;
  error?: string;
  at: number;
}

export type ProviderState = "ready" | "failing" | "disabled";

/** Disabled wins; otherwise a failed test this session marks it failing. */
export function providerState(provider: Pick<ProviderLike, "enabled">, test?: TestResult | null): ProviderState {
  if (!provider.enabled) return "disabled";
  return test && !test.ok ? "failing" : "ready";
}

export const PROVIDER_STATUS: Record<
  ProviderState,
  { label: string; tone: "success" | "danger" | "neutral"; shape: "solid" | "cross" | "slash" }
> = {
  ready: { label: "Enabled", tone: "success", shape: "solid" },
  failing: { label: "Test failed", tone: "danger", shape: "cross" },
  disabled: { label: "Disabled", tone: "neutral", shape: "slash" },
};

/** Where the provider is reached: the base URL, or the harness that owns it. */
export function endpointLabel(provider: Pick<ProviderLike, "kind" | "config">): string {
  const config = asRecord(provider.config);
  if (provider.kind === "harness_managed") {
    const harness = HARNESS_LABEL[String(config.harness)] ?? String(config.harness ?? "a harness");
    return config.provider ? `${harness} · ${String(config.provider)}` : harness;
  }
  return String(config.baseUrl ?? "") || (provider.kind === "anthropic" ? ANTHROPIC_BASE_URL : "No base URL");
}

export const plural = (count: number, one: string, many: string) => `${count} ${count === 1 ? one : many}`;
