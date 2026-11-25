/**
 * The team-runs list is an infinite query: "Load older runs" adds pages. Refetching an
 * infinite query refetches every loaded page, so polling it every few seconds while a
 * run is active multiplied the requests by the pages loaded. Instead only the newest
 * page is polled (`head`) and merged over the pages loaded earlier.
 */

type RunLike = { id: string; createdAt: string; status: string };
type PageLike<T extends RunLike> = { items: T[]; nextCursor?: string | null; total: number };

export const TERMINAL_RUN_STATUSES = new Set([
  "succeeded",
  "completed",
  "failed",
  "cancelled",
  "timed_out",
  "rejected",
]);

/** Newest first, as the API orders runs: `created_at DESC, id DESC`. */
function isOlder(run: RunLike, than: RunLike): boolean {
  const a = Date.parse(run.createdAt);
  const b = Date.parse(than.createdAt);
  if (a !== b) return a < b;
  return run.id < than.id;
}

/**
 * The runs to show: the polled newest page as it is now, then every run from the loaded
 * pages that is older than the newest page's last run. Runs inside the newest page's range
 * that it no longer lists (archived, deleted, filtered out) are dropped, and runs pushed
 * out of the first page by newer ones stay visible.
 */
export function mergeRunPages<T extends RunLike>(
  head: PageLike<T> | undefined,
  pages: PageLike<T>[],
): T[] {
  const loaded = pages.flatMap((page) => page.items);
  if (!head) return dedupe(loaded);
  if (!head.nextCursor || !head.items.length) return [...head.items];
  const last = head.items[head.items.length - 1];
  const seen = new Set(head.items.map((run) => run.id));
  const older = loaded.filter((run) => !seen.has(run.id) && isOlder(run, last));
  return dedupe([...head.items, ...older]);
}

function dedupe<T extends RunLike>(runs: T[]): T[] {
  const seen = new Set<string>();
  return runs.filter((run) => (seen.has(run.id) ? false : (seen.add(run.id), true)));
}

/** Whether any run is still going. */
export function hasActiveRun(runs: RunLike[]): boolean {
  return runs.some((run) => !TERMINAL_RUN_STATUSES.has(run.status));
}

/**
 * An active run in an older page is outside what the head poll refreshes; the loaded
 * pages are then refetched as a whole, but rarely.
 */
export function hasActiveRunOutsideHead<T extends RunLike>(
  head: PageLike<T> | undefined,
  pages: PageLike<T>[],
): boolean {
  if (pages.length <= 1) return false;
  const inHead = new Set(head?.items.map((run) => run.id) ?? []);
  return pages.slice(1).some((page) =>
    page.items.some((run) => !inHead.has(run.id) && !TERMINAL_RUN_STATUSES.has(run.status)),
  );
}

/**
 * Whether the newest page has moved past every run the loaded pages start with: more new
 * runs arrived than one page holds, so runs between the two could be missing. The
 * loaded pages are then refetched once.
 */
export function headOutrunsPages<T extends RunLike>(
  head: PageLike<T> | undefined,
  pages: PageLike<T>[],
): boolean {
  const first = pages[0]?.items[0];
  if (!head?.nextCursor || !head.items.length || !first) return false;
  if (head.items.some((run) => run.id === first.id)) return false;
  return isOlder(first, head.items[head.items.length - 1]);
}
