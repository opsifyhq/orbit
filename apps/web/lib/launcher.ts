"use client";

import { request } from "./http";
import type { WorkerLaunch } from "./types";

export function launchWorkerProfile(profileId: string): Promise<WorkerLaunch> {
  return request<WorkerLaunch>(`/api/v1/worker-profiles/${profileId}/launch`, {
    method: "POST",
    body: "{}",
  });
}

export function stopWorkerLaunch(launchId: string): Promise<WorkerLaunch> {
  return request<WorkerLaunch>(`/api/v1/worker-launches/${launchId}/stop`, {
    method: "POST",
    body: "{}",
  });
}

/** Hide an ended launch from the launch history; it stays readable under Archived. */
export function archiveWorkerLaunch(launchId: string): Promise<WorkerLaunch> {
  return request<WorkerLaunch>(`/api/v1/worker-launches/${launchId}/archive`, {
    method: "POST",
    body: "{}",
  });
}

/** Remove an ended launch record for good. */
export function deleteWorkerLaunch(launchId: string): Promise<void> {
  return request<void>(`/api/v1/worker-launches/${launchId}`, { method: "DELETE" });
}

export function workerLaunchLogs(launchId: string): Promise<{ logs: string }> {
  return request<{ logs: string }>(`/api/v1/worker-launches/${launchId}/logs`);
}

export function workerLaunches(includeInactive = false): Promise<WorkerLaunch[]> {
  const query = includeInactive ? "?includeInactive=true&limit=500" : "?limit=500";
  return request<{ items: WorkerLaunch[] }>(`/api/v1/worker-launches${query}`).then(
    (page) => page.items,
  );
}
