"use client";

import { useMemo, useState } from "react";
import Link from "next/link";
import { useQueryClient } from "@tanstack/react-query";
import { Cable, Plus, Webhook } from "lucide-react";

import { BulkAccessDialog } from "@/components/bulk-access-dialog";
import { ImportPortableButton } from "@/components/import-portable-dialog";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { Callout } from "@/components/ui/callout";
import { Card } from "@/components/ui/card";
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
import { Section } from "@/components/ui/section";
import { Segmented } from "@/components/ui/segmented";
import { Skeleton } from "@/components/ui/skeleton";
import { Status } from "@/components/ui/status";
import { connectionHealth, connectionStatus, type Capability } from "@/lib/connections";
import { capabilityLabel, effectTone, humanizeIdentifier } from "@/lib/presentation";
import { queryKeys, useConnections, useConnectors } from "@/lib/queries";
import type { ConnectionView } from "@/lib/types";
import { relativeTime } from "@/lib/utils";
import { errorMessage } from "@orbit/api-client";

type Filter = "all" | "connected" | "error" | "unchecked";

const FILTER_LABEL: Record<Filter, string> = {
  all: "All",
  connected: "Connected",
  error: "Check failed",
  unchecked: "Not checked",
};

function inFilter(connection: ConnectionView, filter: Filter) {
  return filter === "all" || connectionHealth(connection) === filter;
}

