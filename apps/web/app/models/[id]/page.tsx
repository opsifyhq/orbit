"use client";

import { useState } from "react";
import Link from "next/link";
import { useParams, useRouter } from "next/navigation";
import { useMutation, useQueryClient } from "@tanstack/react-query";
import { Download, Ellipsis, Pause, Pencil, Play, Trash2, Zap } from "lucide-react";
import { toast } from "sonner";

import { exportResource } from "@/components/export-resource-button";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { Callout } from "@/components/ui/callout";
import { Card, CardContent, CardHeader, CardTitle } from "@/components/ui/card";
import { useConfirm } from "@/components/ui/confirm";
import { DescriptionList } from "@/components/ui/description-list";
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuSeparator,
  DropdownMenuTrigger,
} from "@/components/ui/dropdown-menu";
import { EmptyState } from "@/components/ui/empty-state";
import { PageHeader } from "@/components/ui/page-header";
import { Skeleton } from "@/components/ui/skeleton";
import { Status } from "@/components/ui/status";
import {
  COMPAT_FLAGS,
  PROVIDER_STATUS,
  catalogOf,
  endpointLabel,
  kindLabel,
  plural,
  providerState,
} from "@/lib/provider-meta";
import { deleteProvider, setProviderEnabled, useProvider, useProviderTests, useTestProvider } from "@/lib/providers";
import { queryKeys } from "@/lib/queries";
import { formatDateTime, relativeTime } from "@/lib/utils";
import { errorMessage } from "@orbit/api-client";

