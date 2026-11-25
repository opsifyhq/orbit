import { useState } from "react";
import { useMutation, useQueryClient } from "@tanstack/react-query";
import { toast } from "sonner";

import { useConfirm } from "@/components/ui/confirm";
import { api } from "@/lib/api";
import {
  archiveWorkerLaunch,
  deleteWorkerLaunch,
  launchWorkerProfile,
  stopWorkerLaunch,
  workerLaunchLogs,
} from "@/lib/launcher";
import { queryKeys, unwrap } from "@/lib/queries";
import type { WorkerLaunch, WorkerProfile } from "@/lib/types";
import { errorMessage } from "@orbit/api-client";

import { isActive } from "./shared";

/** Launch, relaunch, stop, archive, enable/disable, delete and logs for worker profiles and launches. */
export function useWorkerActions(launchList: WorkerLaunch[]) {
  const queryClient = useQueryClient();
  const confirm = useConfirm();
  const [logsLaunch, setLogsLaunch] = useState<{ id: string; name: string } | null>(null);
  const [logs, setLogs] = useState("");
  const [logsLoading, setLogsLoading] = useState(false);
  const [logsOpen, setLogsOpen] = useState(false);

  const launch = useMutation({
    mutationFn: launchWorkerProfile,
    onSuccess: () => {
      toast.success("Launch requested");
      void queryClient.invalidateQueries({ queryKey: queryKeys.workerLaunches });
    },
    onError: (error) => {
      const message = errorMessage(error);
      // The launcher is an optional service; its absence should not read as a crash.
      toast.error(
        /launcher/i.test(message)
          ? "No worker launcher is reachable, so ORBIT cannot start this worker itself. Use Bootstrap to start it by hand, or configure the launcher."
          : `Launch failed: ${message}`,
      );
    },
  });

  const relaunch = useMutation({
    mutationFn: async (profileId: string) => {
      const active = launchList.filter((item) => item.profileId === profileId && isActive(item));
      for (const item of active) {
        await stopWorkerLaunch(item.id);
      }
      return launchWorkerProfile(profileId);
    },
    onSuccess: () => {
      toast.success("Relaunch requested");
      void queryClient.invalidateQueries({ queryKey: queryKeys.workerLaunches });
    },
    onError: (error) => toast.error(errorMessage(error)),
  });

  const stop = useMutation({
    mutationFn: stopWorkerLaunch,
    onSuccess: () => {
      toast.success("Launch stopped");
      void queryClient.invalidateQueries({ queryKey: queryKeys.workerLaunches });
    },
    onError: (error) => toast.error(errorMessage(error)),
  });

  const archive = useMutation({
    mutationFn: archiveWorkerLaunch,
    onSuccess: () => {
      toast.success("Launch archived");
      void queryClient.invalidateQueries({ queryKey: queryKeys.workerLaunches });
    },
    onError: (error) => toast.error(errorMessage(error)),
  });

  const removeLaunch = useMutation({
    mutationFn: deleteWorkerLaunch,
    onSuccess: () => {
      toast.success("Launch deleted");
      void queryClient.invalidateQueries({ queryKey: queryKeys.workerLaunches });
    },
    onError: (error) => toast.error(errorMessage(error)),
  });

  const toggle = useMutation({
    mutationFn: async ({ id, enabled }: { id: string; enabled: boolean }) =>
      unwrap(
        await api.POST(
          enabled
            ? "/api/v1/worker-profiles/{profile_id}/enable"
            : "/api/v1/worker-profiles/{profile_id}/disable",
          { params: { path: { profile_id: id } } },
        ),
      ),
    onSuccess: (_data, variables) => {
      toast.success(variables.enabled ? "Profile enabled" : "Profile disabled");
      void queryClient.invalidateQueries({ queryKey: queryKeys.workerProfiles });
    },
    onError: (error) => toast.error(errorMessage(error)),
  });

  const remove = useMutation({
    mutationFn: async (id: string) =>
      unwrap(
        await api.DELETE("/api/v1/worker-profiles/{profile_id}", {
          params: { path: { profile_id: id } },
        }),
      ),
    onSuccess: () => {
      toast.success("Profile deleted");
      void queryClient.invalidateQueries({ queryKey: queryKeys.workerProfiles });
    },
    onError: (error) => toast.error(errorMessage(error)),
  });

  async function openLogs(launchId: string, launchName: string) {
    setLogsLaunch({ id: launchId, name: launchName });
    setLogsOpen(true);
    setLogs("");
    setLogsLoading(true);
    try {
      const result = await workerLaunchLogs(launchId);
      setLogs(result.logs || "No log output yet.");
    } catch (error) {
      const message = errorMessage(error);
      setLogs(
        /worker launcher request failed|launcher.*error sending request|no worker launcher/i.test(message)
          ? "No logs: ORBIT could not reach the worker launcher, which keeps them. Logs from a worker you started yourself are in its own container or process output."
          : message,
      );
    } finally {
      setLogsLoading(false);
    }
  }

  async function confirmRelaunchProfile(profile: WorkerProfile) {
    const confirmed = await confirm({
      title: `Relaunch ${profile.name}?`,
      description:
        "The running worker is stopped and started again. Agents running on it may be interrupted.",
      confirmLabel: "Relaunch worker",
      destructive: true,
    });
    if (confirmed) relaunch.mutate(profile.id);
  }

  async function confirmDeleteProfile(profile: WorkerProfile) {
    const confirmed = await confirm({
      title: `Delete ${profile.name}?`,
      description: "The profile and its settings are removed. This cannot be undone.",
      confirmLabel: "Delete profile",
      destructive: true,
    });
    if (confirmed) remove.mutate(profile.id);
  }

  async function confirmDeleteLaunch(item: WorkerLaunch) {
    const confirmed = await confirm({
      title: `Delete launch ${item.name}?`,
      description:
        "The launch record is removed from ORBIT. The worker has already ended; its profile and executions are kept. This cannot be undone.",
      confirmLabel: "Delete launch",
      destructive: true,
    });
    if (confirmed) removeLaunch.mutate(item.id);
  }

  return {
    launch,
    archive,
    removeLaunch,
    confirmDeleteLaunch,
    relaunch,
    stop,
    toggle,
    remove,
    openLogs,
    confirmRelaunchProfile,
    confirmDeleteProfile,
    logsDialog: { open: logsOpen, onOpenChange: setLogsOpen, launch: logsLaunch, logs, loading: logsLoading },
  };
}
