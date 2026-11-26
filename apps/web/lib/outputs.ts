"use client";

import { request } from "./http";

export type OutputKind = "slack" | "mattermost" | "webhook" | "telegram" | "s3" | "smtp";

export interface OutputView {
  id: string;
  name: string;
  kind: OutputKind;
  agentId?: string | null;
  agentIds?: string[];
  teamId?: string | null;
  events: string[];
  template?: string | null;
  enabled: boolean;
  /** True when a webhook URL or Telegram bot token is stored; the value is never returned. */
  hasWebhook: boolean;
  chatId?: string | null;
  messageThreadId?: string | null;
  config?: Record<string, unknown>;
  createdAt: string;
  updatedAt: string;
  /** The newest delivery, without run details (list responses only). */
  lastDelivery?: OutputDelivery | null;
}

/** One delivery of an output. Run fields are null when the caller may not see the run. */
export interface OutputDelivery {
  id: string;
  status: "pending" | "sending" | "succeeded" | "failed" | "dead" | "skipped" | (string & {});
  attempts: number;
  responseStatus?: number | null;
  error?: string | null;
  nextAttemptAt?: string | null;
  executionId?: string | null;
  teamRunId?: string | null;
  task?: string | null;
  agentName?: string | null;
  createdAt: string;
  updatedAt: string;
}

export interface OutputInput {
  name: string;
  kind: OutputKind;
  agentId?: string | null;
  agentIds?: string[];
  teamId?: string | null;
  events: string[];
  webhookUrl?: string;
  /** Telegram bot token, write-only. Omit when editing to keep the stored token. */
  botToken?: string;
  /** Numeric Telegram user/group/channel id (for example -100…). */
  chatId?: string | null;
  /** Optional numeric Telegram forum topic id. */
  messageThreadId?: string | null;
  /** S3 credential JSON: accessKeyId/secretAccessKey/sessionToken. Write-only. */
  s3Credential?: string;
  /** SMTP credential JSON: username/password. Write-only. */
  smtpCredential?: string;
  /** Non-secret S3 settings. */
  config?: Record<string, unknown>;
  template?: string | null;
  enabled: boolean;
}

export function listOutputs(): Promise<OutputView[]> {
  return request<{ items: OutputView[] }>("/api/v1/outputs?limit=500").then((page) => page.items);
}

export function listOutputDeliveries(id: string, limit = 25): Promise<OutputDelivery[]> {
  return request<{ items: OutputDelivery[] }>(`/api/v1/outputs/${id}/deliveries?limit=${limit}`).then(
    (page) => page.items,
  );
}

/** Add an existing output to an agent, or assign its single team target. */
export function retargetOutput(
  output: OutputView,
  scope: "agent" | "team",
  id: string,
): OutputInput {
  const base = {
    name: output.name,
    kind: output.kind,
    events: output.events,
    template: output.template ?? null,
    enabled: output.enabled,
    chatId: output.chatId ?? null,
    messageThreadId: output.messageThreadId ?? null,
    config: output.config ?? {},
  };
  if (scope === "agent") {
    return {
      ...base,
      agentIds: [...new Set([...(output.agentIds ?? (output.agentId ? [output.agentId] : [])), id])],
      teamId: null,
    };
  }
  return { ...base, agentIds: [], teamId: id };
}

export function createOutput(body: OutputInput): Promise<OutputView> {
  return request<OutputView>("/api/v1/outputs", {
    method: "POST",
    body: JSON.stringify(body),
  });
}

export function updateOutput(id: string, body: OutputInput): Promise<OutputView> {
  return request<OutputView>(`/api/v1/outputs/${id}`, {
    method: "PATCH",
    body: JSON.stringify(body),
  });
}

export function deleteOutput(id: string): Promise<void> {
  return request<void>(`/api/v1/outputs/${id}`, { method: "DELETE" });
}

export function testOutput(id: string): Promise<void> {
  return request<void>(`/api/v1/outputs/${id}/test`, {
    method: "POST",
    body: "{}",
  });
}
