"use client";

import Link from "next/link";
import { usePathname } from "next/navigation";
import { useEffect, useState } from "react";
import { Activity, ArrowRight, Bot, Compass, Gauge, Search, Workflow, type LucideIcon } from "lucide-react";

import { Kbd } from "@/components/ui/kbd";
import { useOptionalCommandPalette } from "@/components/shell/command-palette";

const DESTINATIONS: { href: string; label: string; description: string; icon: LucideIcon }[] = [
  { href: "/", label: "Overview", description: "What is running and what needs you", icon: Gauge },
  { href: "/executions", label: "Executions", description: "Every run, with its timeline", icon: Activity },
  { href: "/agents", label: "Agents", description: "Definitions, versions and runs", icon: Bot },
  { href: "/agent-teams", label: "Agent teams", description: "Pipelines and workflows", icon: Workflow },
];

/**
 * Unknown paths: name the path that was not found, offer search (the command palette)
 * and the four places people most often meant to go (#456).
 */
export default function NotFound() {
  const pathname = usePathname();
  const palette = useOptionalCommandPalette();
  // The shortcut label follows the platform, after mount (the server cannot know it).
  const [shortcut, setShortcut] = useState("Ctrl K");
  useEffect(() => {
    if (/Mac|iPhone|iPad/.test(navigator.platform)) setShortcut("⌘ K");
  }, []);

  return (
    <div className="mx-auto flex min-h-[70vh] w-full max-w-2xl flex-col items-center justify-center py-10 text-center">
      <div className="relative mb-6">
        <div aria-hidden className="absolute inset-0 -m-3 rounded-full bg-primary/10 motion-safe:animate-pulse" />
        <div className="relative flex h-14 w-14 items-center justify-center rounded-full border bg-card text-primary shadow-sm">
          <Compass className="h-7 w-7" />
        </div>
      </div>
      <p className="tabular text-xs font-semibold uppercase tracking-[0.18em] text-muted-foreground">
        Error 404
      </p>
      <h1 className="mt-2 text-2xl font-semibold tracking-tight sm:text-3xl">This page does not exist</h1>
      <p className="mt-3 max-w-md text-sm text-muted-foreground">
        Nothing lives at{" "}
        <code className="break-all rounded-md border bg-subtle px-1.5 py-0.5 font-mono text-[13px] text-foreground">
          {pathname}
        </code>
        . The link may be old, or the item was deleted.
      </p>

      {palette ? (
        <button
          type="button"
          onClick={palette.open}
          className="mt-7 flex w-full max-w-md items-center gap-3 rounded-lg border bg-card px-3.5 py-2.5 text-left text-sm text-muted-foreground shadow-sm transition-colors hover:border-border-strong hover:bg-muted/40 focus-visible:outline focus-visible:outline-2 focus-visible:outline-ring"
        >
          <Search className="h-4 w-4 shrink-0" />
          <span className="flex-1">Search pages and agents</span>
          <Kbd className="hidden sm:inline-flex">{shortcut}</Kbd>
        </button>
      ) : null}

      <ul className="mt-8 grid w-full gap-3 sm:grid-cols-2">
        {DESTINATIONS.map(({ href, label, description, icon: Icon }) => (
          <li key={href}>
            <Link
              href={href}
              className="group flex h-full items-center gap-3 rounded-lg border bg-card p-3.5 text-left transition-colors hover:border-border-strong hover:bg-muted/40 focus-visible:outline focus-visible:outline-2 focus-visible:outline-ring"
            >
              <span className="flex h-9 w-9 shrink-0 items-center justify-center rounded-md bg-muted text-muted-foreground transition-colors group-hover:bg-primary/10 group-hover:text-primary">
                <Icon className="h-[18px] w-[18px]" />
              </span>
              <span className="min-w-0 flex-1">
                <span className="block text-sm font-medium">{label}</span>
                <span className="block truncate text-[13px] text-muted-foreground">{description}</span>
              </span>
              <ArrowRight className="h-4 w-4 shrink-0 text-muted-foreground transition-transform motion-safe:group-hover:translate-x-0.5" />
            </Link>
          </li>
        ))}
      </ul>
    </div>
  );
}
