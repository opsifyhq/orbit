"use client";

import { useMemo, useState } from "react";
import Link from "next/link";
import { useQueryClient } from "@tanstack/react-query";
import { KeyRound, Plus } from "lucide-react";

import { BulkAccessDialog } from "@/components/bulk-access-dialog";
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
import { ENV_STATUS, brokenVariables, envState, variableSummary, type EnvState } from "@/lib/env-mapping-meta";
import { useEnvMappingUsage } from "@/lib/env-mappings";
import { useConnections, useEnvMappings } from "@/lib/queries";
import { relativeTime } from "@/lib/utils";
import { errorMessage } from "@orbit/api-client";

type Filter = "all" | EnvState;

const FILTER_LABEL: Record<Filter, string> = {
  all: "All",
  in_use: "In use",
  unused: "Unused",
  attention: "Needs attention",
};

export default function EnvironmentsPage() {
  const mappings = useEnvMappings();
  const connections = useConnections();
  const usage = useEnvMappingUsage();
  const queryClient = useQueryClient();
  const [search, setSearch] = useState("");
  const [filter, setFilter] = useState<Filter>("all");
  const [selected, setSelected] = useState<Set<string>>(new Set());
  const [bulkOpen, setBulkOpen] = useState(false);

  const connectionById = useMemo(
    () => (connections.isSuccess ? new Map((connections.data ?? []).map((connection) => [connection.id, connection])) : null),
    [connections.isSuccess, connections.data],
  );
  const all = useMemo(() => mappings.data ?? [], [mappings.data]);
  const rows = useMemo(
    () =>
      all.map((mapping) => {
        const users = usage.usedBy.get(mapping.id) ?? [];
        const broken = brokenVariables(mapping, connectionById);
        return { mapping, users, broken, state: envState(broken.length, users.length) };
      }),
    [all, usage.usedBy, connectionById],
  );
  // Status needs both answers; until then only "All" can be counted.
  const statusKnown = usage.known && Boolean(connectionById);
  const counts = useMemo(() => {
    const result: Record<Filter, number> = { all: rows.length, in_use: 0, unused: 0, attention: 0 };
    for (const row of rows) result[row.state] += 1;
    return result;
  }, [rows]);
  const visible = useMemo(() => {
    const terms = search.toLowerCase().split(/\s+/).filter(Boolean);
    return rows.filter(({ mapping, state }) => {
      if (filter !== "all" && state !== filter) return false;
      const haystack = [
        mapping.name,
        mapping.description,
        ...mapping.mappings.map((entry) => entry.envVar),
        ...mapping.mappings.map((entry) => connectionById?.get(entry.connectionId ?? "")?.name ?? ""),
      ]
        .join(" ")
        .toLowerCase();
      return terms.every((term) => haystack.includes(term));
    });
  }, [rows, filter, search, connectionById]);

  return (
    <div className="space-y-6">
      <PageHeader
        title="Environments"
        description="Sets of environment variables for worker processes. Each value comes from a connection's credential or a stored secret, so nothing is typed in plain text. Build a set once and attach it to any worker profile."
        actions={
          <Button asChild>
            <Link href="/environments/new">
              <Plus /> New environment
            </Link>
          </Button>
        }
      />

      <ListToolbar>
        <SearchField value={search} onChange={setSearch} placeholder="Filter by name, variable or connection" label="Filter environments" />
        <Segmented
          label="Environment status"
          value={filter}
          onChange={setFilter}
          options={(Object.keys(FILTER_LABEL) as Filter[]).map((value) => ({
            value,
            label: FILTER_LABEL[value],
            count: mappings.data && (value === "all" || statusKnown) ? counts[value] : undefined,
          }))}
        />
      </ListToolbar>

      <SelectionBar
        count={selected.size}
        noun="environments"
        allShownSelected={visible.length > 0 && visible.every(({ mapping }) => selected.has(mapping.id))}
        onSelectAllShown={(checked) => setSelected((current) => toggleShown(current, visible.map(({ mapping }) => mapping.id), checked))}
        onClear={() => setSelected(new Set())}
      >
        <Button size="sm" variant="outline" onClick={() => setBulkOpen(true)}>
          Change access
        </Button>
      </SelectionBar>

      <BulkAccessDialog
        open={bulkOpen}
        onOpenChange={setBulkOpen}
        resource="env_mappings"
        ids={[...selected]}
        onDone={() => {
          setSelected(new Set());
          void queryClient.invalidateQueries({ queryKey: ["env-mappings"] });
        }}
      />

      {mappings.isLoading ? (
        <ListSkeleton />
      ) : mappings.isError ? (
        <Callout tone="danger" title="Environments could not be loaded">
          {errorMessage(mappings.error)}
        </Callout>
      ) : visible.length ? (
        <>
          <ListCard columns={["Environment", "Status", "Used by", "Updated"]}>
            {visible.map(({ mapping, users, broken, state }) => {
              const count = mapping.mappings.length;
              const usedByText = !usage.known
                ? "Checking"
                : users.length
                  ? users.map((user) => user.name).join(", ")
                  : "No worker profile";
              return (
                <ListRow
                  key={mapping.id}
                  checkbox={
                    <Checkbox
                      aria-label={`Select ${mapping.name}`}
                      checked={selected.has(mapping.id)}
                      onChange={(event) => setSelected((current) => toggleId(current, mapping.id, event.target.checked))}
                    />
                  }
                  name={
                    <>
                      <p className="flex items-center gap-2">
                        <Link href={`/environments/${mapping.id}`} className={ROW_LINK}>
                          {mapping.name}
                        </Link>
                        <span className="tabular hidden shrink-0 text-xs text-muted-foreground sm:inline">
                          {count} {count === 1 ? "variable" : "variables"}
                        </span>
                      </p>
                      <p className="mt-0.5 line-clamp-1 font-mono text-xs text-muted-foreground" title={mapping.mappings.map((entry) => entry.envVar).join(", ")}>
                        {variableSummary(mapping)}
                      </p>
                      <p className="mt-1.5 flex flex-wrap items-center gap-x-3 gap-y-1 text-xs text-muted-foreground [@container(min-width:60rem)]:hidden">
                        <span className="tabular sm:hidden">
                          {count} {count === 1 ? "variable" : "variables"}
                        </span>
                        <span>{usage.known ? (users.length ? `Used by ${users.length}` : "Not attached") : "Checking"}</span>
                        <span>{relativeTime(mapping.updatedAt)}</span>
                      </p>
                    </>
                  }
                  status={statusKnown ? <Status meta={ENV_STATUS[state]} /> : <span className="text-xs text-muted-foreground">…</span>}
                  details={
                    <>
                      <p className="truncate" title={usedByText}>
                        {usedByText}
                      </p>
                      <p className={broken.length ? "mt-0.5 truncate text-xs text-warning" : "mt-0.5 truncate text-xs text-muted-foreground"}>
                        {broken.length
                          ? `${broken.map((item) => item.envVar).join(", ")} would be empty`
                          : mapping.description || "No description"}
                      </p>
                    </>
                  }
                  updated={relativeTime(mapping.updatedAt)}
                />
              );
            })}
          </ListCard>
          <ListCount shown={visible.length} total={counts[filter]} singular="environment" plural="environments" />
        </>
      ) : (
        <EmptyState
          icon={<KeyRound />}
          title={
            search
              ? `No environments match “${search}”`
              : all.length === 0
                ? "No environments yet"
                : `No environments that are “${FILTER_LABEL[filter].toLowerCase()}”`
          }
          description={
            search
              ? "Try a shorter term, or clear the filter to see every environment."
              : all.length === 0
                ? "An environment is a named set of variables, for example GITLAB_TOKEN taken from your GitLab connection. Create one, then attach it to a worker profile."
                : "Environments with another status are one tap away in the filter."
          }
          action={
            search ? (
              <Button variant="outline" onClick={() => setSearch("")}>
                Clear filter
              </Button>
            ) : all.length === 0 ? (
              <Button asChild variant="outline">
                <Link href="/environments/new">
                  <Plus /> New environment
                </Link>
              </Button>
            ) : (
              <Button variant="outline" onClick={() => setFilter("all")}>
                Show all environments
              </Button>
            )
          }
        />
      )}
    </div>
  );
}
