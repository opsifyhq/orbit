"use client";

import { useEffect, useState } from "react";
import Link from "next/link";
import {
  BookOpen,
  Bot,
  Cable,
  ChevronDown,
  Compass,
  FileCode2,
  Layers,
  Rocket,
  ScrollText,
  Server,
  ShieldCheck,
  UsersRound,
  type LucideIcon,
} from "lucide-react";

import { Skeleton } from "@/components/ui/skeleton";
import { docRoute, type DocIndex } from "@/lib/docs";
import { cn } from "@/lib/utils";

/**
 * Section icons. Build, Connect, Infrastructure and Admin reuse the icons of the matching
 * group in the app sidebar, so a page is found where its screen lives. Sections are
 * defined in docs/navigation.yaml; unknown ids fall back to a book.
 */
const SECTION_ICONS: Record<string, LucideIcon> = {
  start: Compass,
  build: Bot,
  connect: Cable,
  infrastructure: Server,
  admin: UsersRound,
  operate: Rocket,
  security: ShieldCheck,
  architecture: Layers,
  contribute: FileCode2,
  decisions: ScrollText,
};

/** Reference material starts folded so the task-oriented sections stand out. */
const FOLDED_BY_DEFAULT = new Set(["operate", "security", "architecture", "contribute", "decisions"]);
const FOLD_KEY = "orbit.docs.folded";

function readFolded(): Record<string, boolean> {
  try {
    const raw = window.localStorage.getItem(FOLD_KEY);
    return raw ? (JSON.parse(raw) as Record<string, boolean>) : {};
  } catch {
    return {};
  }
}

function NavList({ index, current }: { index: DocIndex; current: string }) {
  // Explicit choices only; anything unset follows FOLDED_BY_DEFAULT.
  const [choices, setChoices] = useState<Record<string, boolean>>({});
  useEffect(() => setChoices(readFolded()), []);

  function toggle(id: string, folded: boolean) {
    const next = { ...choices, [id]: !folded };
    setChoices(next);
    try {
      window.localStorage.setItem(FOLD_KEY, JSON.stringify(next));
    } catch {
      // Storage is a convenience; folding still works for this visit.
    }
  }

  return (
    <nav aria-label="Documentation" className="space-y-1">
      {index.categories.map((category) => {
        const Icon = SECTION_ICONS[category.id] ?? BookOpen;
        const hasCurrent = category.pages.some((page) => page.slug === current);
        // The section holding the current page is always open.
        const folded = !hasCurrent && (choices[category.id] ?? FOLDED_BY_DEFAULT.has(category.id));
        const listId = `doc-section-${category.id}`;
        return (
          <div key={category.id} className="pt-2 first:pt-0">
            <button
              type="button"
              aria-expanded={!folded}
              aria-controls={listId}
              onClick={() => toggle(category.id, folded)}
              className={cn(
                "flex min-h-9 w-full items-center gap-2 rounded-md px-2 text-left text-xs font-semibold transition-colors",
                hasCurrent ? "text-foreground" : "text-muted-foreground hover:text-foreground",
              )}
            >
              <Icon className={cn("h-4 w-4 shrink-0", hasCurrent ? "text-primary" : "text-muted-foreground")} aria-hidden />
              <span className="min-w-0 flex-1 truncate">{category.title}</span>
              <ChevronDown
                className={cn("h-3.5 w-3.5 shrink-0 transition-transform", folded && "-rotate-90")}
                aria-hidden
              />
            </button>
            {folded ? null : (
              <ul id={listId} className="mb-1 ml-[15px] mt-0.5 border-l">
                {category.pages.map((page) => {
                  const active = page.slug === current;
                  return (
                    <li key={page.slug}>
                      <Link
                        href={docRoute(page.slug)}
                        aria-current={active ? "page" : undefined}
                        className={cn(
                          "-ml-px block min-h-9 border-l-2 py-2 pl-3 pr-2 text-[13px] leading-snug transition-colors",
                          active
                            ? "border-primary bg-accent/60 font-medium text-foreground"
                            : "border-transparent text-muted-foreground hover:border-border-strong hover:text-foreground",
                        )}
                      >
                        {page.title}
                      </Link>
                    </li>
                  );
                })}
              </ul>
            )}
          </div>
        );
      })}
    </nav>
  );
}

/**
 * Page index. On phones it is a disclosure above the content (closed by default and
 * re-closed on navigation via `key`); from `lg` it is a sticky sidebar.
 */
export function DocNav({
  index,
  loading,
  current,
  currentTitle,
}: {
  index?: DocIndex;
  loading: boolean;
  current: string;
  currentTitle?: string;
}) {
  if (loading || !index) {
    return <Skeleton className="h-10 w-full rounded-lg lg:h-96" />;
  }
  const section = index.categories.find((category) => category.pages.some((page) => page.slug === current));
  return (
    <>
      <details key={current} className="group rounded-lg border bg-card lg:hidden">
        <summary className="flex min-h-11 cursor-pointer list-none items-center justify-between gap-2 px-3 text-sm font-medium [&::-webkit-details-marker]:hidden">
          <span className="min-w-0 truncate">
            {section ? <span className="text-muted-foreground">{section.title} / </span> : null}
            {currentTitle ?? "Browse documentation"}
          </span>
          <ChevronDown className="h-4 w-4 shrink-0 text-muted-foreground transition-transform group-open:rotate-180" />
        </summary>
        <div className="max-h-[60dvh] overflow-y-auto border-t p-2">
          <NavList index={index} current={current} />
        </div>
      </details>
      <div className="sticky top-4 hidden max-h-[calc(100dvh-2rem)] overflow-y-auto pr-1 lg:block">
        <NavList index={index} current={current} />
      </div>
    </>
  );
}
