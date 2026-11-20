"use client";

import { createOrbitClient } from "@orbit/api-client";

import { networkError } from "./api-error.ts";

/**
 * Same-origin by default: the Next.js API proxy route forwards `/api/*` to the control
 * plane using `ORBIT_API_INTERNAL_URL` at runtime. Set NEXT_PUBLIC_ORBIT_API_URL only
 * for browser-reachable direct API setups (e.g. split domains); never point it at a
 * server-only internal host.
 */
export const API_BASE_URL =
  process.env.NEXT_PUBLIC_ORBIT_API_URL?.replace(/\/$/, "") ?? "";

const DEV_TOKEN = process.env.NEXT_PUBLIC_ORBIT_DEV_TOKEN ?? null;

export function getAccessToken(): string | null {
  if (typeof window === "undefined") return DEV_TOKEN;
  return window.localStorage.getItem("orbit.access_token") ?? DEV_TOKEN;
}

export function setAccessToken(token: string | null) {
  if (typeof window === "undefined") return;
  if (token && token.trim()) {
    window.localStorage.setItem("orbit.access_token", token.trim());
  } else {
    window.localStorage.removeItem("orbit.access_token");
  }
}

export const api = createOrbitClient({
  baseUrl: API_BASE_URL,
  token: () => getAccessToken(),
});

// `fetch` rejects with a bare TypeError when the server is unreachable. Give it the
// same shape as every other failure (status 0), so pages can tell "offline" from "404".
api.use({
  onError({ error }) {
    if ((error as { name?: string })?.name === "AbortError") return;
    return networkError();
  },
});

/**
 * Same-origin, like downloads: the Next proxy forwards SSE and works even when
 * NEXT_PUBLIC_ORBIT_API_URL is unset or points at a server-only internal host.
 */
export function eventStreamUrl(executionId: string): string {
  return `/api/v1/executions/${encodeURIComponent(executionId)}/events/stream`;
}
