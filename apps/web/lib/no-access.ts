"use client";

const REASON_KEY = "orbit.noAccess.reason";

/** Remember the server's 403 message so /no-access can explain why access was denied. */
export function rememberNoAccessReason(reason: string): void {
  if (typeof window === "undefined") return;
  sessionStorage.setItem(REASON_KEY, reason);
}

/**
 * Read and clear the stashed reason so a later direct visit shows no stale text.
 * One-shot by design: React StrictMode may run a page effect twice, so callers should
 * only set state when a value was actually returned.
 */
export function readNoAccessReason(): string | null {
  if (typeof window === "undefined") return null;
  const reason = sessionStorage.getItem(REASON_KEY);
  if (reason !== null) sessionStorage.removeItem(REASON_KEY);
  return reason;
}
