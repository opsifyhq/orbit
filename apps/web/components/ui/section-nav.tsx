"use client";

import { useEffect, useRef, type ReactNode } from "react";
import { ChevronLeft, ChevronRight } from "lucide-react";

import { Button } from "@/components/ui/button";
import { Card, CardContent, CardHeader } from "@/components/ui/card";
import { cn } from "@/lib/utils";

/**
 * The frame of a long editor (Agent Studio, Skill Studio, Start work, …): one section on
 * screen at a time, a nav that lists every section with its current value, and Back/Next
 * under each section. See "Long editors" in docs/development/design-system.md.
 */
export type NavSection<Id extends string = string> = {
  id: Id;
  title: string;
  /** What the section is currently set to; doubles as the summary of the whole object. */
  value?: ReactNode;
  /** The value is a placeholder for something still required ("Not chosen"). */
  missing?: boolean;
  /** Validation problems in this section; shown instead of the value. */
  problems?: number;
};

function problemLabel(section: NavSection): string {
  return section.problems ? `${section.title}, ${section.problems} to fix` : section.title;
}

/** Desktop: the sections as a vertical list with their current values. */
export function SectionRail<Id extends string>({
  label,
  sections,
  active,
  onGo,
  wrapValues = false,
}: {
  /** Accessible name of the nav, e.g. "Agent sections". */
  label: string;
  sections: NavSection<Id>[];
  active: Id;
  onGo: (id: Id) => void;
  /** Let a value run to two lines instead of truncating (short lists only). */
  wrapValues?: boolean;
}) {
  return (
    <nav aria-label={label}>
      <ol className="space-y-0.5">
        {sections.map((section, index) => {
          const current = section.id === active;
          const problems = section.problems ?? 0;
          return (
            <li key={section.id}>
              <button
                type="button"
                onClick={() => onGo(section.id)}
                aria-current={current ? "step" : undefined}
                className={cn(
                  "flex w-full items-start gap-3 rounded-lg px-2.5 py-2 text-left transition-colors",
                  current ? "bg-accent text-accent-foreground" : "hover:bg-muted/60",
                )}
              >
                <span
                  aria-hidden
                  className={cn(
                    "tabular mt-px flex h-5 w-5 shrink-0 items-center justify-center rounded-full text-xs font-medium",
                    problems
                      ? "bg-destructive/15 text-destructive"
                      : current
                        ? "bg-primary text-primary-foreground"
                        : "bg-muted text-muted-foreground",
                  )}
                >
                  {problems ? "!" : index + 1}
                </span>
                <span className="min-w-0 flex-1">
                  <span className="block text-sm font-medium">{section.title}</span>
                  <span
                    className={cn(
                      "block text-xs",
                      wrapValues ? "line-clamp-2" : "truncate",
                      problems
                        ? "text-destructive"
                        : section.missing
                          ? "text-warning"
                          : "text-muted-foreground",
                    )}
                  >
                    {problems ? `${problems} to fix` : section.value}
                  </span>
                </span>
              </button>
            </li>
          );
        })}
      </ol>
    </nav>
  );
}

/** Phones and tablets: the sections as a scrollable row of steps. */
export function SectionPills<Id extends string>({
  label,
  sections,
  active,
  onGo,
  className,
}: {
  label: string;
  sections: NavSection<Id>[];
  active: Id;
  onGo: (id: Id) => void;
  className?: string;
}) {
  const navRef = useRef<HTMLElement>(null);
  // Keep the current step in view as Back/Next moves through the sections.
  useEffect(() => {
    const nav = navRef.current;
    const pill = nav?.querySelector<HTMLElement>(`[data-section="${active}"]`);
    if (!nav || !pill) return;
    const left = pill.offsetLeft - nav.offsetLeft;
    if (left < nav.scrollLeft || left + pill.offsetWidth > nav.scrollLeft + nav.clientWidth) {
      nav.scrollTo({ left: Math.max(0, left - 16) });
    }
  }, [active]);

  return (
    <nav
      ref={navRef}
      aria-label={label}
      className={cn(
        "-mx-4 overflow-x-auto px-4 [scrollbar-width:none] lg:hidden [&::-webkit-scrollbar]:hidden",
        className,
      )}
    >
      <ol className="flex w-max gap-1.5">
        {sections.map((section, index) => {
          const current = section.id === active;
          const problems = section.problems ?? 0;
          return (
            <li key={section.id}>
              <button
                type="button"
                onClick={() => onGo(section.id)}
                aria-current={current ? "step" : undefined}
                aria-label={problemLabel(section)}
                data-section={section.id}
                className={cn(
                  "inline-flex h-9 items-center gap-1.5 whitespace-nowrap rounded-full border px-3 text-[13px] font-medium transition-colors",
                  current
                    ? "border-primary/40 bg-accent text-accent-foreground"
                    : "bg-card text-muted-foreground hover:text-foreground",
                  problems > 0 && "border-destructive/40 text-destructive",
                )}
              >
                <span aria-hidden className="tabular text-xs opacity-70">
                  {problems ? "!" : index + 1}
                </span>
                {section.title}
                {problems ? (
                  // The count, not only the colour, says this step needs attention.
                  <span aria-hidden className="tabular text-xs">
                    ({problems})
                  </span>
                ) : null}
              </button>
            </li>
          );
        })}
      </ol>
    </nav>
  );
}

/**
 * One section of a long editor. Unmounted while another section is showing, unless
 * `keepMounted`: use that for a section that holds state of its own which Save must
 * still read (the sharing fields), so it is hidden instead of discarded.
 */
export function SectionCard<Id extends string>({
  id,
  active,
  title,
  description,
  previous,
  next,
  onGo,
  keepMounted = false,
  children,
}: {
  id: Id;
  active: Id;
  title: string;
  description?: ReactNode;
  previous?: { id: Id; title: string };
  next?: { id: Id; title: string };
  onGo: (id: Id) => void;
  keepMounted?: boolean;
  children: ReactNode;
}) {
  const shown = active === id;
  if (!shown && !keepMounted) return null;
  return (
    <Card
      id={`section-${id}`}
      hidden={!shown}
      className="scroll-mt-20"
      aria-labelledby={`section-${id}-title`}
    >
      <CardHeader>
        <h2 id={`section-${id}-title`} className="text-[15px] font-semibold leading-6">
          {title}
        </h2>
        {description ? <p className="text-[13px] text-muted-foreground">{description}</p> : null}
      </CardHeader>
      <CardContent className="space-y-5">
        {children}
        {/* Two long section names do not fit side by side on a phone: Next wraps below. */}
        <div className="flex flex-wrap items-center justify-between gap-x-3 gap-y-2 border-t pt-4">
          {previous ? (
            <Button type="button" variant="ghost" onClick={() => onGo(previous.id)}>
              <ChevronLeft /> {previous.title}
            </Button>
          ) : null}
          {next ? (
            <Button type="button" variant="outline" className="ml-auto" onClick={() => onGo(next.id)}>
              {next.title} <ChevronRight />
            </Button>
          ) : null}
        </div>
      </CardContent>
    </Card>
  );
}

/** The neighbours of a section in an ordered list, for `SectionCard`'s Back/Next. */
export function sectionNeighbours<Id extends string>(
  order: readonly Id[],
  titles: Record<Id, string>,
  id: Id,
): { previous?: { id: Id; title: string }; next?: { id: Id; title: string } } {
  const index = order.indexOf(id);
  const previous = order[index - 1];
  const next = order[index + 1];
  return {
    previous: previous ? { id: previous, title: titles[previous] } : undefined,
    next: next ? { id: next, title: titles[next] } : undefined,
  };
}
