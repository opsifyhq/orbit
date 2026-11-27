import { Plus, Server } from "lucide-react";
import { useMemo, useState } from "react";
import { useQuery } from "@tanstack/react-query";

import { BulkAccessDialog } from "@/components/bulk-access-dialog";
import { Button } from "@/components/ui/button";
import { Callout } from "@/components/ui/callout";
import { Checkbox } from "@/components/ui/checkbox";
import { Select } from "@/components/ui/select";
import { SelectAllButton, SelectionBar, toggleShown } from "@/components/ui/resource-list";
import { Card } from "@/components/ui/card";
import { EmptyState } from "@/components/ui/empty-state";
import { Section } from "@/components/ui/section";
import { Skeleton } from "@/components/ui/skeleton";
import { listAccessTeams } from "@/lib/access-teams";
import type { useWorkerProfiles } from "@/lib/queries";
import type { WorkerLaunch, WorkerProfile } from "@/lib/types";
import { errorMessage } from "@orbit/api-client";

import type { SectionId } from "./editor-sections";
import { isActive } from "./shared";
import { WorkerCard } from "./worker-card";

/** Each profile is one worker definition. Launch starts a worker from it. */
export function ProfilesSection({
  profiles,
  launches,
  platformVersion,
  launchPendingFor,
  launchPending,
  relaunchPendingFor,
  onCreate,
  onLaunch,
  onRelaunch,
  onBootstrap,
  onEdit,
  onToggle,
  onDelete,
}: {
  profiles: ReturnType<typeof useWorkerProfiles>;
  launches: WorkerLaunch[];
  platformVersion: string | undefined;
  launchPendingFor: (profile: WorkerProfile) => boolean;
  launchPending: boolean;
  relaunchPendingFor: (profile: WorkerProfile) => boolean;
  onCreate: () => void;
  onLaunch: (profile: WorkerProfile) => void;
  onRelaunch: (profile: WorkerProfile) => void;
  onBootstrap: (profile: WorkerProfile) => void;
  onEdit: (profile: WorkerProfile, section?: SectionId) => void;
  onToggle: (profile: WorkerProfile) => void;
  onDelete: (profile: WorkerProfile) => void;
}) {
  const [teamFilter, setTeamFilter] = useState("all");
  const [visibilityFilter, setVisibilityFilter] = useState("all");
  const [selected, setSelected] = useState<Set<string>>(new Set());
  const [bulkOpen, setBulkOpen] = useState(false);
  const accessTeams = useQuery({ queryKey: ["access-teams"], queryFn: listAccessTeams, staleTime: 60_000 });
  const visible = useMemo(
    () =>
      (profiles.data ?? []).filter((profile) => {
        if (teamFilter === "none" && profile.ownerTeamId) return false;
        if (teamFilter !== "all" && teamFilter !== "none" && profile.ownerTeamId !== teamFilter) return false;
        if (visibilityFilter !== "all" && profile.visibility !== visibilityFilter) return false;
        return true;
      }),
    [profiles.data, teamFilter, visibilityFilter],
  );
  const selectedIds = visible.filter((profile) => selected.has(profile.id)).map((profile) => profile.id);
  return (
    <Section
      title="Profiles"
      count={profiles.data?.length}
      description="Each profile is one worker definition. Launch starts a worker from it."
    >
      {profiles.isLoading ? (
        <Card className="overflow-hidden">
          {Array.from({ length: 2 }).map((_, index) => (
            <div key={index} className="border-b px-5 py-4 last:border-b-0">
              <Skeleton className="h-14" />
            </div>
          ))}
        </Card>
      ) : profiles.isError ? (
        <Callout tone="danger" title="Profiles could not be loaded">
          {errorMessage(profiles.error)}
        </Callout>
      ) : !profiles.data?.length ? (
        <EmptyState
          icon={<Server />}
          title="No worker profiles yet"
          description="A profile pins the harness, image, target, agents, skills and auth secret. Workers start with ORBIT_WORKER_PROFILE_ID."
          action={
            <Button onClick={onCreate}>
              <Plus /> New profile
            </Button>
          }
        />
      ) : (
        <>
          <div className="mb-3 flex flex-wrap items-center gap-2">
            <Select aria-label="Filter profiles by team" value={teamFilter} onChange={(event) => setTeamFilter(event.target.value)} className="w-auto min-w-40">
              <option value="all">All teams</option>
              <option value="none">No team</option>
              {(accessTeams.data ?? []).map((team) => <option key={team.id} value={team.id}>{team.name}</option>)}
            </Select>
            <Select aria-label="Filter profiles by visibility" value={visibilityFilter} onChange={(event) => setVisibilityFilter(event.target.value)} className="w-auto min-w-40">
              <option value="all">All access</option>
              <option value="private">Private</option>
              <option value="team">Team</option>
              <option value="org">Organization</option>
            </Select>
            <SelectAllButton count={visible.length} selected={selectedIds.length} onSelectAll={() => setSelected(new Set(visible.map((profile) => profile.id)))} />
          </div>
          <SelectionBar
            count={selectedIds.length}
            noun="worker profiles"
            allShownSelected={visible.length > 0 && selectedIds.length === visible.length}
            onSelectAllShown={(checked) => setSelected((current) => toggleShown(current, visible.map((profile) => profile.id), checked))}
            onClear={() => setSelected(new Set())}
          >
            <Button size="sm" variant="outline" onClick={() => setBulkOpen(true)}>Change access</Button>
          </SelectionBar>
          <BulkAccessDialog
            open={bulkOpen}
            onOpenChange={setBulkOpen}
            resource="worker_profiles"
            ids={selectedIds}
            onDone={() => setSelected(new Set())}
          />
          <Card className="overflow-hidden">
          <ul>
            {visible.map((profile) => (
              <WorkerCard
                key={profile.id}
                profile={profile}
                checkbox={
                  <Checkbox
                    aria-label={`Select ${profile.name}`}
                    checked={selected.has(profile.id)}
                    onChange={(event) => setSelected((current) => toggleShown(current, [profile.id], event.target.checked))}
                  />
                }
                runningCount={
                  launches.filter((item) => item.profileId === profile.id && isActive(item)).length
                }
                platformVersion={platformVersion}
                launching={launchPendingFor(profile)}
                launchDisabled={launchPending}
                relaunching={relaunchPendingFor(profile)}
                onLaunch={() => onLaunch(profile)}
                onRelaunch={() => onRelaunch(profile)}
                onBootstrap={() => onBootstrap(profile)}
                onEdit={(section) => onEdit(profile, section)}
                onToggle={() => onToggle(profile)}
                onDelete={() => onDelete(profile)}
              />
            ))}
          </ul>
          </Card>
        </>
      )}
    </Section>
  );
}
