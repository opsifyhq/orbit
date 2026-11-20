"use client";

import { useState, type ReactNode } from "react";
import { Archive, Trash2 } from "lucide-react";
import { toast } from "sonner";
import { errorMessage } from "@orbit/api-client";

import { Button } from "@/components/ui/button";
import { Checkbox } from "@/components/ui/checkbox";
import { useConfirm } from "@/components/ui/confirm";
import { request } from "@/lib/http";

type Resource = "agents" | "skills";
type Action = "archive" | "delete";
type BulkResult = {
  updated?: string[];
  deleted?: string[];
  unchanged?: string[];
  failed: Array<{ id: string; code: string; message: string }>;
};

function endpoint(resource: Resource, action: Action) {
  return `/api/v1/${resource}/bulk/${action}`;
}

function noun(count: number, word: string) {
  return `${count} ${word}${count === 1 ? "" : "s"}`;
}

function summary(result: BulkResult, action: Action) {
  const changed = (action === "archive" ? result.updated : result.deleted)?.length ?? 0;
  const unchanged = result.unchanged?.length ?? 0;
  const failed = result.failed.length;
  const parts: string[] = [];
  if (action === "archive") {
    parts.push(`${noun(changed, "agent")} archived`);
    if (unchanged) parts.push(`${noun(unchanged, "agent")} already archived`);
    if (failed) parts.push(`${noun(failed, "agent")} could not be archived`);
  } else {
    parts.push(`${noun(changed, "resource")} deleted`);
    if (failed) parts.push(`${noun(failed, "resource")} could not be deleted`);
  }
  return parts.join(", ");
}

export function BulkActionsBar({
  resource,
  selectedIds,
  allShownSelected,
  onToggleAllShown,
  onChangeAccess,
  onSucceeded,
  onClear,
  archivedIds = [],
}: {
  resource: Resource;
  selectedIds: string[];
  allShownSelected: boolean;
  onToggleAllShown: (checked: boolean) => void;
  onChangeAccess: () => void;
  /** Only ids reported changed or already in the target state are removed from selection. */
  onSucceeded: (ids: string[]) => void;
  onClear: () => void;
  /** Agent ids already archived; used to disable a no-op archive. */
  archivedIds?: string[];
}) {
  const confirm = useConfirm();
  const [busy, setBusy] = useState<Action | null>(null);
  const alreadyArchived = archivedIds.length;

  function call(action: Action, dryRun: boolean) {
    return request<BulkResult>(endpoint(resource, action), {
      method: "POST",
      body: JSON.stringify({ ids: selectedIds, dryRun }),
    });
  }

  function describe(preview: BulkResult, action: Action): ReactNode {
    const changed = (action === "archive" ? preview.updated : preview.deleted)?.length ?? 0;
    const unchanged = preview.unchanged?.length ?? 0;
    const failed = preview.failed.length;
    return (
      <div className="space-y-2">
        <ul className="list-disc space-y-1 pl-5">
          {changed ? <li>{noun(changed, action === "archive" ? "agent" : "resource")} will be {action === "archive" ? "archived" : "deleted"}.</li> : null}
          {unchanged ? <li>{noun(unchanged, "agent")} {unchanged === 1 ? "is" : "are"} already archived and will not change.</li> : null}
          {failed ? <li>{noun(failed, "resource")} cannot be {action === "archive" ? "archived" : "deleted"} and will stay selected.</li> : null}
        </ul>
        {action === "archive" ? (
          <p>
            Archived agents disappear from the normal list and the Archived filter keeps them
            available. Their versions and execution history are kept, and you can restore them.
          </p>
        ) : (
          <p>
            This permanently removes the definitions and their history. It cannot be undone.
            Anything the server refuses stays selected so you can review it.
          </p>
        )}
      </div>
    );
  }

  async function run(action: Action) {
    setBusy(action);
    try {
      // Preview first: the dialog explains mixed states and the confirm button can name
      // the real number of items that will change.
      const preview = await call(action, true);
      const changed = (action === "archive" ? preview.updated : preview.deleted)?.length ?? 0;
      if (!changed) {
        toast.warning(action === "archive" ? "Nothing to archive" : "Nothing can be deleted", {
          description: preview.failed[0]?.message,
        });
        return;
      }
      const confirmed = await confirm({
        title:
          action === "archive"
            ? `Archive ${noun(changed, "agent")}?`
            : `Permanently delete ${noun(changed, "resource")}?`,
        description: describe(preview, action),
        confirmLabel: action === "archive" ? `Archive ${changed}` : `Delete ${changed} permanently`,
        destructive: action === "delete",
      });
      if (!confirmed) return;

      const result = await call(action, false);
      const done = [
        ...(result.updated ?? []),
        ...(result.deleted ?? []),
        ...(result.unchanged ?? []),
      ];
      if (result.failed.length) {
        toast.warning(summary(result, action), { description: result.failed[0]?.message });
      } else {
        toast.success(action === "archive" ? "Agents archived" : "Resources deleted");
      }
      onSucceeded(done);
    } catch (error) {
      toast.error(errorMessage(error));
    } finally {
      setBusy(null);
    }
  }

  return (
    <div className="flex flex-wrap items-center gap-2 rounded-lg border bg-subtle px-3 py-2.5">
      <Checkbox
        aria-label={resource === "agents" ? "Select all shown agents" : "Select all shown skills"}
        checked={allShownSelected}
        onChange={(event) => onToggleAllShown(event.target.checked)}
      />
      <span className="tabular text-[13px] font-medium">{selectedIds.length} selected</span>
      <Button size="sm" variant="outline" onClick={onChangeAccess} disabled={busy !== null}>
        Change access
      </Button>
      {resource === "agents" ? (
        <Button
          size="sm"
          variant="outline"
          onClick={() => void run("archive")}
          loading={busy === "archive"}
          disabled={busy !== null || (alreadyArchived > 0 && alreadyArchived === selectedIds.length)}
          title={alreadyArchived === selectedIds.length ? "Every selected agent is already archived" : undefined}
        >
          <Archive /> Archive
        </Button>
      ) : null}
      <Button
        size="sm"
        variant="outline"
        className="text-destructive hover:bg-destructive/10 hover:text-destructive"
        onClick={() => void run("delete")}
        loading={busy === "delete"}
        disabled={busy !== null}
      >
        <Trash2 /> Delete
      </Button>
      <Button size="sm" variant="ghost" onClick={onClear} disabled={busy !== null}>
        Clear selection
      </Button>
      {resource === "agents" && alreadyArchived ? (
        <p className="w-full text-[12px] text-muted-foreground">
          {noun(alreadyArchived, "agent")} {alreadyArchived === 1 ? "is" : "are"} already archived;
          archiving again changes nothing.
        </p>
      ) : null}
    </div>
  );
}
