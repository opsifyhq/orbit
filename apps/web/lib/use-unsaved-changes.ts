"use client";

import { useCallback, useEffect, useRef } from "react";
import { useRouter } from "next/navigation";

import { useConfirm } from "@/components/ui/confirm";
import { isSamePage, setNavigationGuard } from "@/lib/navigation-guard";

/**
 * Protects a long form from being lost by accident.
 *
 * While `dirty`:
 * - closing or reloading the tab asks the browser to confirm;
 * - a click on a link that leaves the page asks in-app first;
 * - browser Back and Forward to another page ask in-app first;
 * - code that navigates through `confirmNavigation` (the command palette) asks the same.
 *
 * Saving navigates in code (`router.push`) without `confirmNavigation`, so a successful
 * save never prompts.
 *
 * Pass the form's state as `value`; it is compared with what the form started with.
 */
export function useUnsavedChanges(value: unknown, options: { enabled?: boolean } = {}) {
  const confirm = useConfirm();
  const router = useRouter();
  const serialized = JSON.stringify(value);
  const initial = useRef(serialized);
  const dirty = (options.enabled ?? true) && serialized !== initial.current;
  const dirtyRef = useRef(dirty);
  dirtyRef.current = dirty;

  const askToLeave = useCallback(async () => {
    const leave = await confirm({
      title: "Leave without saving?",
      description: "Your changes on this page have not been saved and will be lost.",
      confirmLabel: "Leave page",
      cancelLabel: "Keep editing",
      destructive: true,
    });
    if (leave) dirtyRef.current = false;
    return leave;
  }, [confirm]);

  useEffect(() => {
    if (!dirty) return;
    const onBeforeUnload = (event: BeforeUnloadEvent) => {
      event.preventDefault();
      // Older browsers need a value; the text itself is never shown.
      event.returnValue = "";
    };
    window.addEventListener("beforeunload", onBeforeUnload);
    const removeGuard = setNavigationGuard(() => (dirtyRef.current ? askToLeave() : Promise.resolve(true)));
    return () => {
      window.removeEventListener("beforeunload", onBeforeUnload);
      removeGuard();
    };
  }, [dirty, askToLeave]);

  useEffect(() => {
    const onClick = (event: MouseEvent) => {
      if (!dirtyRef.current || event.defaultPrevented) return;
      // Leave new-tab clicks and modified clicks alone: they do not leave this page.
      if (event.button !== 0 || event.metaKey || event.ctrlKey || event.shiftKey || event.altKey) return;
      const anchor = (event.target as HTMLElement | null)?.closest?.("a[href]") as HTMLAnchorElement | null;
      if (!anchor || anchor.target === "_blank" || anchor.hasAttribute("download")) return;
      const url = new URL(anchor.href, window.location.href);
      if (url.origin !== window.location.origin) return;
      // Same page (a section hash, a query change): nothing is lost.
      if (isSamePage(url.href, window.location.href)) return;
      event.preventDefault();
      event.stopPropagation();
      void askToLeave().then((leave) => {
        if (leave) router.push(`${url.pathname}${url.search}${url.hash}`);
      });
    };
    // Capture phase: before Next's own link handler starts the navigation.
    document.addEventListener("click", onClick, true);
    return () => document.removeEventListener("click", onClick, true);
  }, [askToLeave, router]);

  useEffect(() => {
    // The entry this form lives on, to put back when the user keeps editing. Section
    // hashes change on this page, so it is refreshed whenever history stays here.
    let page = { href: window.location.href, state: window.history.state as unknown };
    const onPopState = (event: PopStateEvent) => {
      if (isSamePage(window.location.href, page.href)) {
        page = { href: window.location.href, state: event.state };
        return;
      }
      if (!dirtyRef.current) return;
      // Back or Forward has already moved the URL. Stop Next's router from rendering the
      // other page (this capture listener on window runs before its own), then ask.
      event.stopImmediatePropagation();
      const target = event.state as unknown;
      void askToLeave().then((leave) => {
        if (leave) {
          // Let the router render the page the browser moved to.
          window.dispatchEvent(new PopStateEvent("popstate", { state: target }));
        } else {
          // Put this page back on top of history; the draft never unmounted.
          window.history.pushState(page.state, "", page.href);
        }
      });
    };
    window.addEventListener("popstate", onPopState, true);
    return () => window.removeEventListener("popstate", onPopState, true);
  }, [askToLeave]);

  return dirty;
}
