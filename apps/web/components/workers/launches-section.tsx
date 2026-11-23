import { Rocket, ScrollText, Square } from "lucide-react";

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
import type { WorkerLaunch } from "@/lib/types";
import { relativeTime } from "@/lib/utils";
import { errorMessage } from "@orbit/api-client";

import { isActive, launchErrorText, launchStatusMeta, targetLabel } from "./shared";

function LaunchRow({
  item,
  relaunching,
  stopping,
  onLogs,
  onStop,
  onRelaunch,
}: {
  item: WorkerLaunch;
  relaunching: boolean;
  stopping: boolean;
  onLogs: () => void;
  onStop: () => void;
  onRelaunch: () => void;
}) {
  const active = isActive(item);
  const relaunchRequired = item.relaunchRequired;
  return (
    <li className="border-b px-4 py-3 last:border-b-0 sm:px-5">
      <div className="flex flex-col gap-2.5 md:flex-row md:items-center md:justify-between md:gap-6">
        <div className="min-w-0 flex-1 space-y-0.5">
          <div className="flex flex-wrap items-center gap-x-3 gap-y-1">
            <Status meta={launchStatusMeta(item.status)} />
            <p className="min-w-0 truncate text-sm font-semibold">{item.name}</p>
            {relaunchRequired ? <Badge variant="warning">Relaunch required</Badge> : null}
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
        <div className="flex items-center gap-2 md:shrink-0">
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
        </div>
      </div>
    </li>
  );
}

/** Worker instances started from a profile by the launcher. */
export function LaunchesSection({
  launches,
  showInactive,
  onShowInactiveChange,
  isRelaunching,
  isStopping,
  onLogs,
  onStop,
  onRelaunch,
}: {
  launches: ReturnType<typeof useWorkerLaunches>;
  showInactive: boolean;
  onShowInactiveChange: (value: boolean) => void;
  isRelaunching: (item: WorkerLaunch) => boolean;
  isStopping: (item: WorkerLaunch) => boolean;
  onLogs: (item: WorkerLaunch) => void;
  onStop: (item: WorkerLaunch) => void;
  onRelaunch: (item: WorkerLaunch) => void;
}) {
  const launchList = launches.data ?? [];
  return (
    <Section
      title="Launches"
      count={launches.data ? launchList.length : undefined}
      description="Worker instances started from a profile by the launcher."
      actions={
        <Segmented
          label="Launches to show"
          value={showInactive ? "history" : "active"}
          onChange={(value) => onShowInactiveChange(value === "history")}
          options={[
            { value: "active", label: "Active" },
            { value: "history", label: "History" },
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
          title={showInactive ? "No launches yet" : "No active launches"}
          description={
            showInactive
              ? "Launch an enabled profile to start a worker."
              : "Launch an enabled profile to start a worker, or switch to History to see past launches."
          }
          action={
            showInactive ? undefined : (
              <Button variant="outline" onClick={() => onShowInactiveChange(true)}>
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
                relaunching={isRelaunching(item)}
                stopping={isStopping(item)}
                onLogs={() => onLogs(item)}
                onStop={() => onStop(item)}
                onRelaunch={() => onRelaunch(item)}
              />
            ))}
          </ul>
        </Card>
      )}
    </Section>
  );
}
