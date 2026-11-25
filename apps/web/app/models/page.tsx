"use client";

import { useMemo, useState } from "react";
import Link from "next/link";
import { useQueryClient } from "@tanstack/react-query";
import { Boxes, Plus } from "lucide-react";

import { BulkAccessDialog } from "@/components/bulk-access-dialog";
import { ImportPortableButton } from "@/components/import-portable-dialog";
import { Button } from "@/components/ui/button";
import { Callout } from "@/components/ui/callout";
import { Checkbox } from "@/components/ui/checkbox";
import { EmptyState } from "@/components/ui/empty-state";
import { PageHeader } from "@/components/ui/page-header";
import {
  ListCard,
  ListCount,
  ListRow,
  ListSkeleton,
  ListToolbar,
  ROW_LINK,
  SearchField,
  SelectionBar,
  toggleId,
  toggleShown,
} from "@/components/ui/resource-list";
import { Segmented } from "@/components/ui/segmented";
import { Status } from "@/components/ui/status";
import {
  PROVIDER_STATUS,
  catalogOf,
  endpointLabel,
  kindLabel,
  plural,
  providerState,
  type ProviderKind,
} from "@/lib/provider-meta";
import { useProviderTests } from "@/lib/providers";
import { queryKeys, useProviders } from "@/lib/queries";
import type { ProviderView } from "@/lib/types";
import { relativeTime } from "@/lib/utils";
import { errorMessage } from "@orbit/api-client";

type Filter = "all" | ProviderKind;

const FILTER_LABEL: Record<Filter, string> = {
  all: "All",
  openai_compatible: "OpenAI-compatible",
  anthropic: "Anthropic",
  harness_managed: "Harness-managed",
};

function modelOf(provider: ProviderView): string {
  return String(((provider.config ?? {}) as Record<string, unknown>).model ?? "");
}

