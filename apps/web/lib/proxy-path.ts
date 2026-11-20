/**
 * Pure validation for the same-origin API proxy path.
 *
 * Next decodes catch-all route params, so `%2F`, `%3F`, `%23`, `%5C` and `%2E` can reach
 * the proxy as literal `/`, `?`, `#`, `\` and `.` characters. Reject those segments
 * instead of letting them change the structure of the upstream URL.
 */

const FORBIDDEN_CHARACTER = /[/?#\\\u0000-\u001f\u007f]/;
// Also reject the encoded spellings in case a runtime hands us a still-encoded param.
const FORBIDDEN_ESCAPE = /%(?:2e|2f|3f|23|5c)/i;

export type UpstreamPathResult =
  | { ok: true; path: string }
  | { ok: false; reason: string };

/**
 * Validate decoded catch-all segments and re-encode them for the upstream URL.
 * Returns the path without the leading `/api/`, or the reason it was rejected.
 */
export function buildUpstreamPath(segments: readonly string[]): UpstreamPathResult {
  if (!Array.isArray(segments) || segments.length === 0) {
    return { ok: false, reason: "empty path" };
  }

  const encoded: string[] = [];
  for (const segment of segments) {
    if (typeof segment !== "string" || segment.length === 0) {
      return { ok: false, reason: "empty segment" };
    }
    if (segment === "." || segment === "..") {
      return { ok: false, reason: "relative path segment" };
    }
    if (FORBIDDEN_CHARACTER.test(segment) || FORBIDDEN_ESCAPE.test(segment)) {
      return { ok: false, reason: "unsafe character" };
    }
    encoded.push(encodeURIComponent(segment));
  }
  return { ok: true, path: encoded.join("/") };
}
