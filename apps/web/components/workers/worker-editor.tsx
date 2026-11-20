"use client";

import { useEffect, useRef, useState } from "react";
import { useMutation, useQueryClient } from "@tanstack/react-query";
import { ChevronLeft } from "lucide-react";
import { toast } from "sonner";

import { ResourceAccessFields, type ResourceAccessHandle } from "@/components/resource-access-fields";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { Callout } from "@/components/ui/callout";
import { PageHeader } from "@/components/ui/page-header";
import { Status } from "@/components/ui/status";
import { api } from "@/lib/api";
import { launchWorkerProfile, stopWorkerLaunch } from "@/lib/launcher";
import {
  queryKeys,
  unwrap,
  useAgents,
  useConnections,
  useEnvMappings,
  useMeta,
  useProviders,
  useSkills,
  useWorkerProfileEnvMappings,
} from "@/lib/queries";
import type { WorkerLaunch, WorkerProfile } from "@/lib/types";
import { errorMessage } from "@orbit/api-client";

import { SectionCard, SectionNav, SectionPills } from "./editor-nav";
import {
  SECTION_ORDER,
  SECTION_TITLES,
  isSectionId,
  sectionSummaries,
  validateWorkerForm,
  type SectionId,
} from "./editor-sections";
import { EnvironmentFields } from "./environment-section";
import { RelaunchOnSaveDialog } from "./launch-dialogs";
import { AuthSecretField, ModelFields } from "./model-section";
import { PermissionGroups } from "./permissions-section";
import { ResourceFields } from "./resources-section";
import { BasicsFields, ImageFields, VersionFields } from "./runtime-section";
import { AgentScopeFields, SkillFields } from "./scope-section";
import { FormGroup, isActive } from "./shared";
import {
  DEFAULT_WORKER_FORM,
  buildProfileBody,
  restartFieldsChanged,
  useWorkerForm,
  valuesFromProfile,
} from "./worker-form";

/** What to open the editor on. `profile: null` creates a new profile. */
export type EditorTarget = {
  profile: WorkerProfile | null;
  /** Pre-fill for a new profile (from the Work wizard). */
  harness?: string;
  target?: string;
  /** A reusable mapping to tick when editing (from Environments, Attach). */
  attachMappingId?: string;
  /** The section to open on. */
  section?: SectionId;
};

/**
 * The profile editor. It takes the whole page, one section at a time (the Agent Studio
 * pattern): a section nav that doubles as the summary, Save always in view, Back and Next
 * between sections, and the section in the URL hash. The browser's Back button closes it.
 */
