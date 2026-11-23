"use client";

import { request } from "./http";

export type OutputKind = "slack" | "mattermost" | "webhook" | "telegram" | "s3";

export interface OutputView {
  id: string;
  name: string;
  kind: OutputKind;
  agentId?: string | null;
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
  /** Non-secret S3 settings. */
  config?: Record<string, unknown>;
  template?: string | null;
  enabled: boolean;
}

export function listOutputs(): Promise<OutputView[]> {
  return request<OutputView[]>("/api/v1/outputs");
}

export function listOutputDeliveries(id: string, limit = 25): Promise<OutputDelivery[]> {
  return request<OutputDelivery[]>(`/api/v1/outputs/${id}/deliveries?limit=${limit}`);
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
