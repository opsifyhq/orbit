/**
 * Returns `value` only when it is a safe same-origin path, otherwise "/".
 *
 * Use for any redirect target taken from user-controlled input (e.g. `?next=`).
 *
 * Cases:
 *   "/agents"              -> "/agents"
 *   "/agents?x=1#y"        -> "/agents?x=1#y"
 *   "//evil.example"       -> "/"
 *   "https://evil.example" -> "/"
 *   "/\evil.example"       -> "/"
 *   "javascript:alert(1)"  -> "/"
 *   "" / null / undefined  -> "/"
 */
export function safeNextPath(value: string | null | undefined): string {
  if (!value || typeof value !== "string") return "/";
  if (!value.startsWith("/") || value.startsWith("//") || value.startsWith("/\\")) return "/";
  // eslint-disable-next-line no-control-regex
  if (/[\u0000-\u001f\u007f\\]/.test(value)) return "/";
  if (typeof window === "undefined") return value;
  try {
    const url = new URL(value, window.location.origin);
    if (url.origin !== window.location.origin) return "/";
  } catch {
    return "/";
  }
  return value;
}

/**
 * Current browser path including query and fragment. SSR-safe: returns "/" when no
 * window exists. Callers are client-side effects/handlers that need a return URL.
 */
export function currentLocationPath(): string {
  if (typeof window === "undefined") return "/";
  const { pathname, search, hash } = window.location;
  return `${pathname}${search}${hash}`;
}

/** Build a login URL that returns to `target`, validating it as a same-origin path. */
export function loginPathFor(target: string | null | undefined): string {
  return `/login?next=${encodeURIComponent(safeNextPath(target))}`;
}