export default function ProviderPage() {
  const params = useParams<{ id: string }>();
  const providerId = params.id;
  const router = useRouter();
  const queryClient = useQueryClient();
  const confirm = useConfirm();
  const provider = useProvider(providerId);
  const tests = useProviderTests();
  const runTest = useTestProvider();
  const [testing, setTesting] = useState(false);

  const toggle = useMutation({
    mutationFn: (enabled: boolean) => setProviderEnabled(providerId, enabled),
    onSuccess: (_result, enabled) => {
      toast.success(enabled ? "Provider enabled" : "Provider disabled");
      void queryClient.invalidateQueries({ queryKey: queryKeys.providers });
    },
    onError: (error) => toast.error(errorMessage(error)),
  });
  const remove = useMutation({
    mutationFn: () => deleteProvider(providerId),
    onSuccess: async () => {
      toast.success("Provider deleted");
      await queryClient.invalidateQueries({ queryKey: queryKeys.providers });
      router.push("/models");
    },
    onError: (error) => toast.error(errorMessage(error)),
  });

  if (provider.isLoading) {
    return (
      <div className="space-y-4">
        <Skeleton className="h-10 w-64" />
        <Skeleton className="h-96 rounded-xl" />
      </div>
    );
  }
  if (!provider.data) {
    return (
      <EmptyState
        title="Provider not found"
        description={provider.error ? errorMessage(provider.error) : "It may have been deleted, or you cannot see it."}
        action={
          <Button asChild>
            <Link href="/models">Back to model providers</Link>
          </Button>
        }
      />
    );
  }

  const data = provider.data;
  const config = (data.config ?? {}) as Record<string, unknown>;
  const compat = (config.compat ?? {}) as Record<string, boolean>;
  const catalog = catalogOf(config);
  const model = String(config.model ?? "");
  const isEndpoint = data.kind !== "harness_managed";
  const test = tests[data.id];

  async function test_() {
    setTesting(true);
    try {
      const outcome = await runTest(data.id);
      if (outcome.ok) toast.success(outcome.summary);
      else toast.error(outcome.summary);
    } finally {
      setTesting(false);
    }
  }

  async function confirmDelete() {
    const ok = await confirm({
      title: `Delete provider ${data.name}?`,
      description:
        "Agents and worker profiles that use it cannot run until you pick another provider. Its stored credential is deleted. This cannot be undone.",
      confirmLabel: "Delete provider",
      destructive: true,
    });
    if (ok) remove.mutate();
  }

  return (
    <div className="space-y-6">
      <PageHeader
        back={{ href: "/models", label: "Model providers" }}
        title={data.name}
        description={`${kindLabel(data.kind)} · ${endpointLabel(data)}`}
        meta={
          <>
            <Status meta={PROVIDER_STATUS[providerState(data, test)]} />
            <span className="text-[13px] text-muted-foreground">
              {test
                ? test.ok
                  ? `Reachable in ${test.latencyMs ?? 0} ms, tested ${relativeTime(new Date(test.at).toISOString())}`
                  : `Tested ${relativeTime(new Date(test.at).toISOString())}`
                : `Updated ${relativeTime(data.updatedAt)}`}
            </span>
          </>
        }
        actions={
          <>
            {isEndpoint ? (
              <Button variant="outline" onClick={() => void test_()} loading={testing}>
                <Zap /> Test
              </Button>
            ) : null}
            <Button asChild variant="outline">
              <Link href={`/models/${data.id}/edit`}>
                <Pencil /> Edit
              </Link>
            </Button>
            <DropdownMenu>
              <DropdownMenuTrigger asChild>
                <Button variant="outline" size="icon" aria-label="More actions">
                  <Ellipsis />
                </Button>
              </DropdownMenuTrigger>
              <DropdownMenuContent align="end">
                <DropdownMenuItem onSelect={() => toggle.mutate(!data.enabled)} disabled={toggle.isPending}>
                  {data.enabled ? (
                    <>
                      <Pause /> Disable
                    </>
                  ) : (
                    <>
                      <Play /> Enable
                    </>
                  )}
                </DropdownMenuItem>
                <DropdownMenuItem onSelect={() => void exportResource("providers", data.id, data.name)}>
                  <Download /> Export as YAML
                </DropdownMenuItem>
                <DropdownMenuSeparator />
                <DropdownMenuItem destructive onSelect={() => void confirmDelete()}>
                  <Trash2 /> Delete provider
                </DropdownMenuItem>
              </DropdownMenuContent>
            </DropdownMenu>
          </>
        }
      />

      {test && !test.ok ? (
        <Callout
          tone="danger"
          title="The last test failed"
          action={
            <Button asChild size="sm" variant="outline">
              <Link href={`/models/${data.id}/edit#connection`}>Check the connection</Link>
            </Button>
          }
        >
          <span className="break-words">{test.error ?? "The endpoint did not answer."}</span>
        </Callout>
      ) : !data.enabled ? (
        <Callout
          tone="info"
          title="This provider is disabled"
          action={
            <Button size="sm" variant="outline" onClick={() => toggle.mutate(true)} loading={toggle.isPending}>
              Enable
            </Button>
          }
        >
          Agents that use it cannot run until it is enabled again.
        </Callout>
      ) : isEndpoint && !data.hasCredential ? (
        <Callout
          tone="warning"
          title="No credential stored"
          action={
            <Button asChild size="sm" variant="outline">
              <Link href={`/models/${data.id}/edit#connection`}>Add one</Link>
            </Button>
          }
        >
          Hosted APIs refuse requests without one; a local endpoint may not need it.
        </Callout>
      ) : null}

      <div className="grid gap-6 lg:grid-cols-[minmax(0,1fr)_20rem] lg:items-start [&>*]:min-w-0">
        <div className="space-y-6">
          <Card>
            <CardHeader>
              <CardTitle>Models</CardTitle>
              <p className="text-[13px] text-muted-foreground">
                {isEndpoint
                  ? "The models agents can pick from this provider, and the one they get when they pick none."
                  : "The models agents can pick. With no list, any model the harness defines is allowed."}
              </p>
            </CardHeader>
            <CardContent className="space-y-4">
              <DescriptionList
                items={[
                  {
                    label: "Default model",
                    value: model || (isEndpoint ? "Not set" : "Harness default"),
                    mono: Boolean(model),
                  },
                ]}
              />
              {catalog.length ? (
                <div className="space-y-2">
                  <p className="text-[13px] font-medium">
                    Catalog <span className="tabular font-normal text-muted-foreground">{catalog.length}</span>
                  </p>
                  <ul className="flex flex-wrap gap-1.5">
                    {catalog.map((item) => (
                      <li key={item}>
                        <Badge variant={item === model ? "default" : "secondary"} className="font-mono">
                          {item}
                        </Badge>
                      </li>
                    ))}
                  </ul>
                </div>
              ) : (
                <p className="text-[13px] text-muted-foreground">
                  No catalog.{" "}
                  {isEndpoint ? (
                    <>
                      Agents type a model id.{" "}
                      <Link href={`/models/${data.id}/edit#models`} className="font-medium text-primary underline-offset-4 hover:underline">
                        Fetch the list
                      </Link>
                    </>
                  ) : null}
                </p>
              )}
              {test?.ok ? (
                <p className="text-xs text-muted-foreground">
                  The last test found {plural(test.modelCount ?? 0, "model", "models")} at the endpoint.
                </p>
              ) : null}
            </CardContent>
          </Card>

          {isEndpoint ? (
            <Card>
              <CardHeader>
                <CardTitle>Limits and support</CardTitle>
              </CardHeader>
              <CardContent className="space-y-4">
                <DescriptionList
                  items={[
                    {
                      label: "Context window",
                      value: config.contextWindow ? `${Number(config.contextWindow).toLocaleString()} tokens` : "Endpoint default",
                    },
                    {
                      label: "Max output",
                      value: config.maxOutputTokens ? `${Number(config.maxOutputTokens).toLocaleString()} tokens` : "Endpoint default",
                    },
                  ]}
                />
                {data.kind === "openai_compatible" ? (
                  <ul className="flex flex-wrap gap-1.5">
                    {COMPAT_FLAGS.map((flag) => {
                      const on = flag.key === "streaming" || flag.key === "modelsEndpoint" ? compat[flag.key] !== false : Boolean(compat[flag.key]);
                      return (
                        <li key={flag.key}>
                          <Badge variant={on ? "secondary" : "outline"} className={on ? undefined : "text-muted-foreground line-through"}>
                            {flag.title}
                          </Badge>
                        </li>
                      );
                    })}
                  </ul>
                ) : null}
              </CardContent>
            </Card>
          ) : null}
        </div>

        <Card>
          <CardHeader>
            <CardTitle>Details</CardTitle>
          </CardHeader>
          <CardContent>
            <DescriptionList
              items={[
                { label: "Type", value: kindLabel(data.kind) },
                isEndpoint
                  ? { label: "Base URL", value: <span className="break-all">{endpointLabel(data)}</span>, mono: true }
                  : { label: "Harness", value: endpointLabel(data) },
                {
                  label: "Credential",
                  value: data.hasCredential ? "Stored, encrypted" : isEndpoint ? "Not set" : "Harness signs in",
                },
                { label: "Created", value: formatDateTime(data.createdAt) },
                { label: "Updated", value: formatDateTime(data.updatedAt) },
              ]}
            />
          </CardContent>
        </Card>
      </div>
    </div>
  );
}
