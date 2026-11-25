import { useState } from "react";

import { Button } from "@/components/ui/button";
import { Callout } from "@/components/ui/callout";
import { ChoiceCard } from "@/components/ui/choice-card";
import { Disclosure } from "@/components/ui/disclosure";
import { Field } from "@/components/ui/field";
import { Input } from "@/components/ui/input";
import { Select } from "@/components/ui/select";
import { brandName } from "@/lib/presentation";

import { resolveProvider, type ProviderRow } from "./model-section";
import type { WorkerFormApi } from "./worker-form";

/** Name, harness and target. */
export function BasicsFields({ form, providers }: { form: WorkerFormApi; providers: ProviderRow[] }) {
  const { values, set } = form;
  const { selectedProvider } = resolveProvider(providers, values.provider, values.harness);
  return (
    <>
      <Field label="Name" htmlFor="wp-name">
        <Input id="wp-name" value={values.name} onChange={(event) => set({ name: event.target.value })} />
      </Field>
      <div className="grid gap-3 sm:grid-cols-2">
        <Field label="Harness" htmlFor="wp-harness">
          <Select
            id="wp-harness"
            value={values.harness}
            onChange={(event) => {
              const nextHarness = event.target.value;
              // An empty image lets the API choose the installation's image for this harness.
              set({ harness: nextHarness, image: "" });
              const currentConfig = (selectedProvider?.config ?? {}) as Record<string, unknown>;
              if (selectedProvider && String(currentConfig.harness ?? "") !== nextHarness) {
                set({ provider: "", model: "" });
              }
              if (nextHarness === "opencode" && !values.authSecret) {
                set({ authSecret: "orbit-opencode-auth" });
              } else if (nextHarness === "pi" && !values.authSecret) {
                set({ authSecret: "orbit-pi-auth" });
              } else if (nextHarness === "mock") {
                set({ authSecret: "" });
              }
            }}
          >
            <option value="">Choose a harness</option>
            {["pi", "opencode", "mock"].map((kind) => (
              <option key={kind} value={kind}>
                {brandName(kind)}
              </option>
            ))}
          </Select>
        </Field>
        <Field label="Target" htmlFor="wp-target" hint="Where the worker runs.">
          <Select id="wp-target" value={values.target} onChange={(event) => set({ target: event.target.value })}>
            <option value="docker">Docker</option>
            <option value="kubernetes">Kubernetes</option>
            <option value="local">Local</option>
          </Select>
        </Field>
      </div>
    </>
  );
}

/** Execution driver and image. The image is rarely changed, so it sits in a disclosure. */
export function ImageFields({ form }: { form: WorkerFormApi }) {
  const { values, set } = form;
  return (
    <>
      <Field
        label="Execution driver"
        htmlFor="wp-driver"
        hint="A long-lived process claims many executions. Docker starts a container for each one."
      >
        <Select id="wp-driver" value={values.driver} onChange={(event) => set({ driver: event.target.value })}>
          <option value="process">Process (long-lived worker)</option>
          <option value="docker">Docker (container per execution)</option>
        </Select>
      </Field>
      <Disclosure
        title="Image"
        summary={
          <span className="font-mono text-xs">{values.image || "Installation default"}</span>
        }
        defaultOpen={Boolean(values.image)}
      >
        <Field
          label="Image"
          htmlFor="wp-image"
          hint="Leave empty to use the installation's worker image for this harness. Override only for a custom image."
        >
          <Input
            id="wp-image"
            className="font-mono text-base md:text-[13px]"
            value={values.image}
            onChange={(event) => set({ image: event.target.value })}
          />
        </Field>
      </Disclosure>
    </>
  );
}

/**
 * Which image tag the worker runs: the platform's own version (relaunched automatically
 * after an upgrade) or a pinned one. Pinning an old tag says so.
 */
export function VersionFields({
  form,
  platformVersion,
}: {
  form: WorkerFormApi;
  platformVersion: string | undefined;
}) {
  const { values, set } = form;
  const { tag } = values;
  // "Pin a tag" with nothing typed yet is still pinning, so it needs its own state.
  const [pinning, setPinning] = useState(Boolean(tag));
  const outdated = Boolean(platformVersion && tag && tag !== platformVersion);
  return (
    <div className="space-y-2">
      <p id="wp-version-label" className="text-sm font-medium">
        Image version
      </p>
      <div role="radiogroup" aria-labelledby="wp-version-label" className="grid gap-2 sm:grid-cols-2">
        <ChoiceCard
          selected={!pinning}
          onSelect={() => {
            setPinning(false);
            set({ tag: "" });
          }}
          title="Follow the platform"
          description={
            platformVersion
              ? `Runs ${platformVersion} and relaunches automatically after an upgrade.`
              : "Relaunches automatically after an upgrade."
          }
        />
        <ChoiceCard
          selected={pinning}
          onSelect={() => setPinning(true)}
          title="Pin a tag"
          description="Stays on one version until you change it."
        />
      </div>
      {pinning ? (
        <Field label="Tag" htmlFor="wp-tag">
          <Input
            id="wp-tag"
            className="font-mono text-base md:text-[13px]"
            value={tag}
            placeholder={platformVersion ?? "0.21.0"}
            onChange={(event) => set({ tag: event.target.value })}
          />
        </Field>
      ) : null}
      {pinning && outdated ? (
        <Callout
          tone="warning"
          title="This tag is not the platform's version"
          action={
            <Button
              type="button"
              size="sm"
              variant="outline"
              onClick={() => {
                setPinning(false);
                set({ tag: "" });
              }}
            >
              Follow the platform
            </Button>
          }
        >
          The platform is on {platformVersion}. A worker on {tag} may not match it.
        </Callout>
      ) : null}
    </div>
  );
}
