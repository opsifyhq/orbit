import type { StatusMeta } from "@/components/ui/status";
import { api } from "@/lib/api";
import { unwrap, useConnections } from "@/lib/queries";
import type { ConnectionView } from "@/lib/types";

export type Capability = { id: string; name: string; effect: string };

export type ConfigProperty = {
  type?: string;
  title?: string;
  description?: string;
  default?: unknown;
  format?: string;
};

export type ConfigSchema = {
  properties?: Record<string, ConfigProperty>;
  required?: string[];
};

/** Connectors whose address is a URL field instead of schema properties. */
export const URL_CONNECTORS = ["gitlab", "webhook"];
/** Config keys the URL field writes, hidden from the generic schema fields. */
export const URL_KEYS = ["baseUrl", "cloneBaseUrl", "url"];

/** How a connection's last check went. The API keeps a usable connection `active`. */
export type ConnectionHealth = "connected" | "error" | "unchecked";

export function connectionHealth(
  connection: Pick<ConnectionView, "status" | "lastCheckedAt">,
): ConnectionHealth {
  if (connection.status === "error") return "error";
  // "active" only means usable; a recorded check is what earns "Connected".
  return connection.status === "active" && connection.lastCheckedAt ? "connected" : "unchecked";
}

export function connectionStatus(
  connection: Pick<ConnectionView, "status" | "lastCheckedAt">,
): StatusMeta {
  const health = connectionHealth(connection);
  if (health === "connected") return { label: "Connected", tone: "success", shape: "solid" };
  // Degraded, not broken: agents can still use a connection whose last check failed.
  if (health === "error") return { label: "Check failed", tone: "warning", shape: "cross" };
  return { label: "Not checked", tone: "neutral", shape: "ring" };
}

export const EFFECT_GROUPS: Array<{ tone: "safe" | "write" | "external"; title: string; hint: string }> = [
  { tone: "safe", title: "Read only", hint: "Look things up. Nothing changes." },
  { tone: "write", title: "Change data", hint: "Create or update things in the service." },
  { tone: "external", title: "Act outside ORBIT", hint: "Send messages or trigger effects people notice." },
];

export function defaultConfig(slug: string): Record<string, unknown> {
  if (slug === "github") return { baseUrl: "https://api.github.com" };
  if (slug === "gitlab") return { baseUrl: "https://gitlab.com/api/v4", cloneBaseUrl: "https://gitlab.com" };
  if (slug === "webhook") return { url: "" };
  return {};
}

/** The URL field's value for a stored config. */
export function urlFromConfig(slug: string, config: Record<string, unknown>): string {
  if (slug === "gitlab") {
    return String(config.cloneBaseUrl ?? "") || String(config.baseUrl ?? "").replace(/\/api\/v4$/, "");
  }
  if (slug === "webhook") return String(config.url ?? "");
  return "";
}

/** Write the URL field into the config the connector expects. */
export function applyUrl(slug: string, config: Record<string, unknown>, url: string): Record<string, unknown> {
  const clean = url.trim().replace(/\/+$/, "");
  if (!clean) return config;
  if (slug === "gitlab") {
    const root = clean.replace(/\/api\/v4$/, "");
    return { ...config, baseUrl: `${root}/api/v4`, cloneBaseUrl: root };
  }
  if (slug === "webhook") return { ...config, url: clean };
  return config;
}

export function urlError(url: string): string | null {
  const clean = url.trim();
  if (!clean) return null;
  return /^https?:\/\/[^\s/]+/.test(clean) ? null : "Use a full URL starting with https:// or http://.";
}

export interface ConnectionInput {
  connectorSlug: string;
  name: string;
  credential?: string;
  scopes: string[];
  config: Record<string, unknown>;
}

export async function createConnection(input: ConnectionInput) {
  return unwrap(await api.POST("/api/v1/connections", { body: input }));
}

export async function updateConnection(id: string, input: ConnectionInput) {
  const { name, scopes, config, credential } = input;
  return unwrap(
    await api.PATCH("/api/v1/connections/{connection_id}", {
      params: { path: { connection_id: id } },
      body: { name, scopes, config, ...(credential ? { credential } : {}) },
    }),
  );
}

/** One connection, read from the list (the API has no single-connection GET). */
export function useConnection(id: string | undefined) {
  const connections = useConnections();
  return {
    ...connections,
    data: connections.data?.find((connection) => connection.id === id),
  };
}

export { allowedKubernetesResources, kubernetesResources } from "@/lib/kubernetes-resources";
