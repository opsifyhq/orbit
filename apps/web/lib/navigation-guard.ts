/**
 * One place that decides whether in-app navigation may leave the current page.
 *
 * A form with unsaved changes registers a guard (see `use-unsaved-changes.ts`); code that
 * navigates with `router.push` outside a link (the command palette) asks
 * `confirmNavigation` first, so it gets the same "Leave without saving?" prompt as a link.
 */

export type NavigationGuard = () => Promise<boolean>;

let active: NavigationGuard | null = null;

/** Register the page's guard; the returned function removes it (and only it). */
export function setNavigationGuard(guard: NavigationGuard): () => void {
  active = guard;
  return () => {
    if (active === guard) active = null;
  };
}

/**
 * Resolve to true when navigation to `target` may go ahead. A target on the current page
 * (a section hash, a query change) never asks: nothing on the page is lost.
 */
export async function confirmNavigation(target: string, current: string = currentHref()): Promise<boolean> {
  if (!active || isSamePage(target, current)) return true;
  return active();
}

/** True when `target` stays on the page `current` shows (same origin and path). */
export function isSamePage(target: string, current: string): boolean {
  const from = new URL(current);
  const to = new URL(target, from);
  return to.origin === from.origin && to.pathname === from.pathname;
}

function currentHref() {
  return typeof window === "undefined" ? "http://localhost/" : window.location.href;
}