export function WorkerEditor({
  editor,
  launches,
  onClose,
  onCreated,
}: {
  editor: EditorTarget;
  launches: WorkerLaunch[];
  onClose: () => void;
  onCreated: (profile: WorkerProfile) => void;
}) {
  const accessRef = useRef<ResourceAccessHandle>(null);
  const editing = editor.profile;
  const attachMappingId = editor.attachMappingId ?? null;
  const queryClient = useQueryClient();
  const form = useWorkerForm(() =>
    editing
      ? valuesFromProfile(editing)
      : {
          ...DEFAULT_WORKER_FORM,
          harness: editor.harness ?? DEFAULT_WORKER_FORM.harness,
          target: editor.target ?? DEFAULT_WORKER_FORM.target,
        },
  );
  const { values } = form;
  const [confirmRelaunch, setConfirmRelaunch] = useState(false);
  const [attempted, setAttempted] = useState(false);

  const providers = useProviders();
  const agents = useAgents({ limit: 100 });
  const skills = useSkills();
  const connections = useConnections();
  const meta = useMeta();
  const platformVersion = meta.data?.version;
  const envMappingLibrary = useEnvMappings();
  const profileEnvMappings = useWorkerProfileEnvMappings(editing?.id);
  // Seed the checkbox state once. Refetches return the same mappings, and re-seeding
  // would throw away any unsaved ticks the user has made in the meantime.
  const envMappingsSeeded = useRef(false);

  useEffect(() => {
    if (!editing || !profileEnvMappings.data || envMappingsSeeded.current) return;
    envMappingsSeeded.current = true;
    const attached = profileEnvMappings.data.map((mapping) => mapping.id);
    form.set({
      envMappingIds:
        attachMappingId && !attached.includes(attachMappingId) ? [...attached, attachMappingId] : attached,
    });
    // form.set merges with a functional update, so it is safe to leave out; the data is the trigger.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [editing, profileEnvMappings.data, attachMappingId]);

  // Saving while an attached-mapping read failed would send a form with no ids and
  // detach every mapping. Wait for a successful read before allowing a save.
  const envMappingsLoading = Boolean(editing) && profileEnvMappings.isLoading;
  const envMappingsFailed = Boolean(editing) && profileEnvMappings.isError;
  const envMappingsBlocked = envMappingsLoading || envMappingsFailed;
  const envMappingsReason = envMappingsLoading
    ? "Attached environment mappings are still loading."
    : envMappingsFailed
      ? `Attached environment mappings could not be loaded: ${errorMessage(profileEnvMappings.error)}. Saving is disabled so they are not detached.`
      : null;

  const activeProfileLaunches = launches.filter(
    (item) => editing && item.profileId === editing.id && isActive(item),
  );
  const needsRelaunch = Boolean(editing) && activeProfileLaunches.length > 0 && restartFieldsChanged(editing, values);

  // One section is on screen at a time. The URL hash remembers which, so a reload or a
  // shared link opens the same section. Opening the editor adds one history entry, so the
  // browser's Back button closes it instead of leaving the page.
  const [active, setActive] = useState<SectionId>(editor.section ?? (attachMappingId ? "environment" : "runtime"));
  const [focusRequest, setFocusRequest] = useState(0);
  const pushed = useRef(false);
  useEffect(() => {
    if (!pushed.current) {
      pushed.current = true;
      window.history.pushState(null, "", `#${active}`);
    }
    const sync = () => {
      const fromHash = window.location.hash.replace(/^#/, "");
      if (isSectionId(fromHash)) setActive(fromHash);
      else onClose();
    };
    window.addEventListener("hashchange", sync);
    window.addEventListener("popstate", sync);
    return () => {
      window.removeEventListener("hashchange", sync);
      window.removeEventListener("popstate", sync);
    };
    // Registered once for the life of the editor.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  /** Close through history so the editor's entry does not linger behind the list. */
  function close() {
    if (isSectionId(window.location.hash.replace(/^#/, ""))) window.history.back();
    else onClose();
  }

  useEffect(() => {
    if (!focusRequest) return;
    const node = document.getElementById(`section-${active}`);
    node?.scrollIntoView({ block: "start" });
    node
      ?.querySelector<HTMLElement>("input:not([type=hidden]), textarea, select")
      ?.focus({ preventScroll: true });
  }, [focusRequest, active]);

  useEffect(() => {
    if (!attachMappingId) return;
    const timer = window.setTimeout(() => {
      document.getElementById("wp-reusable-mappings")?.scrollIntoView({ block: "center" });
    }, 300);
    return () => window.clearTimeout(timer);
  }, [attachMappingId]);

  function goToSection(id: SectionId) {
    setActive(id);
    setFocusRequest((count) => count + 1);
    window.history.replaceState(null, "", `#${id}`);
  }

  const save = useMutation({
    mutationFn: async (applyNow: boolean) => {
      const body = buildProfileBody(values, editing);
      if (editing) {
        const profile = unwrap(
          await api.PATCH("/api/v1/worker-profiles/{profile_id}", {
            params: { path: { profile_id: editing.id } },
            body,
          }),
        );
        if (applyNow) {
          for (const item of activeProfileLaunches) {
            await stopWorkerLaunch(item.id);
          }
          await launchWorkerProfile(editing.id);
        }
        return profile;
      }
      return unwrap(await api.POST("/api/v1/worker-profiles", { body }));
    },
    onSuccess: (profile, applyNow) => {
      if (applyNow) {
        toast.success("Profile saved and worker relaunched");
      } else {
        toast.success(
          editing ? "Profile saved" : "Profile created",
          needsRelaunch ? { description: "Relaunch the worker to apply the new configuration." } : undefined,
        );
      }
      close();
      void queryClient.invalidateQueries({ queryKey: queryKeys.workerProfiles });
      void queryClient.invalidateQueries({ queryKey: queryKeys.workerLaunches });
      void queryClient.invalidateQueries({ queryKey: ["worker-profile-env-mappings"] });
      if (!editing) onCreated(profile as WorkerProfile);
    },
    onError: (error) => toast.error(errorMessage(error)),
  });

  const issues = validateWorkerForm(values);
  const summaries = sectionSummaries(values, Boolean(editing));

  async function requestSave() {
    if (envMappingsBlocked) {
      toast.error(envMappingsReason ?? "Attached environment mappings are unavailable.");
      return;
    }
    if (issues.length) {
      // Open the first section with a problem rather than leaving a dead button.
      setAttempted(true);
      const first = SECTION_ORDER.find((id) => issues.some((issue) => issue.section === id));
      if (first) goToSection(first);
      return;
    }
    if ((await accessRef.current?.save()) === false) return;
    if (needsRelaunch) {
      setConfirmRelaunch(true);
      return;
    }
    save.mutate(false);
  }

  const saveLabel = editing ? "Save changes" : "Create profile";
  const saveButton = (className?: string) => (
    <Button
      onClick={() => void requestSave()}
      loading={save.isPending}
      disabled={envMappingsBlocked}
      title={envMappingsReason ?? undefined}
      size="lg"
      className={className}
    >
      {saveLabel}
    </Button>
  );
  const providerRows = providers.data ?? [];
  const runningCount = activeProfileLaunches.length;

  return (
    <div className="pb-24 lg:pb-0">
      <RelaunchOnSaveDialog
        open={confirmRelaunch}
        onOpenChange={setConfirmRelaunch}
        launchCount={activeProfileLaunches.length}
        onSaveOnly={() => {
          setConfirmRelaunch(false);
          save.mutate(false);
        }}
        onSaveAndRelaunch={() => {
          setConfirmRelaunch(false);
          save.mutate(true);
        }}
      />

      <div className="mb-5 space-y-3">
        <Button type="button" variant="ghost" size="sm" className="-ml-2" onClick={close}>
          <ChevronLeft /> Workers
        </Button>
        <PageHeader
          title={editing ? `Edit ${editing.name}` : "New worker profile"}
          description="The worker loads this profile at startup, claims only its agents and prepares its skills. Secrets stay outside ORBIT."
          meta={
            editing ? (
              <>
                {runningCount ? (
                  <Status meta={{ label: `${runningCount} running`, tone: "success", shape: "orbiting" }} />
                ) : null}
                {editing.enabled ? null : <Badge variant="outline">Disabled</Badge>}
              </>
            ) : undefined
          }
        />
      </div>

      <SectionPills active={active} onSelect={goToSection} issues={issues} />

      <div className="grid gap-6 lg:grid-cols-[15rem_minmax(0,1fr)] lg:items-start">
        {/* Desktop: section nav with the live summary, and Save in view. */}
        <aside className="hidden lg:sticky lg:top-6 lg:block">
          <SectionNav active={active} onSelect={goToSection} summaries={summaries} issues={issues} />
          <div className="mt-4 space-y-2 border-t pt-4">
            {issues.length > 0 ? (
              <p className="text-[13px] text-destructive" role="status">
                {issues.length} {issues.length === 1 ? "issue needs" : "issues need"} fixing.
              </p>
            ) : null}
            {saveButton("w-full")}
            <Button variant="ghost" className="w-full" onClick={close}>
              Cancel
            </Button>
          </div>
        </aside>

        <div className="min-w-0 max-w-3xl space-y-5">
          {envMappingsReason ? (
            <Callout tone={envMappingsFailed ? "danger" : "neutral"} title="Environment mappings are not ready">
              {envMappingsReason}
            </Callout>
          ) : null}
          {needsRelaunch ? (
            <Callout tone="warning" title="A running worker will not pick these changes up yet">
              {runningCount} {runningCount === 1 ? "launch is" : "launches are"} running on this profile.
              Saving asks whether to relaunch {runningCount === 1 ? "it" : "them"}.
            </Callout>
          ) : null}

          {attempted && issues.length > 0 ? (
            <Callout tone="danger" title="Fix these before saving">
              <ul className="mt-1 space-y-1">
                {issues.map((issue) => (
                  <li key={`${issue.section}-${issue.message}`}>
                    <button
                      type="button"
                      onClick={() => goToSection(issue.section)}
                      className="text-left underline decoration-destructive/40 underline-offset-2 hover:decoration-destructive"
                    >
                      <span className="font-medium text-foreground">{SECTION_TITLES[issue.section]}:</span>{" "}
                      {issue.message}
                    </button>
                  </li>
                ))}
              </ul>
            </Callout>
          ) : null}

          <SectionCard id="runtime" active={active} onGo={goToSection}>
            <BasicsFields form={form} providers={providerRows} />
            <ImageFields form={form} />
            <VersionFields form={form} platformVersion={platformVersion} />
          </SectionCard>

          <SectionCard id="model" active={active} onGo={goToSection}>
            <ModelFields form={form} providers={providerRows} />
            <AuthSecretField form={form} />
          </SectionCard>

          <SectionCard id="scope" active={active} onGo={goToSection}>
            <FormGroup title="Agents" description="Which agents this worker may claim.">
              <AgentScopeFields form={form} agents={agents.data?.items ?? []} />
            </FormGroup>
            <Callout tone="neutral" title="Claim scope is not the same as profile visibility">
              A worker claims organization-visible agents plus agents owned by the worker
              profile&apos;s access team. With <strong>Selected agents</strong>, only the
              picked ids are claimable. Making the profile organization-visible does not
              widen this claim scope; move the agent to the profile&apos;s team, make it
              organization-visible, select it explicitly, or remove the profile&apos;s owning
              team.
            </Callout>
            <FormGroup title="Skills" description="Shared skills the worker prepares at startup.">
              <SkillFields form={form} skills={skills.data ?? []} />
            </FormGroup>
          </SectionCard>

          <SectionCard id="permissions" active={active} onGo={goToSection}>
            <PermissionGroups form={form} />
          </SectionCard>

          <SectionCard id="environment" active={active} onGo={goToSection}>
            <EnvironmentFields
              form={form}
              connections={connections.data ?? []}
              library={envMappingLibrary.data}
              libraryLoading={envMappingLibrary.isLoading}
            />
          </SectionCard>

          <SectionCard id="resources" active={active} onGo={goToSection}>
            <ResourceFields form={form} />
          </SectionCard>

          <SectionCard id="sharing" active={active} onGo={goToSection}>
            <ResourceAccessFields ref={accessRef} resource="worker_profiles" resourceId={editing?.id} />
          </SectionCard>
        </div>
      </div>

      {/* Phone and tablet: Save pinned above the 56px tab bar. */}
      <div className="fixed inset-x-0 bottom-14 z-30 border-t bg-background/95 p-3 backdrop-blur lg:hidden">
        <div className="flex items-center justify-between gap-3">
          <Button variant="ghost" onClick={close}>
            Cancel
          </Button>
          {saveButton()}
        </div>
      </div>
    </div>
  );
}
