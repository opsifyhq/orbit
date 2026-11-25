/**
 * What a create or edit page does after the resource itself was saved, given whether the
 * deferred access (sharing) change was saved too. A refused access change is never
 * reported as a plain success:
 * - edit: stay on the form, so the pending access edit is still there to fix or retry;
 * - create: the resource exists now, so go to its edit page (creating again would
 *   duplicate it) and say that access still needs to be set.
 */
export type ResourceSaveOutcome = {
  tone: "success" | "warning";
  message: string;
  /** Where to go next; null keeps the user on the current form. */
  href: string | null;
};

export function resourceSaveOutcome(options: {
  mode: "create" | "edit";
  /** Success text when everything saved, e.g. "Agent created". */
  saved: string;
  /** The resource's page, e.g. `/agents/<id>`. */
  href: string;
  accessSaved: boolean;
}): ResourceSaveOutcome {
  const { mode, saved, href, accessSaved } = options;
  if (accessSaved) return { tone: "success", message: saved, href };
  if (mode === "edit") {
    return {
      tone: "warning",
      message: `${saved}, but access could not be updated. Fix it and use Save access.`,
      href: null,
    };
  }
  return {
    tone: "warning",
    message: `${saved}, but access could not be updated. Set it again here.`,
    href: `${href}/edit`,
  };
}
