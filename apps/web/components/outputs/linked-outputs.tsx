"use client";

import Link from "next/link";

import { Badge } from "@/components/ui/badge";
import { Card, CardContent, CardHeader, CardTitle } from "@/components/ui/card";
import { LinkOutputDialog } from "./link-output-dialog";
import { eventsLabel, kindLabel } from "@/lib/output-meta";
import type { OutputView } from "@/lib/outputs";

/**
 * Outputs scoped to one agent or team. They are ordinary Output resources; this card only
 * makes the link discoverable from the resource itself.
 */
export function LinkedOutputs({
  outputs,
  scope,
  id,
}: {
  outputs: OutputView[];
  scope: "agent" | "team";
  id: string;
}) {
  const linked = outputs.filter((output) =>
    scope === "agent"
      ? (output.agentIds ?? (output.agentId ? [output.agentId] : [])).includes(id)
      : output.teamId === id,
  );
  const addHref = `/outputs/new?${scope === "agent" ? "agent" : "team"}=${id}`;
  return (
    <Card>
      <CardHeader className="flex-row flex-wrap items-center justify-between gap-2 space-y-0">
        <CardTitle as="h2">Outputs</CardTitle>
        <div className="flex items-center gap-3 text-[13px] font-medium">
          <Link href={addHref} className="text-primary underline-offset-4 hover:underline">
            Add output
          </Link>
          <LinkOutputDialog outputs={outputs} scope={scope} id={id} />
          <Link href="/outputs" className="text-muted-foreground underline-offset-4 hover:underline">
            Manage
          </Link>
        </div>
      </CardHeader>
      <CardContent>
        {linked.length ? (
          <ul className="divide-y">
            {linked.map((output) => (
              <li key={output.id} className="flex flex-wrap items-center justify-between gap-2 py-2 first:pt-0 last:pb-0">
                <Link href={`/outputs/${output.id}`} className="truncate text-sm font-medium hover:underline">
                  {output.name}
                </Link>
                <span className="flex items-center gap-2 text-[13px] text-muted-foreground">
                  <Badge variant="outline">{kindLabel(output.kind)}</Badge>
                  <span>{eventsLabel(output.events)}</span>
                  {!output.enabled ? <Badge variant="secondary">Off</Badge> : null}
                </span>
              </li>
            ))}
          </ul>
        ) : (
          <p className="text-sm text-muted-foreground">
            Nothing is notified when this {scope === "agent" ? "agent" : "team"} finishes.
          </p>
        )}
      </CardContent>
    </Card>
  );
}
