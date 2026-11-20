import Link from "next/link";
import { Plus, X } from "lucide-react";

import { Button } from "@/components/ui/button";
import { Field } from "@/components/ui/field";
import { Input } from "@/components/ui/input";
import { Select } from "@/components/ui/select";
import { Skeleton } from "@/components/ui/skeleton";
import { brandName } from "@/lib/presentation";

import { PickList } from "./shared";
import type { InlineMapping, WorkerFormApi } from "./worker-form";

type Connection = { id: string; name: string; connectorSlug: string };
type ReusableMapping = { id: string; name: string; description?: string | null };

/** Credentials the worker injects as environment variables: inline and reusable mappings. */
export function EnvironmentFields({
  form,
  connections,
  library,
  libraryLoading,
}: {
  form: WorkerFormApi;
  connections: Connection[];
  library: ReusableMapping[] | undefined;
  libraryLoading: boolean;
}) {
  const { values, set } = form;
  const { envMappings } = values;
  const setMappings = (update: (current: InlineMapping[]) => InlineMapping[]) =>
    set({ envMappings: update(envMappings) });
  const patchMapping = (index: number, patch: Partial<InlineMapping>) =>
    setMappings((current) => current.map((m, i) => (i === index ? { ...m, ...patch } : m)));

  return (
    <>
      <div className="space-y-2">
        <div className="flex flex-wrap items-center justify-between gap-2">
          <p className="text-sm font-medium">Inline mappings</p>
          <Button
            type="button"
            size="sm"
            variant="outline"
            onClick={() => setMappings((current) => [...current, { envVar: "", connectionId: "", secretKey: "" }])}
          >
            <Plus /> Add mapping
          </Button>
        </div>
        {envMappings.length === 0 ? (
          <p className="rounded-lg border border-dashed p-3 text-[13px] text-muted-foreground">
            No inline mappings. Add one to inject a connection secret as an environment variable.
          </p>
        ) : (
          <ul className="space-y-2">
            {envMappings.map((mapping, index) => (
              <li key={index} className="rounded-lg border bg-card p-3">
                <div className="grid gap-3 sm:grid-cols-[minmax(0,1fr)_minmax(0,1fr)_minmax(0,1fr)_auto] sm:items-end">
                  <Field label="Variable" htmlFor={`wp-env-${index}-var`}>
                    <Input
                      id={`wp-env-${index}-var`}
                      value={mapping.envVar}
                      onChange={(e) => patchMapping(index, { envVar: e.target.value })}
                      placeholder="ENV_VAR_NAME"
                      className="font-mono text-base md:text-[13px]"
                    />
                  </Field>
                  <Field label="Connection" htmlFor={`wp-env-${index}-conn`}>
                    <Select
                      id={`wp-env-${index}-conn`}
                      value={mapping.connectionId}
                      onChange={(e) => patchMapping(index, { connectionId: e.target.value })}
                    >
                      <option value="">Select a connection</option>
                      {connections.map((c) => (
                        <option key={c.id} value={c.id}>
                          {c.name} ({brandName(c.connectorSlug)})
                        </option>
                      ))}
                    </Select>
                  </Field>
                  <Field label="Secret key" htmlFor={`wp-env-${index}-key`}>
                    <Input
                      id={`wp-env-${index}-key`}
                      value={mapping.secretKey}
                      onChange={(e) => patchMapping(index, { secretKey: e.target.value })}
                      placeholder="password"
                    />
                  </Field>
                  <Button
                    type="button"
                    size="icon"
                    variant="ghost"
                    className="justify-self-start sm:justify-self-auto"
                    aria-label={`Remove mapping ${mapping.envVar || index + 1}`}
                    onClick={() => setMappings((current) => current.filter((_, i) => i !== index))}
                  >
                    <X />
                  </Button>
                </div>
              </li>
            ))}
          </ul>
        )}
      </div>

      <div className="space-y-2">
        <div className="flex flex-wrap items-center justify-between gap-2">
          <p id="wp-reusable-mappings" className="text-sm font-medium">
            Reusable mappings
          </p>
          <Link href="/environment-mappings" className="text-[13px] text-primary hover:underline">
            Manage mappings
          </Link>
        </div>
        {libraryLoading ? (
          <Skeleton className="h-16" />
        ) : !library?.length ? (
          <p className="rounded-lg border border-dashed p-3 text-[13px] text-muted-foreground">
            No reusable mappings yet. Create them under Environments and attach them to any profile.
          </p>
        ) : (
          <PickList
            label="reusable mappings"
            idPrefix="wp-envmap"
            selected={values.envMappingIds}
            onChange={(envMappingIds) => set({ envMappingIds })}
            items={library.map((mapping) => ({
              id: mapping.id,
              label: mapping.name,
              description: mapping.description || undefined,
            }))}
            empty="No reusable mappings."
          />
        )}
      </div>
    </>
  );
}
