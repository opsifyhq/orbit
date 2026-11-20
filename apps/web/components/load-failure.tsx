"use client";

import type { ReactNode } from "react";
import Link from "next/link";
import { CloudOff, RefreshCw, ShieldX } from "lucide-react";

import { errorMessage } from "@orbit/api-client";
import { Button } from "@/components/ui/button";
import { EmptyState } from "@/components/ui/empty-state";
import { errorStatus } from "@/lib/http";

/**
 * What to show when the object a page is about did not load. Only a 404 means "not
 * found"; no access is its own answer, and anything else (5xx, offline) can be retried.
 *
 * Render it only while there is no data at all. A failed background refetch must not
 * replace a page (or a half-filled form) that is already on screen.
 */
export function LoadFailure({
  noun,
  error,
  status = errorStatus(error),
  message,
  back,
  onRetry,
  retrying,
  extra,
}: {
  /** Lower-case name of the object: "agent", "execution". */
  noun: string;
  error?: unknown;
  /** Override when the status comes from somewhere other than `error` (a stream). */
  status?: number;
  message?: string;
  back: { href: string; label: string };
  onRetry?: () => void;
  retrying?: boolean;
  extra?: ReactNode;
}) {
  const backButton = (
    <Button asChild variant="outline">
      <Link href={back.href}>{back.label}</Link>
    </Button>
  );
  const title = noun.charAt(0).toUpperCase() + noun.slice(1);

  if (status === 403) {
    return (
      <EmptyState
        icon={<ShieldX />}
        title={`You don't have access to this ${noun}`}
        description="Ask its owner or an administrator to share it with you."
        action={backButton}
      />
    );
  }
  if (status === 404 || (status === undefined && !error)) {
    return (
      <EmptyState
        title={`${title} not found`}
        description="It may have been deleted, or the link is wrong."
        action={backButton}
      />
    );
  }
  // 401 is already being redirected to sign-in; 5xx and network errors are worth another try.
  return (
    <EmptyState
      icon={<CloudOff />}
      title={`Couldn't load this ${noun}`}
      description={message ?? (error ? errorMessage(error) : undefined)}
      action={
        <div className="flex flex-wrap justify-center gap-2">
          {onRetry ? (
            <Button onClick={onRetry} loading={retrying}>
              <RefreshCw /> Retry
            </Button>
          ) : null}
          {backButton}
          {extra}
        </div>
      }
    />
  );
}
