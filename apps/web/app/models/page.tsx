"use client";

import { useRef, useState } from "react";
import { useMutation, useQueryClient } from "@tanstack/react-query";
import {
  Boxes,
  Cpu,
  Download,
  KeyRound,
  MoreHorizontal,
  Pencil,
  Plus,
  Server,
  Trash2,
  Zap,
} from "lucide-react";
import { toast } from "sonner";

import { ResourceAccessFields, type ResourceAccessHandle } from "@/components/resource-access-fields";
import { Badge } from "@/components/ui/badge";
import { Disclosure } from "@/components/ui/disclosure";
import { Button } from "@/components/ui/button";
import { Callout } from "@/components/ui/callout";
import { Card } from "@/components/ui/card";
import { CheckboxField } from "@/components/ui/checkbox";
import { ChoiceCard } from "@/components/ui/choice-card";
import { useConfirm } from "@/components/ui/confirm";
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog";
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuTrigger,
} from "@/components/ui/dropdown-menu";
import { EmptyState } from "@/components/ui/empty-state";
import { Field } from "@/components/ui/field";
import { Input } from "@/components/ui/input";
import { PageHeader } from "@/components/ui/page-header";
import { Section } from "@/components/ui/section";
import { Select } from "@/components/ui/select";
import { exportResource } from "@/components/export-resource-button";
import { ImportPortableButton } from "@/components/import-portable-dialog";
import { Skeleton } from "@/components/ui/skeleton";
import { Status, type StatusMeta } from "@/components/ui/status";
import { api } from "@/lib/api";
import { mergeProviderConfig } from "@/lib/provider-config";
import { request } from "@/lib/http";
import { queryKeys, unwrap, useProviders } from "@/lib/queries";
import { errorMessage } from "@orbit/api-client";
import type { ProviderView } from "@/lib/types";
import { relativeTime } from "@/lib/utils";

interface ProviderForm {
  name: string;
  kind: "openai_compatible" | "anthropic" | "harness_managed";
  baseUrl: string;
  model: string;
  models: string;
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
}

const EMPTY_FORM: ProviderForm = {
  name: "",
  kind: "openai_compatible",
  baseUrl: "",
  model: "",
  models: "",
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
};

const KIND_LABEL: Record<string, string> = {
  openai_compatible: "OpenAI-compatible",
  anthropic: "Anthropic",
  harness_managed: "Harness-managed",
};

