"use client";

import {
  Boxes,
  Copy,
  Download,
  Ellipsis,
  Pencil,
  Power,
  PowerOff,
  Rocket,
  RotateCw,
  Trash2,
} from "lucide-react";
import { toast } from "sonner";

import { exportResource } from "@/components/export-resource-button";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuSeparator,
  DropdownMenuTrigger,
} from "@/components/ui/dropdown-menu";
import { ResourceAccess } from "@/components/ui/resource-access";
import { Status, type StatusMeta } from "@/components/ui/status";
import { copyText } from "@/lib/browser";
import { brandName } from "@/lib/presentation";
import type { WorkerProfile } from "@/lib/types";
import { relativeTime } from "@/lib/utils";

import type { SectionId } from "./editor-sections";
import { targetLabel } from "./shared";

/** Whether the profile is running, idle or switched off. Always a glyph plus a word. */
function runState(runningCount: number, enabled: boolean): StatusMeta {
  if (runningCount) return { label: `${runningCount} running`, tone: "success", shape: "orbiting" };
  if (!enabled) return { label: "Disabled", tone: "neutral", shape: "slash" };
  return { label: "Not running", tone: "neutral", shape: "dashed" };
}

/** One worker profile in the Profiles list: state, what it runs, and the next action. */
export function WorkerCard({
  profile,
  runningCount,
  platformVersion,
  launching,
  launchDisabled,
  relaunching,
  onLaunch,
  onRelaunch,
  onBootstrap,
  onEdit,
  onToggle,
  onDelete,
}: {
  profile: WorkerProfile;
  runningCount: number;
  platformVersion: string | undefined;
  launching: boolean;
  launchDisabled: boolean;
  relaunching: boolean;
  onLaunch: () => void;
  onRelaunch: () => void;
  onBootstrap: () => void;
  onEdit: (section?: SectionId) => void;
  onToggle: () => void;
  onDelete: () => void;
}) {
  const scoped = (profile as WorkerProfile & { agentScope?: string }).agentScope === "selected";
  const following = !profile.imageTag;
  const updateAvailable = Boolean(platformVersion && profile.imageTag && profile.imageTag !== platformVersion);
  const shownTag = profile.imageTag || platformVersion || "platform tag";

  return (
    <li className="border-b px-4 py-3.5 last:border-b-0 sm:px-5">
      <div className="flex flex-col gap-3 md:flex-row md:items-start md:justify-between md:gap-6">
        <div className="min-w-0 flex-1 space-y-1">
          <div className="flex flex-wrap items-center gap-x-3 gap-y-1">
            <h3 className="min-w-0 break-words text-sm font-semibold">{profile.name}</h3>
            <Status meta={runState(runningCount, profile.enabled)} />
            {runningCount && !profile.enabled ? <Badge variant="outline">Disabled</Badge> : null}
          </div>
          <p className="text-[13px] text-muted-foreground">
            {brandName(profile.harnessKind)} · {targetLabel(profile.target)} ·{" "}
            {profile.executionDriver === "docker" ? "container per execution" : "long-lived process"}
            <span className="hidden sm:inline"> · Updated {relativeTime(profile.updatedAt)}</span>
          </p>
          <div className="flex min-w-0 flex-wrap items-center gap-x-2 gap-y-1">
            <p
              className="min-w-0 max-w-full truncate font-mono text-xs text-muted-foreground"
              title={`${profile.image}:${shownTag}`}
            >
              {profile.image}:{shownTag}
            </p>
            {updateAvailable ? (
              <button
                type="button"
                onClick={() => onEdit("runtime")}
                aria-label={`Update available for ${profile.name}: pinned to ${profile.imageTag}, platform is ${platformVersion}. Open runtime settings.`}
                title={`Pinned to ${profile.imageTag}; the platform is ${platformVersion}. Edit to follow the platform.`}
                className="rounded-md"
              >
                <Badge variant="warning">Update available</Badge>
              </button>
            ) : following ? (
              <Badge variant="secondary" title="Relaunched automatically after a platform upgrade">
                Follows platform
              </Badge>
            ) : (
              <Badge variant="outline">Pinned</Badge>
            )}
          </div>
          <div className="flex flex-wrap items-center gap-1.5 pt-0.5">
            <Badge variant="secondary">{scoped ? `${profile.agentIds.length} agents` : "All agents"}</Badge>
            <ResourceAccess ownerTeamId={profile.ownerTeamId} visibility={profile.visibility} />
            <Badge variant="secondary">
              {profile.skillSlugs.length} {profile.skillSlugs.length === 1 ? "skill" : "skills"}
            </Badge>

          </div>
        </div>

        <div className="flex items-center gap-2 md:shrink-0">
          {runningCount ? (
            <Button
              size="sm"
              variant="outline"
              className="max-md:flex-1"
              loading={relaunching}
              onClick={onRelaunch}
            >
              <RotateCw /> Relaunch
            </Button>
          ) : (
            <Button
              size="sm"
              className="max-md:flex-1"
              loading={launching}
              disabled={launchDisabled}
              onClick={onLaunch}
            >
              <Rocket /> Launch
            </Button>
          )}
          <Button size="sm" variant="outline" className="max-md:flex-1" onClick={() => onEdit()}>
            <Pencil /> Edit
          </Button>
          <DropdownMenu>
            <DropdownMenuTrigger asChild>
              <Button size="icon" variant="ghost" aria-label={`More actions for ${profile.name}`}>
                <Ellipsis />
              </Button>
            </DropdownMenuTrigger>
            <DropdownMenuContent align="end">
              <DropdownMenuItem onSelect={onBootstrap}>
                <Boxes /> Bootstrap by hand
              </DropdownMenuItem>
              <DropdownMenuItem
                onSelect={async () => {
                  if (await copyText(profile.id)) toast.success("Profile ID copied");
                  else toast.error("Copying is not available in this browser");
                }}
              >
                <Copy /> Copy profile ID
              </DropdownMenuItem>
              <DropdownMenuItem onSelect={() => void exportResource("worker_profiles", profile.id, profile.name)}>
                <Download /> Export as YAML
              </DropdownMenuItem>
              <DropdownMenuItem onSelect={onToggle}>
                {profile.enabled ? (
                  <>
                    <PowerOff /> Disable profile
                  </>
                ) : (
                  <>
                    <Power /> Enable profile
                  </>
                )}
              </DropdownMenuItem>
              <DropdownMenuSeparator />
              <DropdownMenuItem destructive disabled={profile.enabled} onSelect={onDelete}>
                <Trash2 />
                {profile.enabled ? "Delete profile (disable first)" : "Delete profile"}
              </DropdownMenuItem>
            </DropdownMenuContent>
          </DropdownMenu>
        </div>
      </div>
    </li>
  );
}
