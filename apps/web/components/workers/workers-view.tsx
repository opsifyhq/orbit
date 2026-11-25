"use client";

import { useEffect, useRef, useState } from "react";
import { useSearchParams } from "next/navigation";
import { Plus } from "lucide-react";
import { toast } from "sonner";

import { Button } from "@/components/ui/button";
import { ImportPortableButton } from "@/components/import-portable-dialog";
import { Callout } from "@/components/ui/callout";
import { PageHeader } from "@/components/ui/page-header";
import { useMe, useMeta, useWorkerLaunches, useWorkerProfiles, useWorkers } from "@/lib/queries";
import type { LaunchView } from "@/lib/run-controls";
import type { WorkerProfile } from "@/lib/types";

import { BootstrapDialog } from "./bootstrap-dialog";
import { LaunchesSection } from "./launches-section";
import { LogsDialog } from "./launch-dialogs";
import { LiveWorkersSection } from "./live-workers-section";
import { ProfilesSection } from "./profiles-section";
import { launchErrorText } from "./shared";
import { useWorkerActions } from "./use-worker-actions";
import { WorkerEditor, type EditorTarget } from "./worker-editor";
import { freshFailedLaunches, workersSummary } from "./workers-summary";

/** The Workers page: live workers, profiles and launches, plus the profile editor. */
export function WorkersView() {
  const profiles = useWorkerProfiles();
  const [launchView, setLaunchView] = useState<LaunchView>("active");
  const showInactive = launchView !== "active";
  const launches = useWorkerLaunches(showInactive);
  const me = useMe();
  const liveWorkers = useWorkers();
  const meta = useMeta();
  const platformVersion = meta.data?.version;
  const searchParams = useSearchParams();
  const [editor, setEditor] = useState<EditorTarget | null>(null);
  const [editorKey, setEditorKey] = useState(0);
  const [bootstrapProfile, setBootstrapProfile] = useState<WorkerProfile | null>(null);
  const launchList = launches.data ?? [];
  const actions = useWorkerActions(launchList);

  function openEditor(target: EditorTarget) {
    setEditorKey((key) => key + 1);
    setEditor(target);
  }

  // Pre-fill from Work wizard query params.
  const prefilled = useRef(false);
  useEffect(() => {
    if (searchParams.get("create") !== "1" || prefilled.current) return;
    prefilled.current = true;
    const harnessParam = searchParams.get("harness");
    const placementParam = searchParams.get("placement");
    openEditor({
      profile: null,
      harness: harnessParam || undefined,
      target: placementParam
        ? placementParam === "kubernetes"
          ? "kubernetes"
          : placementParam === "docker"
            ? "docker"
            : "local"
        : undefined,
    });
  }, [searchParams]);

  // `?edit=<profile>&attach=<mapping>` opens that profile with the mapping ticked, at the
  // section where it is attached. Used by the Environments page.
  const [deepLinked, setDeepLinked] = useState(false);
  useEffect(() => {
    if (deepLinked || !profiles.data) return;
    const editId = searchParams.get("edit");
    if (!editId) return;
    setDeepLinked(true);
    // Consume the link so a reload or Back does not reopen the dialog.
    window.history.replaceState(null, "", window.location.pathname);
    const profile = profiles.data.find((candidate) => candidate.id === editId);
    if (!profile) {
      toast.error("That worker profile no longer exists");
      return;
    }
    openEditor({ profile, attachMappingId: searchParams.get("attach") || undefined });
  }, [deepLinked, profiles.data, searchParams]);

  const failedLaunches = freshFailedLaunches(launchList);
  const summary = workersSummary({
    workers: liveWorkers.data,
    profiles: profiles.data,
    failedLaunches,
  });

  // The editor takes the whole page; the dialogs below stay available either way.
  const dialogs = (
    <>
      <BootstrapDialog profile={bootstrapProfile} onClose={() => setBootstrapProfile(null)} />
      <LogsDialog
        open={actions.logsDialog.open}
        onOpenChange={actions.logsDialog.onOpenChange}
        name={actions.logsDialog.launch?.name}
        logs={actions.logsDialog.logs}
        loading={actions.logsDialog.loading}
      />
    </>
  );

  if (editor) {
    return (
      <>
        <WorkerEditor
          key={editorKey}
          editor={editor}
          launches={launchList}
          onClose={() => setEditor(null)}
          onCreated={setBootstrapProfile}
        />
        {dialogs}
      </>
    );
  }

  return (
    <div className="space-y-6">
      <PageHeader
        title="Workers"
        description="Workers run agent executions. A profile fixes the image, target, agents, skills and harness auth; launch it from here or bootstrap it yourself with Docker, Compose or Kubernetes."
        meta={summary.length ? summary : undefined}
        actions={
          <div className="flex flex-wrap gap-2">
            <ImportPortableButton />
            <Button onClick={() => openEditor({ profile: null })}>
              <Plus /> New profile
            </Button>
          </div>
        }
      />

      {failedLaunches.length ? (
        <Callout
          tone="danger"
          title={`${failedLaunches.length} ${failedLaunches.length === 1 ? "launch has" : "launches have"} failed`}
          action={
            showInactive ? undefined : (
              <Button size="sm" variant="outline" onClick={() => setLaunchView("history")}>
                Show history
              </Button>
            )
          }
        >
          {failedLaunches[0].error
            ? `${failedLaunches[0].name}: ${launchErrorText(failedLaunches[0].error)}`
            : `${failedLaunches[0].name} did not start. Open its logs for details.`}
        </Callout>
      ) : null}

      <ProfilesSection
        profiles={profiles}
        launches={launchList}
        platformVersion={platformVersion}
        launchPendingFor={(profile) => actions.launch.isPending && actions.launch.variables === profile.id}
        launchPending={actions.launch.isPending}
        relaunchPendingFor={(profile) => actions.relaunch.isPending && actions.relaunch.variables === profile.id}
        onCreate={() => openEditor({ profile: null })}
        onLaunch={(profile) => actions.launch.mutate(profile.id)}
        onRelaunch={(profile) => void actions.confirmRelaunchProfile(profile)}
        onBootstrap={setBootstrapProfile}
        onEdit={(profile, section) => openEditor({ profile, section })}
        onToggle={(profile) => actions.toggle.mutate({ id: profile.id, enabled: !profile.enabled })}
        onDelete={(profile) => void actions.confirmDeleteProfile(profile)}
      />

      <LaunchesSection
        launches={launches}
        view={launchView}
        onViewChange={setLaunchView}
        permissions={me.data?.permissions}
        isEnding={(item) =>
          (actions.archive.isPending && actions.archive.variables === item.id) ||
          (actions.removeLaunch.isPending && actions.removeLaunch.variables === item.id)
        }
        onArchive={(item) => actions.archive.mutate(item.id)}
        onDelete={(item) => void actions.confirmDeleteLaunch(item)}
        isRelaunching={(item) => actions.launch.isPending && actions.launch.variables === item.profileId}
        isStopping={(item) => actions.stop.isPending && actions.stop.variables === item.id}
        onLogs={(item) => void actions.openLogs(item.id, item.name)}
        onStop={(item) => actions.stop.mutate(item.id)}
        onRelaunch={(item) => actions.launch.mutate(item.profileId)}
      />

      <LiveWorkersSection workers={liveWorkers} />

      {dialogs}
    </div>
  );
}
