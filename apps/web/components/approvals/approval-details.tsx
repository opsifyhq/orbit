"use client";

import { Badge } from "@/components/ui/badge";
import { Callout } from "@/components/ui/callout";
import { approvalArguments, rowValue, truncationNotice } from "@/lib/approval-arguments";
import { brandName, effectLabel, effectTone } from "@/lib/presentation";
import { titleCase } from "@/lib/utils";
import type { ApprovalWithExecution } from "@/lib/types";

export function ApprovalDetails({ approval }: { approval: ApprovalWithExecution }) {
  const action = (approval.requestedAction ?? {}) as {
    capability?: string;
    connector?: string;
    connectionName?: string;
    effect?: string;
    arguments?: unknown;
    argumentsBytes?: unknown;
    argumentsTruncated?: unknown;
  };
  const shown = approvalArguments(action);
  const args = shown.args;
  const notice = truncationNotice(shown);
  const rows = actionRows(action.capability, args);
  const tone = effectTone(action.effect);
  return (
    <div className="space-y-3 rounded-lg border bg-subtle p-3 text-[13px]">
      <div className="flex flex-wrap items-center gap-x-4 gap-y-1.5">
        <Badge variant={tone === "safe" ? "secondary" : "warning"}>
          {effectLabel(action.effect)}
        </Badge>
        <span>
          <span className="text-muted-foreground">Agent</span> {approval.agentName}
        </span>
        {action.connectionName ? (
          <span>
            <span className="text-muted-foreground">Connection</span> {action.connectionName}
          </span>
        ) : null}
        {action.connector ? (
          <span>
            <span className="text-muted-foreground">Connector</span> {brandName(action.connector)}
          </span>
        ) : null}
      </div>
      {rows.length ? (
        <dl className="grid gap-x-4 gap-y-1.5 sm:grid-cols-[6rem_minmax(0,1fr)]">
          {rows.map(([label, value]) => (
            <div key={label} className="contents">
              <dt className="text-muted-foreground">{titleCase(label)}</dt>
              <dd className="break-words font-mono text-xs leading-relaxed">{value}</dd>
            </div>
          ))}
        </dl>
      ) : null}
      {notice ? (
        <Callout tone="warning" title="Arguments truncated">
          {notice}
        </Callout>
      ) : null}
      {shown.text ? (
        <details open={shown.truncated}>
          <summary className="w-fit cursor-pointer select-none text-muted-foreground hover:text-foreground">
            Full arguments
          </summary>
          <pre className="code-block mt-1.5 max-h-48 whitespace-pre-wrap break-words bg-card">
            {shown.text}
          </pre>
        </details>
      ) : null}
    </div>
  );
}

function actionRows(
  capability: string | undefined,
  args: Record<string, unknown>,
): Array<[string, string]> {
  const rows: Array<[string, string]> = [];
  const push = (label: string, value: unknown) => {
    if (value !== undefined && value !== null && String(value).trim() !== "") {
      rows.push([label, rowValue(value)]);
    }
  };
  // Where it happens comes first: the same action on another repository is another decision.
  push("repository", [args.owner, args.repo ?? args.project].filter(Boolean).join("/"));
  if (capability === "repo.commit") push("message", args.message);
  if (capability === "repo.push") push("branch", args.branch);
  if (capability === "pulls.create") {
    push("title", args.title);
    push("head", args.head);
    push("base", args.base);
    push("draft", args.draft);
  }
  if (capability?.startsWith("issues.")) push("number", args.number);
  if (capability === "issues.update" || capability === "issues.create") {
    push("state", args.state);
    push("labels", Array.isArray(args.labels) ? args.labels.join(", ") : args.labels);
    push("assignees", Array.isArray(args.assignees) ? args.assignees.join(", ") : args.assignees);
  }
  if (capability === "issues.comment") push("body", args.body);
  return rows;
}