const ANTHROPIC_BASE_URL = "https://api.anthropic.com/v1";
const PROVIDER_PRESETS = [
  {
    id: "custom",
    label: "Custom endpoint",
    baseUrl: "",
    hint: "Any OpenAI-compatible /v1 API.",
  },
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

const HARNESS_LABEL: Record<string, string> = {
  opencode: "OpenCode",
  claude_code: "Claude Code",
  pi: "Pi",
};

const COMPAT_FLAGS: Array<{
  key: "tools" | "streaming" | "jsonMode" | "vision" | "modelsEndpoint";
  title: string;
  description: string;
}> = [
  { key: "tools", title: "Tool calling", description: "The endpoint supports function and tool calls." },
  { key: "streaming", title: "Streaming", description: "Responses can be streamed token by token." },
  { key: "jsonMode", title: "JSON mode", description: "The endpoint can force valid JSON output." },
  { key: "vision", title: "Vision", description: "Accepts images as input." },
  { key: "modelsEndpoint", title: "Models endpoint", description: "Lists models at /models." },
];

interface TestResult {
  ok: boolean;
  latencyMs?: number;
  modelCount?: number;
  error?: string;
  at: number;
}

const parseModels = (value: string) =>
  value
    .split(",")
    .map((item) => item.trim())
    .filter(Boolean);

const catalogOf = (config: Record<string, unknown>): string[] => {
  const models = (config.settings as Record<string, unknown> | undefined)?.models;
  return Array.isArray(models) ? (models as string[]) : [];
};

const plural = (count: number, one: string, many: string) => `${count} ${count === 1 ? one : many}`;

function testStatus(result: TestResult | undefined): StatusMeta {
  if (!result) return { label: "Not tested yet", tone: "neutral", shape: "ring" };
  if (result.ok) {
    return {
      label: `Reachable in ${result.latencyMs ?? 0} ms`,
      tone: "success",
      shape: "solid",
    };
  }
  return { label: "Test failed", tone: "danger", shape: "cross" };
}

export default function ModelsPage() {
  const accessRef = useRef<ResourceAccessHandle>(null);
  const providers = useProviders();
  const queryClient = useQueryClient();
  const confirm = useConfirm();
  const [open, setOpen] = useState(false);
  const [editing, setEditing] = useState<ProviderView | null>(null);
  const [form, setForm] = useState<ProviderForm>(EMPTY_FORM);
  const [probing, setProbing] = useState<string | null>(null);
  const [probingDraft, setProbingDraft] = useState(false);
  const [draftResult, setDraftResult] = useState<TestResult | null>(null);
  // The API does not store probe results, so we remember this session's tests per provider.
  const [results, setResults] = useState<Record<string, TestResult>>({});

  const save = useMutation({
    mutationFn: async () => {
      const catalog = parseModels(form.models);
      const limits = {
        ...(form.contextWindow ? { contextWindow: Number(form.contextWindow) } : {}),
        ...(form.maxOutputTokens ? { maxOutputTokens: Number(form.maxOutputTokens) } : {}),
      };
      // Only these fields are on the form. Everything else in a stored config is kept
      // as-is so editing one setting cannot erase headers, flags or other settings.
      const compatFlags = {
        tools: form.tools,
        streaming: form.streaming,
        jsonMode: form.jsonMode,
        vision: form.vision,
        modelsEndpoint: form.modelsEndpoint,
      };
      const rendered =
        form.kind === "anthropic"
          ? {
              baseUrl: form.baseUrl.trim().replace(/\/$/, "") || ANTHROPIC_BASE_URL,
              model: form.model.trim(),
              ...limits,
            }
          : form.kind === "openai_compatible"
          ? {
              baseUrl: form.baseUrl.trim().replace(/\/$/, ""),
              model: form.model.trim(),
              ...limits,
              // On create these empty maps are part of the existing request shape; on
              // edit they would overwrite stored values, so leave them to the merge.
              ...(editing ? {} : { headers: {}, requestParams: {} }),
              compat: editing
                ? compatFlags
                : { chatCompletions: true, responsesApi: false, embeddings: false, ...compatFlags },
              // No `settings` here: the API rejects unknown fields for this kind.
            }
          : {
              harness: form.harnessKind,
              provider: form.harnessProvider.trim() || undefined,
              model: form.model.trim() || undefined,
              ...(catalog.length ? { settings: { models: catalog } } : {}),
            };
      const existing = editing ? (editing.config as Record<string, unknown> | undefined) : null;
      const config = mergeProviderConfig(existing, rendered);

      if (editing) {
        return unwrap(
          await api.PATCH("/api/v1/providers/{provider_id}", {
            params: { path: { provider_id: editing.id } },
            body: {
              name: form.name.trim(),
              config,
              ...(form.credential ? { credential: form.credential } : {}),
            },
          }),
        );
      }
      return unwrap(
        await api.POST("/api/v1/providers", {
          body: {
            name: form.name.trim(),
            kind: form.kind,
            config,
            ...(form.credential ? { credential: form.credential } : {}),
          },
        }),
      );
    },
    onSuccess: () => {
      toast.success(editing ? "Provider updated" : "Provider created");
      closeDialog();
      void queryClient.invalidateQueries({ queryKey: queryKeys.providers });
    },
    onError: (error) => toast.error(errorMessage(error)),
  });

  async function saveProvider() {
    if ((await accessRef.current?.save()) === false) return;
    save.mutate();
  }

  const remove = useMutation({
    mutationFn: async (id: string) =>
      unwrap(
        await api.DELETE("/api/v1/providers/{provider_id}", {
          params: { path: { provider_id: id } },
        }),
      ),
    onSuccess: () => {
      toast.success("Provider deleted");
      void queryClient.invalidateQueries({ queryKey: queryKeys.providers });
    },
    onError: (error) => toast.error(errorMessage(error)),
  });

  async function probe(provider: ProviderView) {
    setProbing(provider.id);
    try {
      const result = unwrap(
        await api.POST("/api/v1/providers/{provider_id}/test", {
          params: { path: { provider_id: provider.id } },
        }),
      );
      setResults((current) => ({
        ...current,
        [provider.id]: {
          ok: result.ok,
          latencyMs: result.latencyMs,
          modelCount: result.models.length,
          error: result.error ?? undefined,
          at: Date.now(),
        },
      }));
      if (result.ok) {
        toast.success(
          `Reachable in ${result.latencyMs} ms. ${plural(result.models.length, "model", "models")} found.`,
        );
        void queryClient.invalidateQueries({ queryKey: queryKeys.providers });
      } else {
        toast.error(result.error ?? "Test failed");
      }
    } catch (error) {
      setResults((current) => ({
        ...current,
        [provider.id]: { ok: false, error: errorMessage(error), at: Date.now() },
      }));
      toast.error(errorMessage(error));
    } finally {
      setProbing(null);
    }
  }

  async function probeDraft() {
    if (!form.baseUrl.trim()) {
      toast.error("Enter a base URL to test the connection");
      return;
    }
    setProbingDraft(true);
    try {
      // `request` adds auth, handles 401 and turns non-JSON/HTML error bodies into a
      // readable message; a probe that ran but failed comes back as `{ ok: false }`.
      const result = await request<{
        ok?: boolean;
        latencyMs?: number;
        models?: string[];
        error?: string | { message?: string };
        message?: string;
      }>("/api/v1/providers/probe", {
        method: "POST",
        body: JSON.stringify({
          kind: form.kind,
          config:
            form.kind === "anthropic"
              ? {
                  baseUrl: form.baseUrl.trim().replace(/\/$/, "") || ANTHROPIC_BASE_URL,
                  model: form.model.trim() || "discover",
                }
              : {
                  baseUrl: form.baseUrl.trim().replace(/\/$/, ""),
                  model: form.model.trim() || "discover",
                  headers: {},
                  compat: {
                    chatCompletions: true,
                    responsesApi: false,
                    tools: form.tools,
                    streaming: form.streaming,
                    jsonMode: form.jsonMode,
                    vision: form.vision,
                    embeddings: false,
                    modelsEndpoint: form.modelsEndpoint,
                  },
                },
          credential: form.credential.trim() || undefined,
        }),
      });
      // The error may be a string (probe result) or the API's error envelope.
      const failure =
        typeof result.error === "string" ? result.error : (result.error?.message ?? result.message);
      if (result.ok) {
        const models = result.models ?? [];
        setForm((current) => ({
          ...current,
          models: current.models || models.join(", "),
          model: current.model || models[0] || "",
        }));
        setDraftResult({ ok: true, latencyMs: result.latencyMs ?? 0, modelCount: models.length, at: Date.now() });
        toast.success(
          `Reachable in ${result.latencyMs ?? 0} ms. ${plural(models.length, "model", "models")} found.`,
        );
      } else {
        setDraftResult({ ok: false, error: failure ?? "The endpoint did not answer", at: Date.now() });
        toast.error(`Test failed: ${failure ?? "the endpoint did not answer"}`);
      }
    } catch (error) {
      setDraftResult({ ok: false, error: errorMessage(error), at: Date.now() });
      toast.error(errorMessage(error));
    } finally {
      setProbingDraft(false);
    }
  }

  function closeDialog() {
    setOpen(false);
    setEditing(null);
    setForm(EMPTY_FORM);
    setDraftResult(null);
  }

  function openCreate() {
    setEditing(null);
    setForm(EMPTY_FORM);
    setDraftResult(null);
    setOpen(true);
  }

  function openEdit(provider: ProviderView) {
    const config = (provider.config ?? {}) as Record<string, unknown>;
    const compat = (config.compat ?? {}) as Record<string, boolean>;
    setEditing(provider);
    setDraftResult(null);
    setForm({
      ...EMPTY_FORM,
      name: provider.name,
      kind: provider.kind as ProviderForm["kind"],
      baseUrl: String(config.baseUrl ?? ""),
      model: String(config.model ?? ""),
      models: catalogOf(config).join(", "),
      contextWindow: config.contextWindow ? String(config.contextWindow) : "",
      maxOutputTokens: config.maxOutputTokens ? String(config.maxOutputTokens) : "",
      tools: Boolean(compat.tools),
      streaming: compat.streaming !== false,
      jsonMode: Boolean(compat.jsonMode),
      vision: Boolean(compat.vision),
      modelsEndpoint: compat.modelsEndpoint !== false,
      harnessProvider: String(config.provider ?? "opencode-go"),
      harnessKind: String(config.harness ?? "opencode"),
    });
    setOpen(true);
  }

  async function deleteProvider(provider: ProviderView) {
    const ok = await confirm({
      title: `Delete provider ${provider.name}?`,
      description:
        "Agents that use it will no longer be able to run until you pick another provider. This cannot be undone.",
      confirmLabel: "Delete provider",
      destructive: true,
    });
    if (ok) remove.mutate(provider.id);
  }

  const modelChoices = parseModels(form.models);
  const isEndpoint = form.kind !== "harness_managed";
  const isOpenAi = form.kind === "openai_compatible";

  return (
    <div className="space-y-6">
      <PageHeader
        title="Model providers"
        description="Where agents get their models: an OpenAI-compatible endpoint, the native Anthropic API, or a provider managed by a harness. Credentials are encrypted and never shown again."
        actions={
          <div className="flex flex-wrap gap-2">
            <ImportPortableButton />
            <Button onClick={openCreate}>
              <Plus /> Add provider
            </Button>
          </div>
        }
      />

      {providers.isLoading ? (
        <Card className="overflow-hidden">
          {Array.from({ length: 3 }).map((_, index) => (
            <div key={index} className="border-b px-5 py-4 last:border-b-0">
              <Skeleton className="h-14" />
            </div>
          ))}
        </Card>
      ) : providers.isError ? (
        <Callout tone="danger" title="Could not load providers">
          {errorMessage(providers.error)}
        </Callout>
      ) : !providers.data?.length ? (
        <EmptyState
          icon={<Boxes />}
          title="No model providers yet"
          description="Add vLLM, LiteLLM, an API gateway or any OpenAI-compatible endpoint. Providers are separate from harnesses, so agents can mix them freely."
          action={
            <Button variant="outline" onClick={openCreate}>
              <Plus /> Add provider
            </Button>
          }
        />
      ) : (
        <Section title="Providers" count={providers.data.length}>
          <Card className="overflow-hidden">
            {providers.data.map((provider) => {
              const config = (provider.config ?? {}) as Record<string, unknown>;
              const catalog = catalogOf(config);
              const result = results[provider.id];
              const testable = provider.kind !== "harness_managed";
              const model = String(config.model ?? "");
              return (
                <div
                  key={provider.id}
                  className="flex flex-col gap-3 border-b px-4 py-4 last:border-b-0 sm:px-5 lg:flex-row lg:items-start lg:justify-between"
                >
                  <div className="min-w-0 space-y-1.5">
                    <div className="flex flex-wrap items-center gap-2">
                      <p className="truncate text-sm font-medium">{provider.name}</p>
                      <Badge variant="outline">{KIND_LABEL[provider.kind] ?? provider.kind}</Badge>
                      {provider.enabled ? null : <Badge variant="secondary">Disabled</Badge>}
                    </div>

                    {testable ? (
                      <p className="truncate font-mono text-xs text-muted-foreground" title={String(config.baseUrl ?? "")}>
                        {String(config.baseUrl ?? "No base URL")}
                      </p>
                    ) : (
                      <p className="text-[13px] text-muted-foreground">
                        Runs through {HARNESS_LABEL[String(config.harness)] ?? String(config.harness ?? "a harness")}
                        {config.provider ? (
                          <>
                            {" as "}
                            <span className="font-mono text-xs">{String(config.provider)}</span>
                          </>
                        ) : null}
                      </p>
                    )}

                    <div className="flex flex-wrap items-center gap-x-4 gap-y-1 text-[13px] text-muted-foreground">
                      <span>
                        Default model:{" "}
                        {model ? (
                          <span className="break-all font-mono text-xs text-foreground">{model}</span>
                        ) : (
                          <span>{testable ? "not set" : "harness default"}</span>
                        )}
                      </span>
                      <span>{plural(catalog.length, "model", "models")} in catalog</span>
                      <span
                        className="inline-flex items-center gap-1"
                        title={provider.hasCredential ? "A credential is stored. It is never shown." : undefined}
                      >
                        <KeyRound className="h-3.5 w-3.5" aria-hidden />
                        {provider.hasCredential
                          ? "Credential set"
                          : testable
                            ? "No credential"
                            : "Authenticated by the harness"}
                      </span>
                    </div>

                    {testable ? (
                      <div className="space-y-0.5">
                        <Status meta={testStatus(result)} />
                        {result?.ok === false && result.error ? (
                          <p className="break-words text-[13px] text-destructive">{result.error}</p>
                        ) : null}
                        {result?.ok ? (
                          <p className="text-[13px] text-muted-foreground">
                            {plural(result.modelCount ?? 0, "model", "models")} found, tested{" "}
                            {relativeTime(new Date(result.at).toISOString())}
                          </p>
                        ) : null}
                      </div>
                    ) : null}
                  </div>

                  <div className="flex shrink-0 flex-wrap items-center gap-2">
                    {testable ? (
                      <Button
                        size="sm"
                        variant="outline"
                        onClick={() => void probe(provider)}
                        loading={probing === provider.id}
                      >
                        <Zap /> Test
                      </Button>
                    ) : null}
                    <Button size="sm" variant="ghost" onClick={() => openEdit(provider)}>
                      <Pencil /> Edit
                    </Button>
                    <DropdownMenu>
                      <DropdownMenuTrigger asChild>
                        <Button
                          size="icon-sm"
                          variant="ghost"
                          aria-label={`More actions for ${provider.name}`}
                        >
                          <MoreHorizontal />
                        </Button>
                      </DropdownMenuTrigger>
                      <DropdownMenuContent align="end">
                        <DropdownMenuItem onSelect={() => void exportResource("providers", provider.id, provider.name)}>
                          <Download /> Export as YAML
                        </DropdownMenuItem>
                        <DropdownMenuItem destructive onSelect={() => void deleteProvider(provider)}>
                          <Trash2 /> Delete provider
                        </DropdownMenuItem>
                      </DropdownMenuContent>
                    </DropdownMenu>
                  </div>
                </div>
              );
            })}
          </Card>
        </Section>
      )}

      <Dialog
        open={open}
        onOpenChange={(next) => {
          if (next) setOpen(true);
          else closeDialog();
        }}
      >
        <DialogContent className="max-w-xl">
          <DialogHeader>
            <DialogTitle>{editing ? `Edit ${editing.name}` : "Add provider"}</DialogTitle>
            <DialogDescription>
              {editing
                ? "Change how ORBIT reaches this provider. The type cannot be changed."
                : "Choose how agents will reach models, then fill in the details."}
            </DialogDescription>
          </DialogHeader>

          <div className="space-y-4">
            <div role="radiogroup" aria-label="Provider type" className="grid gap-2 sm:grid-cols-3">
              <ChoiceCard
                selected={form.kind === "openai_compatible"}
                disabled={Boolean(editing)}
                onSelect={() => setForm({ ...form, kind: "openai_compatible", baseUrl: "" })}
                icon={<Server />}
                title="OpenAI-compatible"
                description="vLLM, LiteLLM, a gateway, or any /v1 API."
              />
              <ChoiceCard
                selected={form.kind === "anthropic"}
                disabled={Boolean(editing)}
                onSelect={() =>
                  setForm({ ...form, kind: "anthropic", baseUrl: ANTHROPIC_BASE_URL })
                }
                icon={<Cpu />}
                title="Anthropic"
                description="The native Claude Messages API."
              />
              <ChoiceCard
                selected={form.kind === "harness_managed"}
                disabled={Boolean(editing)}
                onSelect={() => setForm({ ...form, kind: "harness_managed" })}
                icon={<Boxes />}
                title="Harness-managed"
                description="The harness signs in itself, for example OpenCode Go."
              />
            </div>

            <Field label="Name" htmlFor="provider-name">
              <Input
                id="provider-name"
                value={form.name}
                placeholder="Company gateway"
                onChange={(event) => setForm({ ...form, name: event.target.value })}
              />
            </Field>

            {isEndpoint ? (
              <>
                <Field
                  label="Quick start"
                  htmlFor="provider-preset"
                  hint="Presets only fill the base URL. Model names stay free text or come from the provider's /models."
                >
                  <Select
                    id="provider-preset"
                    value={
                      PROVIDER_PRESETS.find((preset) => preset.baseUrl === form.baseUrl)?.id ??
                      "custom"
                    }
                    onChange={(event) => {
                      const preset = PROVIDER_PRESETS.find(
                        (candidate) => candidate.id === event.target.value,
                      );
                      if (!preset) return;
                      setForm({
                        ...form,
                        kind: "openai_compatible",
                        baseUrl: preset.baseUrl,
                        model: "",
                      });
                    }}
                  >
                    {PROVIDER_PRESETS.map((preset) => (
                      <option key={preset.id} value={preset.id}>
                        {preset.label}
                      </option>
                    ))}
                  </Select>
                  <p className="text-[13px] text-muted-foreground">
                    {PROVIDER_PRESETS.find((preset) => preset.baseUrl === form.baseUrl)?.hint ??
                      "Choose a preset or enter a custom endpoint."}
                  </p>
                </Field>

                <Field
                  label="Base URL"
                  htmlFor="provider-base-url"
                  hint="The root of the API, usually ending in /v1. No fixed provider list: any compatible gateway works."
                >
                  <Input
                    id="provider-base-url"
                    className="font-mono"
                    value={form.baseUrl}
                    placeholder="https://gateway.example.com/v1"
                    onChange={(event) => setForm({ ...form, baseUrl: event.target.value })}
                  />
                </Field>

                <Field
                  label="Credential"
                  htmlFor="provider-credential"
                  optional
                  hint={
                    editing
                      ? editing.hasCredential
                        ? "A credential is stored. Leave blank to keep it, or enter a new one to replace it."
                        : "No credential is stored. Enter one to add it."
                      : "Stored encrypted. You will not be able to read it again."
                  }
                >
                  <Input
                    id="provider-credential"
                    type="password"
                    value={form.credential}
                    autoComplete="new-password"
                    onChange={(event) => setForm({ ...form, credential: event.target.value })}
                  />
                </Field>

                <Field
                  label="Model catalog"
                  htmlFor="provider-models"
                  optional
                  hint={
                    modelChoices.length
                      ? "Comma-separated. These fill the model pickers in agent and worker forms."
                      : "Test the connection to fetch models automatically, or type them separated by commas."
                  }
                >
                  <div className="flex gap-2">
                    <Input
                      id="provider-models"
                      className="min-w-0 flex-1"
                      value={form.models}
                      placeholder="qwen3-coder, llama-3.3-70b"
                      onChange={(event) => setForm({ ...form, models: event.target.value })}
                    />
                    <Button
                      type="button"
                      variant="outline"
                      onClick={() => void probeDraft()}
                      loading={probingDraft}
                      disabled={!form.baseUrl.trim()}
                    >
                      <Zap /> Test and fetch
                    </Button>
                  </div>
                  {draftResult ? (
                    <div className="pt-1">
                      <Status meta={testStatus(draftResult)} />
                      {draftResult.ok ? null : (
                        <p className="break-words text-[13px] text-destructive">{draftResult.error}</p>
                      )}
                    </div>
                  ) : null}
                </Field>

                <Field label="Default model" htmlFor="provider-model" hint="Used when an agent does not pick a model.">
                  {modelChoices.length ? (
                    <Select
                      id="provider-model"
                      value={form.model}
                      onChange={(event) => setForm({ ...form, model: event.target.value })}
                    >
                      <option value="">Choose a model</option>
                      {modelChoices.map((candidate) => (
                        <option key={candidate} value={candidate}>
                          {candidate}
                        </option>
                      ))}
                    </Select>
                  ) : (
                    <Input
                      id="provider-model"
                      className="font-mono"
                      value={form.model}
                      placeholder="qwen3-coder"
                      onChange={(event) => setForm({ ...form, model: event.target.value })}
                    />
                  )}
                </Field>

                <Disclosure title="Advanced settings" summary="Limits and what the endpoint supports">
                    <div className="grid gap-3 sm:grid-cols-2">
                      <Field label="Context window" htmlFor="provider-context" optional hint="In tokens.">
                        <Input
                          id="provider-context"
                          type="number"
                          inputMode="numeric"
                          value={form.contextWindow}
                          onChange={(event) => setForm({ ...form, contextWindow: event.target.value })}
                        />
                      </Field>
                      <Field label="Max output" htmlFor="provider-max-output" optional hint="In tokens.">
                        <Input
                          id="provider-max-output"
                          type="number"
                          inputMode="numeric"
                          value={form.maxOutputTokens}
                          onChange={(event) => setForm({ ...form, maxOutputTokens: event.target.value })}
                        />
                      </Field>
                    </div>
                    {isOpenAi ? (
                    <div className="grid gap-2 sm:grid-cols-2">
                      {COMPAT_FLAGS.map((flag) => (
                        <CheckboxField
                          key={flag.key}
                          title={flag.title}
                          description={flag.description}
                          checked={form[flag.key]}
                          onChange={(event) => setForm({ ...form, [flag.key]: event.target.checked })}
                        />
                      ))}
                    </div>
                    ) : null}
                  </Disclosure>
              </>
            ) : (
              <>
                <div className="grid gap-4 sm:grid-cols-2">
                  <Field label="Harness" htmlFor="provider-harness">
                    <Select
                      id="provider-harness"
                      value={form.harnessKind}
                      onChange={(event) => setForm({ ...form, harnessKind: event.target.value })}
                    >
                      <option value="opencode">OpenCode</option>
                      <option value="claude_code">Claude Code</option>
                      <option value="pi">Pi</option>
                    </Select>
                  </Field>
                  <Field
                    label="Provider name in the harness"
                    htmlFor="provider-harness-name"
                  >
                    <Input
                      id="provider-harness-name"
                      className="font-mono"
                      value={form.harnessProvider}
                      placeholder="opencode-go"
                      onChange={(event) => setForm({ ...form, harnessProvider: event.target.value })}
                    />
                  </Field>
                </div>

                <Field
                  label="Credential"
                  htmlFor="provider-credential"
                  optional
                  hint={
                    editing
                      ? editing.hasCredential
                        ? "A credential is stored. Leave blank to keep it, or enter a new one to replace it."
                        : "No credential is stored. The harness normally signs in itself."
                      : "The harness normally signs in itself. Stored encrypted if you add one."
                  }
                >
                  <Input
                    id="provider-credential"
                    type="password"
                    value={form.credential}
                    autoComplete="new-password"
                    onChange={(event) => setForm({ ...form, credential: event.target.value })}
                  />
                </Field>

                <Disclosure title="Models" summary="Default model and catalog">
                    <Field
                      label="Model catalog"
                      htmlFor="provider-models"
                      optional
                      hint="Comma-separated. These fill the model pickers in agent and worker forms. Leave empty to allow any model the harness defines."
                    >
                      <Input
                        id="provider-models"
                        value={form.models}
                        placeholder="deepseek-v4-pro, claude-sonnet-4"
                        onChange={(event) => setForm({ ...form, models: event.target.value })}
                      />
                    </Field>
                    <Field label="Default model" htmlFor="provider-model" optional>
                      {modelChoices.length ? (
                        <Select
                          id="provider-model"
                          value={form.model}
                          onChange={(event) => setForm({ ...form, model: event.target.value })}
                        >
                          <option value="">Harness default</option>
                          {modelChoices.map((candidate) => (
                            <option key={candidate} value={candidate}>
                              {candidate}
                            </option>
                          ))}
                        </Select>
                      ) : (
                        <Input
                          id="provider-model"
                          className="font-mono"
                          value={form.model}
                          placeholder="Harness default"
                          onChange={(event) => setForm({ ...form, model: event.target.value })}
                        />
                      )}
                    </Field>
                  </Disclosure>
              </>
            )}
          </div>

          <ResourceAccessFields ref={accessRef} resource="providers" resourceId={editing?.id} />
          <DialogFooter>
            <Button variant="outline" onClick={closeDialog}>
              Cancel
            </Button>
            <Button onClick={() => void saveProvider()} loading={save.isPending} disabled={!form.name.trim()}>
              {editing ? "Save changes" : "Add provider"}
            </Button>
          </DialogFooter>
        </DialogContent>
      </Dialog>
    </div>
  );
}
