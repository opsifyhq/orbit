"use client";

import { useEffect, useMemo, useState, type ComponentProps, type ReactNode } from "react";
import Link from "next/link";
import type { Components } from "react-markdown";

import { toast } from "sonner";

import { Markdown } from "@/components/ui/markdown";
import { fetchDocAsset, headingIdsByLine, resolveDocLink } from "@/lib/docs";
import { cn } from "@/lib/utils";

/** Image from the embedded docs: loaded with the bearer token, shown from a blob URL. */
function DocImage({ src, alt }: { src?: string; alt?: string }) {
  const [url, setUrl] = useState<string | null>(null);
  const [failed, setFailed] = useState(false);
  const kind = resolveDocLink(src).kind;

  useEffect(() => {
    if (!src || kind !== "asset") return;
    let objectUrl: string | null = null;
    let cancelled = false;
    fetchDocAsset(src)
      .then((blob) => {
        if (cancelled) return;
        objectUrl = URL.createObjectURL(blob);
        setUrl(objectUrl);
      })
      .catch(() => !cancelled && setFailed(true));
    return () => {
      cancelled = true;
      if (objectUrl) URL.revokeObjectURL(objectUrl);
    };
  }, [src, kind]);

  if (!src) return null;
  if (kind !== "asset") {
    // eslint-disable-next-line @next/next/no-img-element
    return <img src={src} alt={alt ?? ""} className="my-3 h-auto max-w-full rounded-md border" referrerPolicy="no-referrer" />;
  }
  if (failed) {
    return <span className="text-[13px] text-muted-foreground">[image unavailable: {alt || src}]</span>;
  }
  if (!url) {
    return <span aria-hidden className="my-3 block h-40 animate-pulse rounded-md bg-muted" />;
  }
  // eslint-disable-next-line @next/next/no-img-element
  return <img src={url} alt={alt ?? ""} className="my-3 h-auto max-w-full rounded-md border bg-card" />;
}

const LINK_CLASS = "font-medium text-primary underline underline-offset-2";

function DocLink({ href, children }: ComponentProps<"a">) {
  const target = resolveDocLink(href);
  if (target.kind === "page") {
    return (
      <Link href={target.href} className={LINK_CLASS}>
        {children}
      </Link>
    );
  }
  if (target.kind === "anchor") {
    return (
      <a href={target.href} className={LINK_CLASS}>
        {children}
      </a>
    );
  }
  if (target.kind === "asset") {
    return (
      <a
        href={target.href}
        className={LINK_CLASS}
        onClick={(event) => {
          event.preventDefault();
          // Popup blockers only allow a window opened synchronously in the click handler.
          // Do not pass "noopener" in features: some browsers then return a null handle.
          const win = window.open("", "_blank");
          if (!win) {
            toast.error("Your browser blocked the pop-up. Allow pop-ups for this site and try again.");
            return;
          }
          win.opener = null;
          void fetchDocAsset(target.href)
            .then((blob) => {
              if (win.closed) return;
              const objectUrl = URL.createObjectURL(blob);
              win.location.href = objectUrl;
              // Give the new document time to load before releasing the blob.
              window.setTimeout(() => URL.revokeObjectURL(objectUrl), 60_000);
            })
            .catch(() => {
              if (!win.closed) win.close();
              toast.error("Could not open the document asset.");
            });
        }}
      >
        {children}
      </a>
    );
  }
  return (
    <a href={target.href} className={LINK_CLASS} target="_blank" rel="noreferrer noopener">
      {children}
    </a>
  );
}

function heading(Tag: "h1" | "h2" | "h3" | "h4", className: string, ids: Map<number, string>): Components["h1"] {
  return function Heading({ node, children }: { node?: { position?: { start: { line: number } } }; children?: ReactNode }) {
    const id = node?.position ? ids.get(node.position.start.line) : undefined;
    return (
      <Tag id={id} className={cn("scroll-mt-20", className)}>
        {children}
      </Tag>
    );
  };
}

/**
 * Renders a documentation page. The Markdown comes from the API (trusted repo content)
 * and is still rendered without raw HTML, using the app's own type scale. Page links
 * stay inside `/documentation`, images load through the authenticated API.
 */
export function DocReader({ markdown, slug }: { markdown: string; slug: string }) {
  const components = useMemo<Components>(() => {
    const ids = headingIdsByLine(markdown);
    return {
      h1: heading("h1", "font-display mb-3 mt-8 text-2xl leading-tight first:mt-0", ids),
      h2: heading("h2", "mb-2 mt-8 border-b pb-1.5 text-xl font-semibold first:mt-0", ids),
      h3: heading("h3", "mb-1.5 mt-6 text-base font-semibold first:mt-0", ids),
      h4: heading("h4", "mb-1 mt-4 text-sm font-semibold first:mt-0", ids),
      a: DocLink,
      img: ({ src, alt }) => <DocImage src={typeof src === "string" ? src : undefined} alt={alt} />,
      // Reference tables (permissions, access matrix, settings) are dense: give them room
      // and headers that do not wrap; anything still too wide scrolls inside its frame.
      table: ({ node: _node, ...props }) => (
        <div className="my-5 overflow-x-auto rounded-lg border">
          <table className="w-full border-collapse text-[13px] leading-6 [&_tbody_tr:last-child>td]:border-b-0" {...props} />
        </div>
      ),
      th: ({ node: _node, ...props }) => (
        <th
          className="whitespace-nowrap border-b bg-subtle px-3 py-2 text-left text-xs font-semibold uppercase tracking-wide text-muted-foreground"
          {...props}
        />
      ),
      // `overflow-wrap: anywhere` lets long tokens (paths like `a/b/c/d`) wrap, so one cell
      // cannot force the whole table wider than the page; the minimum width keeps words
      // whole on phones, where the table scrolls sideways inside its frame instead.
      td: ({ node: _node, ...props }) => (
        <td className="min-w-28 border-b px-3 py-2 align-top [overflow-wrap:anywhere]" {...props} />
      ),
    };
  }, [markdown]);

  // Anchors point at headings that only exist after the (async) page has rendered.
  useEffect(() => {
    const hash = window.location.hash.slice(1);
    if (!hash) {
      window.scrollTo({ top: 0 });
      return;
    }
    document.getElementById(decodeURIComponent(hash))?.scrollIntoView();
  }, [slug, markdown]);

  return (
    <Markdown className="text-[15px] leading-7" components={components}>
      {markdown}
    </Markdown>
  );
}