export default function ConnectionsPage() {
  const connections = useConnections();
  const connectors = useConnectors();
  const queryClient = useQueryClient();
  const [filter, setFilter] = useState<Filter>("all");
  const [search, setSearch] = useState("");
  const [selected, setSelected] = useState<Set<string>>(new Set());
  const [bulkOpen, setBulkOpen] = useState(false);

  // The effect of every capability, so scopes can be counted by what they can do.
  const effectBySlug = useMemo(() => {
    const map = new Map<string, Map<string, string>>();
    for (const connector of connectors.data ?? []) {
      const caps = (connector.capabilities ?? []) as Capability[];
      map.set(connector.slug, new Map(caps.map((cap) => [cap.id, cap.effect])));
    }
    return map;
  }, [connectors.data]);

  const all = useMemo(() => connections.data ?? [], [connections.data]);
  const counts = useMemo(() => {
    const result: Record<Filter, number> = { all: 0, connected: 0, error: 0, unchecked: 0 };
    for (const connection of all) {
      for (const key of Object.keys(result) as Filter[]) if (inFilter(connection, key)) result[key] += 1;
    }
    return result;
  }, [all]);
  const visible = useMemo(() => {
    const terms = search.toLowerCase().split(/\s+/).filter(Boolean);
    return all.filter((connection) => {
      if (!inFilter(connection, filter)) return false;
      const haystack = `${connection.name} ${connection.connectorName} ${connection.scopes.map(capabilityLabel).join(" ")}`.toLowerCase();
      return terms.every((term) => haystack.includes(term));
    });
  }, [all, filter, search]);

  const connectedCount = useMemo(() => {
    const result = new Map<string, number>();
    for (const connection of all) result.set(connection.connectorSlug, (result.get(connection.connectorSlug) ?? 0) + 1);
    return result;
  }, [all]);

  const scopeSummary = (connection: ConnectionView) => {
    const effects = effectBySlug.get(connection.connectorSlug);
    const tally = { safe: 0, write: 0, external: 0 };
    for (const scope of connection.scopes) tally[effectTone(effects?.get(scope))] += 1;
    return [
      tally.safe ? `${tally.safe} read` : null,
      tally.write ? `${tally.write} change` : null,
      tally.external ? `${tally.external} act outside` : null,
    ]
      .filter(Boolean)
      .join(" · ");
  };

  return (
    <div className="space-y-6">
      <PageHeader
        title="Connectors"
        description="Connections are accounts you have authorised for external services. Agents use them only for the capabilities you allow, and ORBIT enforces that."
        actions={
          <div className="flex flex-wrap gap-2">
            <ImportPortableButton />
            <Button asChild>
              <Link href="/connections/new">
                <Plus /> New connection
              </Link>
            </Button>
          </div>
        }
      />

      <ListToolbar>
        <SearchField
          value={search}
          onChange={setSearch}
          placeholder="Filter by name, service or capability"
          label="Filter connections"
        />
        <Segmented
          label="Connection status"
          value={filter}
          onChange={setFilter}
          options={(Object.keys(FILTER_LABEL) as Filter[]).map((value) => ({
            value,
            label: FILTER_LABEL[value],
            count: connections.data ? counts[value] : undefined,
          }))}
        />
      </ListToolbar>

      <SelectionBar
        count={selected.size}
        noun="connections"
        allShownSelected={visible.length > 0 && visible.every((connection) => selected.has(connection.id))}
        onSelectAllShown={(checked) =>
          setSelected((current) => toggleShown(current, visible.map((connection) => connection.id), checked))
        }
        onClear={() => setSelected(new Set())}
      >
        <Button size="sm" variant="outline" onClick={() => setBulkOpen(true)}>
          Change access
        </Button>
      </SelectionBar>

      <BulkAccessDialog
        open={bulkOpen}
        onOpenChange={setBulkOpen}
        resource="connections"
        ids={[...selected]}
        onDone={() => {
          setSelected(new Set());
          void queryClient.invalidateQueries({ queryKey: queryKeys.connections });
        }}
      />

      {connections.isLoading ? (
        <ListSkeleton />
      ) : connections.isError ? (
        <Callout tone="danger" title="Connections could not be loaded">
          {errorMessage(connections.error)}
        </Callout>
      ) : visible.length ? (
        <>
          <ListCard columns={["Connection", "Status", "Capabilities", "Checked"]}>
            {visible.map((connection) => {
              const summary = scopeSummary(connection);
              return (
                <ListRow
                  key={connection.id}
                  checkbox={
                    <Checkbox
                      aria-label={`Select ${connection.name}`}
                      checked={selected.has(connection.id)}
                      onChange={(event) => setSelected((current) => toggleId(current, connection.id, event.target.checked))}
                    />
                  }
                  name={
                    <>
                      <p className="flex items-center gap-2">
                        <Link href={`/connections/${connection.id}`} className={ROW_LINK}>
                          {connection.name}
                        </Link>
                        <span className="hidden shrink-0 text-xs text-muted-foreground sm:inline">{connection.connectorName}</span>
                      </p>
                      <p className="mt-0.5 line-clamp-1 text-[13px] text-muted-foreground">
                        {connection.scopes.length
                          ? connection.scopes.map(capabilityLabel).join(", ")
                          : "No capabilities allowed yet"}
                      </p>
                      <p className="mt-1.5 flex flex-wrap items-center gap-x-3 gap-y-1 text-xs text-muted-foreground md:hidden">
                        <span className="sm:hidden">{connection.connectorName}</span>
                        <span>{summary || "Nothing allowed"}</span>
                        <span>{connection.hasCredential ? "Credential stored" : "No credential"}</span>
                        <span>{connection.lastCheckedAt ? `Checked ${relativeTime(connection.lastCheckedAt)}` : "Never checked"}</span>
                      </p>
                    </>
                  }
                  status={<Status meta={connectionStatus(connection)} />}
                  details={
                    <>
                      <p className="truncate">{summary || "Nothing allowed"}</p>
                      <p className={connection.hasCredential ? "mt-0.5 text-xs text-muted-foreground" : "mt-0.5 text-xs text-warning"}>
                        {connection.hasCredential ? "Credential stored" : "No credential"}
                      </p>
                    </>
                  }
                  updated={connection.lastCheckedAt ? relativeTime(connection.lastCheckedAt) : "Never"}
                />
              );
            })}
          </ListCard>
          <ListCount shown={visible.length} total={counts[filter]} singular="connection" plural="connections" />
        </>
      ) : (
        <EmptyState
          icon={<Cable />}
          title={
            search
              ? `No connections match “${search}”`
              : all.length === 0
                ? "No connections yet"
                : `No connections with status “${FILTER_LABEL[filter].toLowerCase()}”`
          }
          description={
            search
              ? "Try a shorter term, or clear the filter to see every connection."
              : all.length === 0
                ? "Connect a service below, such as a GitHub or GitLab account, a Telegram bot or a webhook. Then allow agents specific capabilities."
                : "Connections with another status are one tap away in the filter."
          }
          action={
            search ? (
              <Button variant="outline" onClick={() => setSearch("")}>
                Clear filter
              </Button>
            ) : all.length === 0 ? (
              <Button asChild variant="outline">
                <Link href="/connections/new">
                  <Plus /> New connection
                </Link>
              </Button>
            ) : (
              <Button variant="outline" onClick={() => setFilter("all")}>
                Show all connections
              </Button>
            )
          }
        />
      )}

      <Section
        title="Connect a service"
        description="The services ORBIT can connect to. Capabilities are counted by what they can do: read only, change data, or act outside ORBIT."
      >
        {connectors.isLoading ? (
          <div className="grid gap-3 sm:grid-cols-2 lg:grid-cols-3">
            {Array.from({ length: 3 }).map((_, index) => (
              <Skeleton key={index} className="h-40 rounded-xl" />
            ))}
          </div>
        ) : connectors.isError ? (
          <Callout tone="danger" title="The connector catalogue could not be loaded">
            {errorMessage(connectors.error)}
          </Callout>
        ) : (
          <div className="grid gap-3 sm:grid-cols-2 lg:grid-cols-3">
            {(connectors.data ?? []).map((connector) => {
              const caps = (connector.capabilities ?? []) as Capability[];
              const auth = (connector.auth ?? []) as string[];
              const tally = { safe: 0, write: 0, external: 0 };
              for (const cap of caps) tally[effectTone(cap.effect)] += 1;
              const existing = connectedCount.get(connector.slug) ?? 0;
              return (
                <Card key={connector.id} className="flex flex-col gap-3 p-4">
                  <div className="space-y-1">
                    <div className="flex items-start justify-between gap-2">
                      <h3 className="text-[15px] font-semibold">{connector.name}</h3>
                      {connector.webhooks ? (
                        <Badge variant="outline">
                          <Webhook className="h-3 w-3" aria-hidden /> Webhooks
                        </Badge>
                      ) : null}
                    </div>
                    <p className="text-[13px] text-muted-foreground">{connector.description}</p>
                  </div>
                  <div className="flex flex-wrap gap-1.5">
                    {tally.safe ? <Badge variant="secondary">{tally.safe} read only</Badge> : null}
                    {tally.write ? <Badge variant="warning">{tally.write} change data</Badge> : null}
                    {tally.external ? <Badge variant="warning">{tally.external} act outside ORBIT</Badge> : null}
                    {auth.map((method) => (
                      <Badge key={method} variant="outline">
                        {humanizeIdentifier(method)}
                      </Badge>
                    ))}
                  </div>
                  <div className="mt-auto flex items-center justify-between gap-2 pt-1">
                    <span className="text-[13px] text-muted-foreground">
                      {existing ? `${existing} connected` : "Not connected"}
                    </span>
                    <Button asChild size="sm" variant="outline" aria-label={`Connect ${connector.name}`}>
                      <Link href={`/connections/new?connector=${connector.slug}`}>
                        <Plus /> Connect
                      </Link>
                    </Button>
                  </div>
                </Card>
              );
            })}
          </div>
        )}
      </Section>
    </div>
  );
}
