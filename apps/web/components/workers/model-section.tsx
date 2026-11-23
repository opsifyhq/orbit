import { toast } from "sonner";

import { Button } from "@/components/ui/button";
import { Callout } from "@/components/ui/callout";
import { Field } from "@/components/ui/field";
import { Input } from "@/components/ui/input";
import { Select } from "@/components/ui/select";
import { isOrbitManagedProvider } from "@/lib/provider-meta";
import type { useProviders } from "@/lib/queries";
import {
  harnessLabel,
  harnessMismatch,
  providerOptionValue,
  resolveProvider,
} from "@/lib/worker-harness";

import type { WorkerFormApi } from "./worker-form";

export type ProviderRow = NonNullable<ReturnType<typeof useProviders>["data"]>[number];

export { providerOptionValue, resolveProvider };

/** Provider and model, from the catalogue kept under Models. */
export function ModelFields({ form, providers }: { form: WorkerFormApi; providers: ProviderRow[] }) {
  const { values, set } = form;
  const { provider, model } = values;
  const { selectedProvider, providerModels } = resolveProvider(providers, provider);
  const mismatch = harnessMismatch(values.harness, provider, providers);

  return (
    <div className="space-y-3">
      <div className="grid gap-3 sm:grid-cols-2">
        <Field
          label="Provider"
          htmlFor="wp-provider"
          hint="Harness-managed providers use the harness auth secret and need a worker on the same harness. ORBIT-managed providers get credentials injected at runtime."
        >
          {providers.length ? (
            <div className="space-y-2">
              <Select
                id="wp-provider"
                value={selectedProvider ? provider : "__custom"}
                onChange={(event) => {
                  if (event.target.value === "__custom") {
                    set({ provider: "", model: "" });
                    return;
                  }
                  const next = providers.find((candidate) => providerOptionValue(candidate) === event.target.value);
                  const config = (next?.config ?? {}) as Record<string, unknown>;
                  const settings = config.settings as Record<string, unknown> | undefined;
                  const catalog = Array.isArray(settings?.models)
                    ? (settings.models as unknown[]).filter((value): value is string => typeof value === "string")
                    : [];
                  // A harness change is never a side effect of choosing a model. The
                  // warning below offers an explicit switch instead.
                  set({ provider: event.target.value, model: String(config.model ?? "") || catalog[0] || "" });
                }}
              >
                <option value="__custom">Custom provider</option>
                {providers.some((candidate) => candidate.kind === "harness_managed") ? (
                  <optgroup label="Harness-authenticated (worker handles auth)">
                    {providers
                      .filter((candidate) => candidate.kind === "harness_managed")
                      .map((candidate) => {
                        const config = (candidate.config ?? {}) as Record<string, unknown>;
                        const optionName = providerOptionValue(candidate);
                        const providerHarness = String(config.harness ?? "");
                        return (
                          <option key={candidate.id} value={optionName}>
                            {candidate.name}
                            {providerHarness ? ` (${providerHarness})` : ""}
                          </option>
                        );
                      })}
                  </optgroup>
                ) : null}
                {providers.some((candidate) => isOrbitManagedProvider(candidate.kind)) ? (
                  <optgroup label="ORBIT-managed (API key stored in ORBIT)">
                    {providers
                      .filter((candidate) => isOrbitManagedProvider(candidate.kind))
                      .map((candidate) => (
                        <option key={candidate.id} value={providerOptionValue(candidate)}>
                          {candidate.name}
                        </option>
                      ))}
                  </optgroup>
                ) : null}
              </Select>
              {selectedProvider ? null : (
                <Input
                  value={provider}
                  placeholder="Custom provider name"
                  aria-label="Custom provider name"
                  onChange={(event) => set({ provider: event.target.value })}
                />
              )}
            </div>
          ) : (
            <Input
              id="wp-provider"
              value={provider}
              placeholder="opencode-go"
              onChange={(event) => set({ provider: event.target.value })}
            />
          )}
        </Field>
        <Field
          label="Model"
          htmlFor="wp-model"
          hint={
            selectedProvider
              ? "Defaults to the provider's model. Override if needed."
              : "Free-form model ID for custom or harness-default providers."
          }
        >
          {providerModels.length ? (
            <Select id="wp-model" value={model} onChange={(event) => set({ model: event.target.value })}>
              <option value="">Provider default</option>
              {model && !providerModels.includes(model) ? <option value={model}>{model} (custom)</option> : null}
              {providerModels.map((candidate) => (
                <option key={candidate} value={candidate}>
                  {candidate}
                </option>
              ))}
            </Select>
          ) : (
            <Input id="wp-model" value={model} onChange={(event) => set({ model: event.target.value })} />
          )}
        </Field>
      </div>
      {mismatch ? (
        <Callout
          tone="warning"
          title={mismatch.title}
          action={
            <Button
              type="button"
              size="sm"
              variant="outline"
              onClick={() => {
                // Let the API fill the installation's image for the new harness.
                set({ harness: mismatch.providerHarness, image: "" });
                toast.success(`Worker switched to ${harnessLabel(mismatch.providerHarness)}`);
              }}
            >
              Switch to {harnessLabel(mismatch.providerHarness)} worker
            </Button>
          }
        >
          This provider is managed by {harnessLabel(mismatch.providerHarness)}, so the worker and its image stay as
          they are until you switch. Saving is blocked while the harnesses differ.
        </Callout>
      ) : null}
    </div>
  );
}

/** The secret a harness-managed provider authenticates with. */
export function AuthSecretField({ form }: { form: WorkerFormApi }) {
  return (
    <Field
      label="Harness auth secret"
      htmlFor="wp-auth"
      optional
      hint="Only for providers signed in through the harness. Leave blank for ORBIT-managed providers; their credentials are injected at runtime."
    >
      <Input
        id="wp-auth"
        className="font-mono text-base md:text-[13px]"
        value={form.values.authSecret}
        onChange={(event) => form.set({ authSecret: event.target.value })}
        placeholder="orbit-opencode-auth"
      />
    </Field>
  );
}
