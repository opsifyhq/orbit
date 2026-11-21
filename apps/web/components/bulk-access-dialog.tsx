"use client";

import { useState } from "react";
import { useMutation, useQuery } from "@tanstack/react-query";
import { toast } from "sonner";

import { Button } from "@/components/ui/button";
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog";
import { Field } from "@/components/ui/field";
import { Select } from "@/components/ui/select";
import { listAccessTeams } from "@/lib/access-teams";
import { request } from "@/lib/http";
import { useLicense } from "@/lib/queries";
import { errorMessage } from "@orbit/api-client";

type BulkResult = {
  updated: string[];
  failed: Array<{ id: string; message: string }>;
};

export function BulkAccessDialog({
  open,
  onOpenChange,
  resource,
  ids,
  onDone,
}: {
  open: boolean;
  onOpenChange: (open: boolean) => void;
  resource: "agents" | "agent_teams" | "skills" | "connections" | "env_mappings";
  ids: string[];
  onDone: () => void;
}) {
  const license = useLicense();
  const features = license.data?.license.features ?? [];
  const enabled = features.includes("teams") && features.includes("resource_permissions");
  const teams = useQuery({
    queryKey: ["access-teams"],
    queryFn: listAccessTeams,
    enabled,
  });
  const [visibility, setVisibility] = useState("org");
  const [owner, setOwner] = useState("");

  const save = useMutation({
    mutationFn: () =>
      request<BulkResult>(`/api/v1/sharing/${resource}/bulk`, {
        method: "PATCH",
        body: JSON.stringify({
          ids,
          visibility,
          ...(owner && owner !== "__none" ? { ownerTeamId: owner } : {}),
          ...(owner === "__none" ? { clearOwnerTeam: true } : {}),
        }),
      }),
    onSuccess: (result) => {
      if (result.failed.length) {
        toast.warning(
          `${result.updated.length} updated, ${result.failed.length} failed`,
          { description: result.failed[0]?.message },
        );
      } else {
        toast.success(`${result.updated.length} ${result.updated.length === 1 ? "resource" : "resources"} updated`);
      }
      onOpenChange(false);
      onDone();
    },
    onError: (error) => toast.error(errorMessage(error)),
  });

  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      <DialogContent>
        <DialogHeader>
          <DialogTitle>Change access for {ids.length} selected</DialogTitle>
          <DialogDescription>
            Visibility applies to every selected resource. Resources you cannot manage are
            reported and left unchanged.
          </DialogDescription>
        </DialogHeader>
        {!enabled && license.isSuccess ? (
          <p className="rounded-lg border bg-subtle px-3 py-2.5 text-[13px] text-muted-foreground">
            Teams and private access need an enterprise license with teams and resource
            permissions. You can still set Organization visibility.
          </p>
        ) : null}
        <Field label="Visibility" htmlFor="bulk-visibility">
          <Select
            id="bulk-visibility"
            value={visibility}
            onChange={(event) => setVisibility(event.target.value)}
          >
            <option value="org">Organization</option>
            <option value="team" disabled={!enabled}>Team</option>
            <option value="private" disabled={!enabled}>Private</option>
          </Select>
        </Field>
        <Field label="Owning team" htmlFor="bulk-owner" optional hint="Leave unchanged unless you need to move the resources.">
          <Select id="bulk-owner" value={owner} onChange={(event) => setOwner(event.target.value)}>
            <option value="">Leave unchanged</option>
            <option value="__none">No owning team</option>
            {(teams.data ?? []).map((team) => (
              <option key={team.id} value={team.id}>
                {team.name}
              </option>
            ))}
          </Select>
        </Field>
        <DialogFooter>
          <Button variant="outline" onClick={() => onOpenChange(false)}>
            Cancel
          </Button>
          <Button
            onClick={() => save.mutate()}
            loading={save.isPending}
            disabled={
              !ids.length ||
              !enabled ||
              (visibility === "team" && (!owner || owner === "__none"))
            }
          >
            Update access
          </Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  );
}
