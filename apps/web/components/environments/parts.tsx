"use client";

import Link from "next/link";
import { Cable, ChevronDown, KeyRound, TriangleAlert } from "lucide-react";

import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuTrigger,
} from "@/components/ui/dropdown-menu";
import type { EnvMappingEntry } from "@/lib/env-mappings";
import { brandName } from "@/lib/presentation";

/** Where a variable's value is read from, with a warning when that source is gone. */
export function VariableSource({
  entry,
  connection,
  connectionsLoaded,
}: {
  entry: EnvMappingEntry;
  connection?: { name: string; connectorSlug: string; hasCredential: boolean };
  connectionsLoaded: boolean;
}) {
  if (entry.connectionId) {
    if (!connection) {
      return connectionsLoaded ? (
        <span className="inline-flex items-center gap-1.5 text-[13px] text-warning">
          <TriangleAlert className="h-3.5 w-3.5 shrink-0" /> Its connection no longer exists
        </span>
      ) : (
        <span className="text-[13px] text-muted-foreground">From a connection</span>
      );
    }
    return (
      <span className="flex min-w-0 flex-wrap items-center gap-x-2 gap-y-1 text-[13px] text-muted-foreground">
        <Cable className="h-3.5 w-3.5 shrink-0" />
        <span className="min-w-0 truncate text-foreground">{connection.name}</span>
        <Badge variant="secondary">{brandName(connection.connectorSlug)}</Badge>
        {connection.hasCredential ? null : <Badge variant="warning">No credential</Badge>}
      </span>
    );
  }
  return (
    <span className="flex min-w-0 items-center gap-2 text-[13px] text-muted-foreground">
      <KeyRound className="h-3.5 w-3.5 shrink-0" />
      <span className="shrink-0 whitespace-nowrap">Stored secret</span>
      <span className="min-w-0 truncate font-mono text-xs text-foreground">
        {entry.secretKey ?? entry.secretRef ?? ""}
      </span>
    </span>
  );
}

/** Which worker profiles use a mapping, and a way to attach it to another one. */
export function AttachedWorkers({
  mappingId,
  users,
  profiles,
}: {
  mappingId: string;
  users: Array<{ id: string; name: string }>;
  profiles: Array<{ id: string; name: string }>;
}) {
  const available = profiles.filter((profile) => !users.some((user) => user.id === profile.id));
  const linkClass = "font-medium text-foreground underline-offset-4 hover:underline";

  if (!profiles.length) {
    return (
      <span>
        No worker profiles yet.{" "}
        <Link href="/workers?create=1" className="font-medium text-primary underline-offset-4 hover:underline">
          Create one in Workers
        </Link>
      </span>
    );
  }

  return (
    <span className="flex flex-wrap items-center gap-x-2 gap-y-1">
      {users.length ? (
        <span>
          Used by{" "}
          {users.map((user, index) => (
            <span key={user.id}>
              {index ? ", " : null}
              <Link href={`/workers?edit=${user.id}`} className={linkClass}>
                {user.name}
              </Link>
            </span>
          ))}
        </span>
      ) : (
        <span>Not attached to a worker yet.</span>
      )}
      {available.length ? (
        <DropdownMenu>
          <DropdownMenuTrigger asChild>
            <Button size="sm" variant="link" className="h-auto p-0 text-[13px]">
              {users.length ? "Attach to another" : "Attach to a worker"}
              <ChevronDown />
            </Button>
          </DropdownMenuTrigger>
          <DropdownMenuContent align="start">
            {available.map((profile) => (
              <DropdownMenuItem key={profile.id} asChild>
                <Link href={`/workers?edit=${profile.id}&attach=${mappingId}`}>{profile.name}</Link>
              </DropdownMenuItem>
            ))}
          </DropdownMenuContent>
        </DropdownMenu>
      ) : null}
    </span>
  );
}