export default function ModelsPage() {
  const providers = useProviders();
  const tests = useProviderTests();
  const queryClient = useQueryClient();
  const [search, setSearch] = useState("");
  const [filter, setFilter] = useState<Filter>("all");
  const [selected, setSelected] = useState<Set<string>>(new Set());
  const [bulkOpen, setBulkOpen] = useState(false);

  const all = useMemo(() => providers.data ?? [], [providers.data]);
  const counts = useMemo(() => {
    const result: Record<Filter, number> = { all: all.length, openai_compatible: 0, anthropic: 0, harness_managed: 0 };
    for (const provider of all) if (provider.kind in result) result[provider.kind as ProviderKind] += 1;
    return result;
  }, [all]);
  const visible = useMemo(() => {
    const terms = search.toLowerCase().split(/\s+/).filter(Boolean);
    return all.filter((provider) => {
      if (filter !== "all" && provider.kind !== filter) return false;
      const haystack = [
        provider.name,
        kindLabel(provider.kind),
        endpointLabel(provider),
        modelOf(provider),
        ...catalogOf(provider.config),
      ]
        .join(" ")
        .toLowerCase();
      return terms.every((term) => haystack.includes(term));
    });
  }, [all, filter, search]);

  return (
    <div className="space-y-6">
      <PageHeader
        title="Model providers"
        description="Where agents get their models: an OpenAI-compatible endpoint, the native Anthropic API, or a provider a harness manages. Credentials are encrypted and never shown again."
        actions={
          <div className="flex flex-wrap gap-2">
            <ImportPortableButton />
            <Button asChild>
              <Link href="/models/new">
                <Plus /> Add provider
              </Link>
            </Button>
          </div>
        }
      />

      <ListToolbar>
        <SearchField value={search} onChange={setSearch} placeholder="Filter by name, URL or model" label="Filter providers" />
        <Segmented
          label="Provider type"
          value={filter}
          onChange={setFilter}
          options={(Object.keys(FILTER_LABEL) as Filter[]).map((value) => ({
            value,
            label: FILTER_LABEL[value],
            count: providers.data ? counts[value] : undefined,
          }))}
        />
      </ListToolbar>

      <SelectionBar
        count={selected.size}
        noun="providers"
        allShownSelected={visible.length > 0 && visible.every((provider) => selected.has(provider.id))}
        onSelectAllShown={(checked) => setSelected((current) => toggleShown(current, visible.map((provider) => provider.id), checked))}
        onClear={() => setSelected(new Set())}
      >
        <Button size="sm" variant="outline" onClick={() => setBulkOpen(true)}>
          Change access
        </Button>
      </SelectionBar>

      <BulkAccessDialog
        open={bulkOpen}
        onOpenChange={setBulkOpen}
        resource="providers"
        ids={[...selected]}
        onDone={() => {
          setSelected(new Set());
          void queryClient.invalidateQueries({ queryKey: queryKeys.providers });
        }}
      />

      {providers.isLoading ? (
        <ListSkeleton />
      ) : providers.isError ? (
        <Callout tone="danger" title="Providers could not be loaded">
          {errorMessage(providers.error)}
        </Callout>
      ) : visible.length ? (
        <>
          <ListCard columns={["Provider", "Status", "Models", "Updated"]}>
            {visible.map((provider) => {
              const isEndpoint = provider.kind !== "harness_managed";
              const model = modelOf(provider);
              const catalog = catalogOf(provider.config);
              const test = tests[provider.id];
              const modelText = model || (isEndpoint ? "No default model" : "Harness default");
              const credential = provider.hasCredential
                ? "Credential stored"
                : isEndpoint
                  ? "No credential"
                  : "Harness signs in";
              return (
                <ListRow
                  key={provider.id}
                  checkbox={
                    <Checkbox
                      aria-label={`Select ${provider.name}`}
                      checked={selected.has(provider.id)}
                      onChange={(event) => setSelected((current) => toggleId(current, provider.id, event.target.checked))}
                    />
                  }
                  name={
                    <>
                      <p className="flex items-center gap-2">
                        <Link href={`/models/${provider.id}`} className={ROW_LINK}>
                          {provider.name}
                        </Link>
                        <span className="hidden shrink-0 text-xs text-muted-foreground sm:inline">{kindLabel(provider.kind)}</span>
                      </p>
                      <p
                        className={
                          isEndpoint
                            ? "mt-0.5 truncate font-mono text-xs text-muted-foreground"
                            : "mt-0.5 truncate text-[13px] text-muted-foreground"
                        }
                        title={endpointLabel(provider)}
                      >
                        {test && !test.ok ? <span className="font-sans text-[13px] text-destructive">{test.error}</span> : endpointLabel(provider)}
                      </p>
                      <p className="mt-1.5 flex flex-wrap items-center gap-x-3 gap-y-1 text-xs text-muted-foreground [@container(min-width:60rem)]:hidden">
                        <span className="sm:hidden">{kindLabel(provider.kind)}</span>
                        <span className="max-w-full truncate font-mono">{modelText}</span>
                        <span>{plural(catalog.length, "model", "models")}</span>
                        <span>{relativeTime(provider.updatedAt)}</span>
                      </p>
                    </>
                  }
                  status={<Status meta={PROVIDER_STATUS[providerState(provider, test)]} />}
                  details={
                    <>
                      <p className={model ? "truncate font-mono text-xs" : "truncate"} title={model || undefined}>
                        {modelText}
                      </p>
                      <p
                        className={
                          isEndpoint && !provider.hasCredential
                            ? "mt-0.5 truncate text-xs text-warning"
                            : "mt-0.5 truncate text-xs text-muted-foreground"
                        }
                      >
                        {plural(catalog.length, "model", "models")} · {credential}
                      </p>
                    </>
                  }
                  updated={relativeTime(provider.updatedAt)}
                />
              );
            })}
          </ListCard>
          <ListCount
            shown={visible.length}
            total={counts[filter]}
            singular="provider"
            plural="providers"
            note={Object.keys(tests).length ? "Test results are kept until you reload." : undefined}
          />
        </>
      ) : (
        <EmptyState
          icon={<Boxes />}
          title={
            search
              ? `No providers match “${search}”`
              : all.length === 0
                ? "No model providers yet"
                : `No ${FILTER_LABEL[filter]} providers`
          }
          description={
            search
              ? "Try a shorter term, or clear the filter to see every provider."
              : all.length === 0
                ? "Add vLLM, LiteLLM, an API gateway or any OpenAI-compatible endpoint, the Anthropic API, or a provider your harness signs in to."
                : "Providers of another type are one tap away in the filter."
          }
          action={
            search ? (
              <Button variant="outline" onClick={() => setSearch("")}>
                Clear filter
              </Button>
            ) : all.length === 0 ? (
              <Button asChild variant="outline">
                <Link href="/models/new">
                  <Plus /> Add provider
                </Link>
              </Button>
            ) : (
              <Button variant="outline" onClick={() => setFilter("all")}>
                Show all providers
              </Button>
            )
          }
        />
      )}
    </div>
  );
}
