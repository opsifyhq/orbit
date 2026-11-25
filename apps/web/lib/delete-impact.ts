import { ApiError } from "./api-error.ts";

/** What a permanent agent delete would be refused for and what it would remove (#434). */
export interface DeleteImpact {
  usedBy: string[];
  removes: string[];
}

/**
 * The dependants named by a 409 "still used by" refusal (`details` with path `usedBy`), or an
 * empty list for any other error.
 */
export function usedByFromError(error: unknown): string[] {
  if (!(error instanceof ApiError) || error.status !== 409) return [];
  return (error.details ?? []).filter((detail) => detail.path === "usedBy").map((detail) => detail.message);
}

/** One sentence naming what still uses an item, for a toast after a refused delete. */
export function stillUsedMessage(noun: string, usedBy: string[]): string {
  return `This ${noun} is still used by ${usedBy.join(", ")}. Remove it there first.`;
}

/** The message for a failed delete: the dependants when the API listed them, else `fallback`. */
export function deleteFailureMessage(noun: string, error: unknown, fallback: string): string {
  const usedBy = usedByFromError(error);
  return usedBy.length ? stillUsedMessage(noun, usedBy) : fallback;
}
