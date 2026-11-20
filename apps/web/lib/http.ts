"use client";

import { API_BASE_URL, getAccessToken, setAccessToken } from "./api.ts";
import { ApiError, networkError } from "./api-error.ts";

export { ApiError };
import { currentLocationPath, loginPathFor } from "./safe-redirect.ts";

/** HTTP status of an error, or undefined when it is not an `ApiError`. */
export function errorStatus(error: unknown): number | undefined {
  return error instanceof ApiError ? error.status : undefined;
}

/** Field-level validation details from a failed save, for a form to place next to its fields. */
export function validationDetails(error: unknown): Array<{ path: string; message: string }> {
  return error instanceof ApiError ? (error.details ?? []) : [];
}

/** The credentials are not valid (401): the session is over and the token must be replaced. */
export function isSessionError(error: unknown): boolean {
  return errorStatus(error) === 401;
}

/** The identity is valid but the user may not use ORBIT (403 from `/me`). */
export function isNoAccessError(error: unknown): boolean {
  return errorStatus(error) === 403;
}

/** 4xx other than "slow down / try again" will fail the same way on every retry. */
export function isPermanentClientError(error: unknown): boolean {
  const status = errorStatus(error);
  return status !== undefined && status >= 400 && status < 500 && status !== 408 && status !== 429;
}

/**
 * Build an `ApiError` for a non-OK response from whatever the body looks like: the ORBIT
 * envelope (`{error:{code,message}}`), a probe result (`{error:"..."}` / `{message}`),
 * plain text, or an HTML page from a proxy in front of the API.
 */
export function apiErrorFromBody(status: number, body: unknown, statusText = ""): ApiError {
  const fallback = `HTTP ${status}${statusText ? ` ${statusText}` : ""}`;
  if (body && typeof body === "object") {
    const record = body as { error?: unknown; message?: unknown };
    const inner = record.error;
    if (inner && typeof inner === "object") {
      const envelope = inner as {
        code?: unknown;
        message?: unknown;
        details?: ApiError["details"];
        requestId?: unknown;
      };
      return new ApiError(
        status,
        typeof envelope.message === "string" && envelope.message ? envelope.message : fallback,
        {
          code: typeof envelope.code === "string" ? envelope.code : undefined,
          details: Array.isArray(envelope.details) ? envelope.details : undefined,
          requestId: typeof envelope.requestId === "string" ? envelope.requestId : undefined,
        },
      );
    }
    if (typeof inner === "string" && inner) return new ApiError(status, inner);
    if (typeof record.message === "string" && record.message) {
      return new ApiError(status, record.message);
    }
  }
  if (typeof body === "string") {
    const text = body.trim();
    // Proxies answer with HTML error pages; that is not something to show a person.
    if (text && !text.startsWith("<") && text.length <= 300) return new ApiError(status, text);
  }
  return new ApiError(status, fallback);
}

/** Read the body of a failed response into an `ApiError` without ever throwing a parse error. */
export async function apiErrorFromResponse(response: Response): Promise<ApiError> {
  const text = await response.text().catch(() => "");
  let body: unknown = text;
  try {
    body = JSON.parse(text);
  } catch {
    // not JSON; apiErrorFromBody handles plain text
  }
  return apiErrorFromBody(response.status, body, response.statusText);
}

/**
 * A 401 with a token means the session is gone: forget the token and go to the login
 * screen. Every helper that calls the API (generated client via `unwrap`, JSON, raw and
 * SSE) funnels through here so a stale token is handled the same way.
 */
export function handleUnauthorized() {
  if (!getAccessToken()) return;
  setAccessToken(null);
  const current = currentLocationPath();
  if (typeof window !== "undefined" && !current.split(/[?#]/, 1)[0].startsWith("/login")) {
    window.location.assign(loginPathFor(current));
  }
}

/**
 * Authenticated request against the control plane that returns the raw `Response`
 * (blobs, text, probes). Uses the same base URL and bearer token as the generated client,
 * redirects to login on 401 and throws an `ApiError` with a readable message otherwise.
 */
export async function requestRaw(
  path: string,
  init?: RequestInit & {
    /**
     * Always go through the same-origin web proxy instead of `API_BASE_URL`. Use for
     * browser downloads: `NEXT_PUBLIC_ORBIT_API_URL` may point at a host only the server
     * can reach (see PR #94).
     */
    sameOrigin?: boolean;
  },
): Promise<Response> {
  const { sameOrigin, ...rest } = init ?? {};
  init = rest;
  const token = getAccessToken();
  const headers = new Headers(init?.headers);
  if (token) headers.set("Authorization", `Bearer ${token}`);

  let response: Response;
  try {
    response = await fetch(`${sameOrigin ? "" : API_BASE_URL}${path}`, { ...init, cache: "no-store", headers });
  } catch (error) {
    if ((error as { name?: string })?.name === "AbortError") throw error;
    throw networkError();
  }
  if (!response.ok) {
    if (response.status === 401) handleUnauthorized();
    throw await apiErrorFromResponse(response);
  }
  return response;
}

/**
 * Authenticated JSON request against the control plane.
 *
 * Used by the hand-written API modules (agent teams, launcher) that predate the
 * generated client. Reads 204 as `undefined` and surfaces the error envelope message.
 */
export async function request<T>(path: string, init?: RequestInit): Promise<T> {
  const headers = new Headers(init?.headers);
  if (!headers.has("Content-Type")) headers.set("Content-Type", "application/json");
  const response = await requestRaw(path, { ...init, headers });
  if (response.status === 204) return undefined as T;
  try {
    return (await response.json()) as T;
  } catch {
    throw new ApiError(response.status, "The API returned a response that is not valid JSON.");
  }
}
