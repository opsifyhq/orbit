"use client";

import Link from "next/link";
import { useEffect, useRef, useState } from "react";
import { ChevronDown, Play, Search } from "lucide-react";

import { BrandMark, Wordmark } from "@/components/shell/brand";
import { useCommandPalette } from "@/components/shell/command-palette";
import { NAV_GROUPS, isActivePath,
  isWorkPath, type NavItem } from "@/components/shell/nav";
import { UserMenu } from "@/components/shell/user-menu";
import { Kbd } from "@/components/ui/kbd";
import { StatusGlyph } from "@/components/ui/status";
import { cn } from "@/lib/utils";

export interface NavSignals {
  pendingApprovals: number;
  runningExecutions: number;
}

/** Live counts beside the two destinations where something may be waiting. */
export function NavSignal({ href, signals }: { href: string; signals: NavSignals }) {
  if (href === "/approvals" && signals.pendingApprovals > 0) {
    return (
      <span
        className="tabular min-w-5 rounded-full bg-warning px-1.5 text-center text-xs font-semibold leading-5 text-ink"
        aria-label={`${signals.pendingApprovals} waiting`}
      >
        {signals.pendingApprovals > 99 ? "99+" : signals.pendingApprovals}
      </span>
    );
  }
  if (href === "/executions" && signals.runningExecutions > 0) {
    return (
      <span
        className="tabular inline-flex items-center gap-1.5 text-xs font-medium text-info"
        aria-label={`${signals.runningExecutions} running`}
      >
        <StatusGlyph shape="orbiting" tone="info" />
        {signals.runningExecutions}
      </span>
    );
  }
  return null;
}

export function NavLink({
  item,
  active,
  signals,
  onNavigate,
  size = "default",
}: {
  item: NavItem;
  active: boolean;
  signals: NavSignals;
  onNavigate?: () => void;
  size?: "default" | "lg";
}) {
  return (
    <Link
      href={item.href}
      onClick={onNavigate}
      aria-current={active ? "page" : undefined}
      className={cn(
        "group relative flex items-center gap-2.5 rounded-md px-2.5 text-sm font-medium transition-colors",
        size === "lg" ? "h-11 text-[15px]" : "h-[34px]",
        active
          ? "bg-ink-raised text-ink-foreground"
          : "text-ink-muted hover:bg-ink-raised/60 hover:text-ink-foreground",
      )}
    >
      {active ? (
        <span className="absolute inset-y-2 left-0 w-0.5 rounded-full bg-ink-accent" aria-hidden />
      ) : null}
      <item.icon
        className={cn("h-4 w-4 shrink-0", active ? "text-ink-accent" : "text-ink-muted group-hover:text-ink-foreground")}
      />
      <span className="min-w-0 flex-1 truncate">{item.label}</span>
      <NavSignal href={item.href} signals={signals} />
    </Link>
  );
}

const COLLAPSE_KEY = "orbit.nav.collapsed";
const DEFAULT_COLLAPSED: Record<string, boolean> = { infrastructure: true, admin: true };

export function Sidebar({ pathname, signals }: { pathname: string; signals: NavSignals }) {
  const palette = useCommandPalette();
  // Setup pages are visited rarely: they start folded so the daily ones stand out.
  // The choice is remembered per browser.
  const [collapsed, setCollapsed] = useState<Record<string, boolean>>(DEFAULT_COLLAPSED);
  useEffect(() => {
    try {
      const stored = window.localStorage.getItem(COLLAPSE_KEY);
      if (stored) setCollapsed({ ...DEFAULT_COLLAPSED, ...JSON.parse(stored) });
    } catch {
      // Unreadable preference: keep the defaults.
    }
  }, []);
  // Keep the current page's entry in view when the window is short.
  const navRef = useRef<HTMLElement>(null);
  useEffect(() => {
    navRef.current?.querySelector('[aria-current="page"]')?.scrollIntoView({ block: "nearest" });
  }, [pathname]);

  function toggleGroup(id: string) {
    setCollapsed((current) => {
      const next = { ...current, [id]: !current[id] };
      try {
        window.localStorage.setItem(COLLAPSE_KEY, JSON.stringify(next));
      } catch {
        // Not persisted; still applies for this visit.
      }
      return next;
    });
  }
  return (
    <aside
      data-ink
      className="fixed inset-y-0 left-0 z-40 hidden w-[15.5rem] flex-col border-r border-ink-border bg-ink text-ink-foreground lg:flex"
    >
      <div className="flex h-14 shrink-0 items-center px-4">
        <Link href="/" className="flex items-center gap-2.5 rounded-md" aria-label="ORBIT overview">
          <BrandMark />
          <Wordmark />
        </Link>
      </div>

      <div className="space-y-2 px-3 pb-3">
        <button
          type="button"
          onClick={palette.open}
          className="flex h-9 w-full items-center gap-2 rounded-md border border-ink-border bg-ink-raised/60 px-2.5 text-[13px] text-ink-muted transition-colors hover:border-ink-muted/40 hover:text-ink-foreground"
        >
          <Search className="h-4 w-4" />
          <span className="flex-1 text-left">Search or jump to</span>
          <Kbd className="border-ink-muted/40 text-ink-muted">⌘K</Kbd>
        </button>
        <Link
          href="/work"
          aria-current={isWorkPath(pathname) ? "page" : undefined}
          className="flex h-9 w-full items-center justify-center gap-2 rounded-md bg-primary text-sm font-semibold text-primary-foreground transition-colors hover:bg-primary/90"
        >
          <Play className="h-4 w-4" /> Start work
        </Link>
      </div>

      <nav ref={navRef} aria-label="Main" className="nav-scroll min-h-0 flex-1 overflow-y-auto px-3 pb-4">
        {NAV_GROUPS.map((group) => {
          const hasActive = group.items.some((item) => isActivePath(pathname, item.href));
          // A group holding the current page is always shown, whatever was collapsed.
          const open = !group.label || hasActive || !collapsed[group.id];
          return (
            <div key={group.id} className="mt-3 first:mt-1">
              {group.label ? (
                <button
                  type="button"
                  aria-expanded={open}
                  onClick={() => toggleGroup(group.id)}
                  className="flex h-7 w-full items-center justify-between rounded-md px-2.5 text-xs font-medium text-ink-muted transition-colors hover:text-ink-foreground"
                >
                  {group.label}
                  <ChevronDown className={cn("h-3.5 w-3.5 transition-transform", !open && "-rotate-90")} />
                </button>
              ) : null}
              {open ? (
                <ul className="space-y-0.5">
                  {group.items.map((item) => (
                    <li key={item.href}>
                      <NavLink item={item} active={isActivePath(pathname, item.href)} signals={signals} />
                    </li>
                  ))}
                </ul>
              ) : null}
            </div>
          );
        })}
      </nav>

      <div className="shrink-0 border-t border-ink-border p-2">
        <UserMenu />
      </div>
    </aside>
  );
}
