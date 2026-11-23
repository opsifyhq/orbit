/**
 * Pure helpers for the worker editor's harness/provider relationship. Kept out of the
 * React components so `node --test` can verify that a mismatch is detected and named.
 */
export type WorkerProviderLike = {
  name: string;
  kind: string;
  config: unknown;
};

const HARNESS_LABELS: Record<string, string> = {
  opencode: "OpenCode",
  pi: "Pi",
  mock: "Mock",
  claude_code: "Claude Code",
};

const asRecord = (value: unknown): Record<string, unknown> =>
  value && typeof value === "object" && !Array.isArray(value) ? (value as Record<string, unknown>) : {};

/** Product names, matching the rest of the web app. Unknown harnesses get a readable fallback. */
export function harnessLabel(harness: string): string {
  if (!harness) return "a harness";
  return (
    HARNESS_LABELS[harness] ??
    harness
      .replace(/[_-]+/g, " ")
      .replace(/\b\w/g, (char) => char.toUpperCase())
      .trim()
  );
}

function withArticle(label: string): string {
  return `${/^[aeiou]/i.test(label) ? "an" : "a"} ${label}`;
}

/** The value a provider is stored under on a worker profile. */
export function providerOptionValue(provider: WorkerProviderLike): string {
  const config = asRecord(provider.config);
  return provider.kind === "harness_managed" ? String(config.provider ?? provider.name) : provider.name;
}

/** The provider row matching a profile's provider, and the models it offers. */
export function resolveProvider<T extends WorkerProviderLike>(
  providers: T[],
  provider: string,
): { selectedProvider: T | undefined; providerModels: string[] } {
  const selectedProvider = providers.find((candidate) => providerOptionValue(candidate) === provider);
  const settings = asRecord(asRecord(selectedProvider?.config).settings);
  const providerModels = Array.isArray(settings.models)
    ? settings.models.filter((item): item is string => typeof item === "string")
    : [];
  return { selectedProvider, providerModels };
}

/** The harness a harness-managed provider runs under, or null for other provider kinds. */
export function providerHarness(provider: WorkerProviderLike | undefined): string | null {
  if (!provider || provider.kind !== "harness_managed") return null;
  return String(asRecord(provider.config).harness ?? "") || null;
}

export type HarnessMismatch = {
  workerHarness: string;
  providerHarness: string;
  /** Short title for the warning callout, naming both harnesses. */
  title: string;
  /** Full message used as a blocking editor issue. */
  message: string;
};

/**
 * A harness-managed provider only works with a worker on the same harness. Returning a
 * mismatch lets the UI warn before anything changes and blocks the save until it is fixed.
 */
export function harnessMismatch(
  workerHarness: string,
  provider: string,
  providers: WorkerProviderLike[],
): HarnessMismatch | null {
  const selected = resolveProvider(providers, provider).selectedProvider;
  const modelHarness = providerHarness(selected);
  if (!workerHarness || !modelHarness || modelHarness === workerHarness) return null;

  const workerLabel = harnessLabel(workerHarness);
  const modelLabel = harnessLabel(modelHarness);
  const title = `${workerLabel} worker with ${withArticle(modelLabel)}-harness model`;
  return {
    workerHarness,
    providerHarness: modelHarness,
    title,
    message: `${title}. Switch the worker to ${modelLabel} or choose a ${workerLabel}-harness provider before saving.`,
  };
}
