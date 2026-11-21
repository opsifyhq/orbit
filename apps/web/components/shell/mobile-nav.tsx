"use client";

import * as React from "react";
import Link from "next/link";
import * as DialogPrimitive from "@radix-ui/react-dialog";
import { Menu, Play, Search, X } from "lucide-react";

import { BrandMark, Wordmark } from "@/components/shell/brand";
import { useCommandPalette } from "@/components/shell/command-palette";
import {
  MOBILE_PRIMARY,
  NAV_GROUPS,
  NAV_ITEMS,
  isActivePath,
  isWorkPath,
} from "@/components/shell/nav";
import { NavLink, type NavSignals } from "@/components/shell/sidebar";
import { UserMenuCompact } from "@/components/shell/user-menu";
import { DialogOverlay, DialogPortal, useRestoreFocus } from "@/components/ui/dialog";
import { cn } from "@/lib/utils";

/** Phone top bar: where you are, search, account. Nothing else competes for it. */
export function MobileHeader() {
  const palette = useCommandPalette();
  return (
    <header
      data-ink
      className="sticky top-0 z-30 flex h-14 items-center gap-2 border-b border-ink-border bg-ink px-3 text-ink-foreground lg:hidden"
    >
      <Link href="/" aria-label="ORBIT overview" className="flex h-9 items-center gap-2.5 rounded-md px-1">
        <BrandMark />
        <Wordmark />
      </Link>
      <span className="flex-1" />
      <button
        type="button"
        onClick={palette.open}
        aria-label="Search"
        className="flex h-9 w-9 items-center justify-center rounded-md text-ink-muted transition-colors hover:bg-ink-raised hover:text-ink-foreground"
      >
        <Search className="h-[18px] w-[18px]" />
      </button>
      <UserMenuCompact />
    </header>
  );
}

/**
 * Phone bottom bar: the four daily destinations plus "More", which opens every
 * other page. Nothing in the product is desktop-only.
 */
export function MobileTabBar({ pathname, signals }: { pathname: string; signals: NavSignals }) {
  const [open, setOpen] = React.useState(false);
  const focus = useRestoreFocus();
  const primary = MOBILE_PRIMARY.map((href) => NAV_ITEMS.find((item) => item.href === href)!);
  const inMore =
    !primary.some((item) => isActivePath(pathname, item.href)) || isWorkPath(pathname);

  React.useEffect(() => {
    setOpen(false);
  }, [pathname]);

  const tabClass = (active: boolean) =>
    cn(
      "relative flex min-h-[56px] flex-col items-center justify-center gap-1 text-xs font-medium transition-colors",
      active ? "text-ink-foreground" : "text-ink-muted",
    );

  return (
    <>
      <nav
        data-ink
        aria-label="Primary"
        className="safe-bottom fixed inset-x-0 bottom-0 z-40 grid grid-cols-5 border-t border-ink-border bg-ink lg:hidden"
      >
        {primary.map((item) => {
          const active = isActivePath(pathname, item.href) && !isWorkPath(pathname);
          const badge =
            item.href === "/approvals"
              ? signals.pendingApprovals
              : item.href === "/executions"
                ? signals.runningExecutions
                : 0;
          return (
            <Link
              key={item.href}
              href={item.href}
              aria-current={active ? "page" : undefined}
              className={tabClass(active)}
            >
              {active ? <span className="absolute inset-x-5 top-0 h-0.5 rounded-full bg-ink-accent" aria-hidden /> : null}
              <span className="relative">
                <item.icon className={cn("h-5 w-5", active && "text-ink-accent")} />
                {badge > 0 ? (
                  <span
                    className={cn(
                      "tabular absolute -right-2.5 -top-1.5 flex h-[18px] min-w-[18px] items-center justify-center rounded-full px-1 text-xs font-bold leading-none text-ink",
                      item.href === "/approvals" ? "bg-warning" : "bg-info",
                    )}
                  >
                    {badge > 9 ? "9+" : badge}
                  </span>
                ) : null}
              </span>
              {item.label}
            </Link>
          );
        })}
        <button
          type="button"
          onClick={() => setOpen(true)}
          aria-haspopup="dialog"
          aria-expanded={open}
          className={tabClass(inMore)}
        >
          {inMore ? <span className="absolute inset-x-5 top-0 h-0.5 rounded-full bg-ink-accent" aria-hidden /> : null}
          <Menu className={cn("h-5 w-5", inMore && "text-ink-accent")} />
          More
        </button>
      </nav>

      <DialogPrimitive.Root open={open} onOpenChange={setOpen}>
        <DialogPortal>
          <DialogOverlay className="lg:hidden" />
          <DialogPrimitive.Content
            onOpenAutoFocus={focus.remember}
            onCloseAutoFocus={focus.restore}
            data-ink
            aria-describedby={undefined}
            className="safe-bottom fixed inset-x-0 bottom-0 z-50 flex max-h-[88dvh] flex-col rounded-t-2xl border-t border-ink-border bg-ink text-ink-foreground data-[state=open]:animate-sheet-in lg:hidden"
          >
            <div className="flex items-center justify-between px-4 pb-1 pt-3">
              <DialogPrimitive.Title className="font-display text-base">All pages</DialogPrimitive.Title>
              <DialogPrimitive.Close
                aria-label="Close"
                className="flex h-9 w-9 items-center justify-center rounded-md text-ink-muted hover:bg-ink-raised hover:text-ink-foreground"
              >
                <X className="h-5 w-5" />
              </DialogPrimitive.Close>
            </div>
            <div className="min-h-0 flex-1 overflow-y-auto px-3 pb-4">
              <Link
                href="/work"
                onClick={() => setOpen(false)}
                className="mb-1 mt-2 flex h-11 items-center justify-center gap-2 rounded-md bg-primary text-[15px] font-semibold text-primary-foreground"
              >
                <Play className="h-4 w-4" /> Start work
              </Link>
              {NAV_GROUPS.map((group) => (
                <div key={group.id} className="mt-4">
                  {group.label ? (
                    <p className="px-2.5 pb-1 text-xs font-medium text-ink-muted">{group.label}</p>
                  ) : null}
                  <ul>
                    {group.items.map((item) => (
                      <li key={item.href}>
                        <NavLink
                          item={item}
                          size="lg"
                          active={isActivePath(pathname, item.href)}
                          signals={signals}
                          onNavigate={() => setOpen(false)}
                        />
                      </li>
                    ))}
                  </ul>
                </div>
              ))}
            </div>
          </DialogPrimitive.Content>
        </DialogPortal>
      </DialogPrimitive.Root>
    </>
  );
}
