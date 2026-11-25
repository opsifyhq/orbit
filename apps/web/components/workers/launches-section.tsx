import { Archive, Rocket, ScrollText, Square, Trash2 } from "lucide-react";

import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { Callout } from "@/components/ui/callout";
import { Card } from "@/components/ui/card";
import { EmptyState } from "@/components/ui/empty-state";
import { Section } from "@/components/ui/section";
import { Segmented } from "@/components/ui/segmented";
import { Skeleton } from "@/components/ui/skeleton";
import { Status } from "@/components/ui/status";
import type { useWorkerLaunches } from "@/lib/queries";
import { launchEndActions, launchesForView, type LaunchView } from "@/lib/run-controls";
import type { WorkerLaunch } from "@/lib/types";
import { relativeTime } from "@/lib/utils";
import { errorMessage } from "@orbit/api-client";

import { isActive, launchErrorText, launchStatusMeta, targetLabel } from "./shared";

function LaunchRow({
  item,
  permissions,
  relaunching,
  stopping,
  ending,
  onLogs,
  onStop,
  onRelaunch,
  onArchive,
  onDelete,
}: {
  item: WorkerLaunch;
  permissions: readonly string[] | undefined;
  relaunching: boolean;
  stopping: boolean;
  ending: boolean;
  onLogs: () => void;
  onStop: () => void;
  onRelaunch: () => void;
  onArchive: () => void;
  onDelete: () => void;
}) {
  const active = isActive(item);
  const end = launchEndActions(item, permissions);
  const relaunchRequired = item.relaunchRequired;
  return (
    <li className="border-b px-4 py-3 last:border-b-0 sm:px-5">
      <div className="flex flex-col gap-2.5 md:flex-row md:items-center md:justify-between md:gap-6">
        <div className="min-w-0 flex-1 space-y-0.5">
          <div className="flex flex-wrap items-center gap-x-3 gap-y-1">
            <Status meta={launchStatusMeta(item.status)} />
            <p className="min-w-0 truncate text-sm font-semibold">{item.name}</p>
            {relaunchRequired ? <Badge variant="warning">Relaunch required</Badge> : null}
            {item.archivedAt ? <Badge variant="secondary">Archived</Badge> : null}
          </div>
          <p className="text-[13px] text-muted-foreground">
            {targetLabel(item.target)} ·{" "}
            {item.workerId ? (
              <>
                worker <span className="break-all font-mono text-xs">{item.workerId}</span>
              </>
            ) : (
              "no worker connected yet"
            )}{" "}
            · updated {relativeTime(item.updatedAt)}
          </p>
          <p className="truncate font-mono text-xs text-muted-foreground" title={item.image}>
            {item.image}
          </p>
          {item.error ? (
            <p className="break-words text-[13px] text-destructive" title={item.error}>
              {launchErrorText(item.error)}
            </p>
          ) : null}
        </div>
        <div className="flex flex-wrap items-center gap-2 md:shrink-0">
          {active ? (
            <Button size="sm" variant="outline" className="max-md:flex-1" loading={stopping} onClick={onStop}>
              <Square /> Stop
            </Button>
          ) : (
            <Button size="sm" variant="outline" className="max-md:flex-1" loading={relaunching} onClick={onRelaunch}>
              <Rocket /> Launch again
            </Button>
          )}
          <Button size="sm" variant="ghost" className="max-md:flex-1" onClick={onLogs}>
            <ScrollText /> Logs
          </Button>
          {end.archive ? (
            <Button size="sm" variant="ghost" className="max-md:flex-1" disabled={ending} onClick={onArchive}>
              <Archive /> Archive
            </Button>
          ) : null}
          {end.remove ? (
            <Button
              size="sm"
              variant="ghost"
              className="text-destructive hover:text-destructive max-md:flex-1"
              disabled={ending}
              onClick={onDelete}
            >
              <Trash2 /> Delete
            </Button>
          ) : null}
        </div>
      </div>
    </li>
  );
}

/** Worker instances started from a profile by the launcher. */
export function LaunchesSection({
  launches,
  view,
  onViewChange,
  permissions,
  isRelaunching,
  isStopping,
  isEnding,
  onLogs,
  onStop,
  onRelaunch,
  onArchive,
  onDelete,
}: {
  launches: ReturnType<typeof useWorkerLaunches>;
  view: LaunchView;
  onViewChange: (value: LaunchView) => void;
  permissions: readonly string[] | undefined;
  isRelaunching: (item: WorkerLaunch) => boolean;
  isStopping: (item: WorkerLaunch) => boolean;
  isEnding: (item: WorkerLaunch) => boolean;
  onLogs: (item: WorkerLaunch) => void;
  onStop: (item: WorkerLaunch) => void;
  onRelaunch: (item: WorkerLaunch) => void;
  onArchive: (item: WorkerLaunch) => void;
  onDelete: (item: WorkerLaunch) => void;
}) {
  const launchList = launchesForView(launches.data ?? [], view);
  const showInactive = view !== "active";
  return (
    <Section
      title="Launches"
      count={launches.data ? launchList.length : undefined}
      description="Worker instances started from a profile by the launcher."
      actions={
        <Segmented
          label="Launches to show"
          value={view}
          onChange={(value) => onViewChange(value as LaunchView)}
          options={[
            { value: "active", label: "Active" },
            { value: "history", label: "History" },
            { value: "archived", label: "Archived" },
          ]}
        />
      }
    >
      {launches.isLoading ? (
        <Card className="overflow-hidden">
          <div className="px-5 py-4">
            <Skeleton className="h-12" />
          </div>
        </Card>
      ) : launches.isError ? (
        <Callout tone="danger" title="Launches could not be loaded">
          {errorMessage(launches.error)}
        </Callout>
      ) : !launchList.length ? (
        <EmptyState
          icon={<Rocket />}
          title={view === "archived" ? "No archived launches" : showInactive ? "No launches yet" : "No active launches"}
          description={
            view === "archived"
              ? "Archive an ended launch in History to move it here."
              : showInactive
              ? "Launch an enabled profile to start a worker."
              : "Launch an enabled profile to start a worker, or switch to History to see past launches."
          }
          action={
            showInactive ? undefined : (
              <Button variant="outline" onClick={() => onViewChange("history")}>
                Show history
              </Button>
            )
          }
        />
      ) : (
        <Card className="overflow-hidden">
          <ul>
            {launchList.map((item) => (
              <LaunchRow
                key={item.id}
                item={item}
                permissions={permissions}
                relaunching={isRelaunching(item)}
                stopping={isStopping(item)}
                ending={isEnding(item)}
                onLogs={() => onLogs(item)}
                onStop={() => onStop(item)}
                onRelaunch={() => onRelaunch(item)}
                onArchive={() => onArchive(item)}
                onDelete={() => onDelete(item)}
              />
            ))}
          </ul>
        </Card>
      )}
    </Section>
  );
}
