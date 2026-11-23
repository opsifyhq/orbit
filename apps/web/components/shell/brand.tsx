import { useId } from "react";

import { useMeta } from "@/lib/queries";
import { cn } from "@/lib/utils";

/** The ORBIT ring and satellite. The satellite takes the frame's text colour so it reads on light and dark. */
export function BrandMark({ className }: { className?: string }) {
  // Unique per instance: a gradient defined inside a hidden copy (the desktop
  // sidebar on a phone) cannot be referenced by the visible one.
  const gradientId = useId();
  return (
    <svg viewBox="0 0 32 32" className={cn("h-7 w-7 shrink-0", className)} aria-hidden>
      <defs>
        <linearGradient id={gradientId} x1="4" y1="24" x2="28" y2="8" gradientUnits="userSpaceOnUse">
          <stop offset="0" stopColor="#5B4BE8" />
          <stop offset="0.6" stopColor="#7C6CFF" />
          <stop offset="1" stopColor="#A894FF" />
        </linearGradient>
      </defs>
      <g transform="rotate(-25 16 16)">
        <ellipse cx="16" cy="16" rx="11.5" ry="5.2" fill="none" stroke={`url(#${gradientId})`} strokeWidth="2.6" />
        <circle cx="27.5" cy="16" r="3.1" fill="hsl(var(--ink-foreground))" />
        <circle cx="27.5" cy="16" r="1.6" fill="#7C6CFF" />
      </g>
    </svg>
  );
}

export function Wordmark({ className }: { className?: string }) {
  return (
    <span className={cn("text-[15px] font-bold tracking-[0.2em] text-ink-foreground [font-stretch:116%]", className)}>
      ORBIT
    </span>
  );
}

/**
 * The mark, the wordmark and, under it, the version this installation runs. The version
 * comes from the API (`/api/v1/meta`), so it is the server's, not the browser bundle's.
 * Its line is reserved while it loads so the wordmark does not jump.
 */
export function BrandLockup({ className }: { className?: string }) {
  const version = useMeta().data?.version;
  return (
    <span className={cn("flex items-center gap-2.5", className)}>
      <BrandMark />
      <span className="flex flex-col gap-1">
        <Wordmark className="leading-none" />
        <span
          className="tabular text-[11px] font-medium leading-none text-ink-muted"
          title={version ? `ORBIT version ${version}` : undefined}
          aria-label={version ? `Version ${version}` : undefined}
        >
          {version ? `v${version}` : "\u00a0"}
        </span>
      </span>
    </span>
  );
}
