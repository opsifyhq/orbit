"use client";

import { useQuery } from "@tanstack/react-query";

import { request } from "./http";

/**
 * Embedded documentation endpoints (`/api/v1/docs`). These are not in the generated
 * OpenAPI client yet, so the response shapes are declared here; replace them with the
 * generated types after the next `pnpm api:generate`.
 */
export interface DocSummary {
  slug: string;
  title: string;
  description: string;
  url: string;
}

export interface DocCategory {
  id: string;
  title: string;
  pages: DocSummary[];
}

export interface DocIndex {
  categories: DocCategory[];
}

export interface DocPage {
  slug: string;
  title: string;
  description: string;
  category: string;
  url: string;
  path: string;
  /** Markdown with relative links already rewritten to `/api/v1/docs/...` routes. */
  markdown: string;
}

export const DOCS_API = "/api/v1/docs";
export const DOCS_ASSETS_API = `${DOCS_API}/assets/`;
export const DOCS_HOME_SLUG = "index";

export const docsKeys = {
  index: ["docs", "index"] as const,
  page: (slug: string) => ["docs", "page", slug] as const,
};

export function useDocsIndex() {
  return useQuery<DocIndex>({
    queryKey: docsKeys.index,
    queryFn: () => request<DocIndex>(DOCS_API),
    staleTime: 10 * 60_000,
  });
}

export function useDocPage(slug: string) {
  return useQuery<DocPage>({
    queryKey: docsKeys.page(slug),
    queryFn: () =>
      request<DocPage>(`${DOCS_API}/${slug.split("/").map(encodeURIComponent).join("/")}`),
    staleTime: 10 * 60_000,
    retry: false,
  });
}

/** In-app route for a documentation slug. */
export function docRoute(slug: string): string {
  return slug === DOCS_HOME_SLUG ? "/documentation" : `/documentation/${slug}`;
}

/**
 * Map a link from the API's Markdown to where it should go in the web app. Returns
 * `kind: "page"` for in-app navigation, `"asset"` for embedded files that need an
 * authenticated fetch, and `"external"` for everything else.
 */
export function resolveDocLink(href: string | undefined): {
  kind: "page" | "asset" | "anchor" | "external";
  href: string;
} {
  if (!href) return { kind: "external", href: "" };
  if (href.startsWith("#")) return { kind: "anchor", href };
  if (href.startsWith(DOCS_ASSETS_API)) return { kind: "asset", href };
  if (href.startsWith(`${DOCS_API}/`)) {
    const rest = href.slice(DOCS_API.length + 1);
    const hashAt = rest.indexOf("#");
    const slug = decodeURIComponent(hashAt === -1 ? rest : rest.slice(0, hashAt));
    const hash = hashAt === -1 ? "" : rest.slice(hashAt);
    return { kind: "page", href: `${docRoute(slug)}${hash}` };
  }
  return { kind: "external", href };
}

/** GitHub-style heading slug; mirrors `heading_slug` in `crates/orbit-api/src/docs.rs`. */
export function headingSlug(text: string): string {
  return Array.from(text.trim())
    .map((char) => {
      if (/[\p{L}\p{N}_-]/u.test(char)) return char.toLowerCase();
      if (char === " ") return "-";
      return "";
    })
    .join("");
}

/** Plain text of an inline Markdown heading (links, code and emphasis flattened). */
function plainHeading(text: string): string {
  return text
    .replace(/!?\[([^\]]*)\]\([^)]*\)/g, "$1")
    .replace(/[`*]/g, "")
    .replace(/\s+#+\s*$/, "");
}

/**
 * Heading ids by source line (1-based), de-duplicated the way GitHub and the API do
 * (`title`, `title-1`, ...). Computed from the source so rendering stays pure.
 */
export function headingIdsByLine(markdown: string): Map<number, string> {
  const ids = new Map<number, string>();
  const seen = new Map<string, number>();
  let fence: string | null = null;
  markdown.split("\n").forEach((line, index) => {
    const fenceMatch = /^\s{0,3}(`{3,}|~{3,})/.exec(line);
    if (fenceMatch) {
      if (fence === null) fence = fenceMatch[1][0];
      else if (fenceMatch[1][0] === fence) fence = null;
      return;
    }
    if (fence !== null) return;
    const heading = /^\s{0,3}#{1,6}\s+(.*?)\s*$/.exec(line);
    if (!heading) return;
    const base = headingSlug(plainHeading(heading[1]));
    const count = seen.get(base) ?? 0;
    seen.set(base, count + 1);
    ids.set(index + 1, count === 0 ? base : `${base}-${count}`);
  });
  return ids;
}

/**
 * Fetch an embedded asset same-origin (through the web proxy) with the session's bearer
 * token; an `<img>` cannot send one. `NEXT_PUBLIC_ORBIT_API_URL` may be server-only.
 */
export async function fetchDocAsset(href: string): Promise<Blob> {
  const { getAccessToken } = await import("./api");
  const token = getAccessToken();
  const response = await fetch(href, {
    headers: token ? { Authorization: `Bearer ${token}` } : {},
  });
  if (!response.ok) throw new Error(`HTTP ${response.status}`);
  return response.blob();
}
