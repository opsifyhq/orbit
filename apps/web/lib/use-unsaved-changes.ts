"use client";

import { useEffect, useRef } from "react";
import { useRouter } from "next/navigation";

import { useConfirm } from "@/components/ui/confirm";

/**
 * Protects a long form from being lost by accident.
 *
 * While `dirty`, closing or reloading the tab asks the browser to confirm, and a click
 * on a link that leaves the page asks in-app first. Saving navigates in code
 * (`router.push`), which is not intercepted, so a successful save never prompts.
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

  useEffect(() => {
    if (!dirty) return;
    const onBeforeUnload = (event: BeforeUnloadEvent) => {
      event.preventDefault();
      // Older browsers need a value; the text itself is never shown.
      event.returnValue = "";
    };
    window.addEventListener("beforeunload", onBeforeUnload);
    return () => window.removeEventListener("beforeunload", onBeforeUnload);
  }, [dirty]);

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
      if (url.pathname === window.location.pathname) return;
      event.preventDefault();
      event.stopPropagation();
      void confirm({
        title: "Leave without saving?",
        description: "Your changes on this page have not been saved and will be lost.",
        confirmLabel: "Leave page",
        cancelLabel: "Keep editing",
        destructive: true,
      }).then((leave) => {
        if (!leave) return;
        dirtyRef.current = false;
        router.push(`${url.pathname}${url.search}${url.hash}`);
      });
    };
    // Capture phase: before Next's own link handler starts the navigation.
    document.addEventListener("click", onClick, true);
    return () => document.removeEventListener("click", onClick, true);
  }, [confirm, router]);

  return dirty;
}
