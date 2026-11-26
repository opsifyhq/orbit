"use client";

import Link from "next/link";
import { useState } from "react";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { MoreHorizontal, Plus, Trash2, UserMinus, Users } from "lucide-react";
import { toast } from "sonner";

import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { Callout } from "@/components/ui/callout";
import { Card } from "@/components/ui/card";
import { useConfirm } from "@/components/ui/confirm";
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog";
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuTrigger,
} from "@/components/ui/dropdown-menu";
import { EmptyState } from "@/components/ui/empty-state";
import { Field } from "@/components/ui/field";
import { Input } from "@/components/ui/input";
import { PageHeader } from "@/components/ui/page-header";
import { Select } from "@/components/ui/select";
import { Skeleton } from "@/components/ui/skeleton";
import {
  addTeamMember,
  createAccessTeam,
  deleteAccessTeam,
  listAccessTeams,
  listTeamMembers as listMembers,
  removeTeamMember,
  type AccessTeam,
} from "@/lib/access-teams";
import { errorMessage } from "@orbit/api-client";
import { api } from "@/lib/api";
import { unwrap, useLicense } from "@/lib/queries";
import { relativeTime } from "@/lib/utils";

function slugify(value: string) {
  return value
    .toLowerCase()
    .trim()
    .replace(/[^a-z0-9]+/g, "-")
    .replace(/^-+|-+$/g, "");
}

const MEMBER_ROLES = [
  { value: "lead", label: "Lead", hint: "Manages the team's resources" },
  { value: "member", label: "Member", hint: "Can use the team's resources" },
  { value: "viewer", label: "Viewer", hint: "Can see the team's resources" },
];

const roleLabel = (role: string) => MEMBER_ROLES.find((item) => item.value === role)?.label ?? role;

export default function TeamsPage() {
  const license = useLicense();
  const licenseKnown = license.isSuccess;
  // Until the license is known, leave the action available; the API still enforces it.
  const licensed = !licenseKnown || Boolean(license.data?.license.features.includes("teams"));
  const queryClient = useQueryClient();
  const confirm = useConfirm();
  const teams = useQuery({ queryKey: ["access-teams"], queryFn: listAccessTeams });
  const users = useQuery({
    queryKey: ["users"],
    queryFn: async () =>
      (unwrap(await api.GET("/api/v1/users", { params: { query: { limit: 500 } } }))).items,
  });
  const [open, setOpen] = useState(false);
  const [name, setName] = useState("");
  const [slug, setSlug] = useState("");
  const [slugTouched, setSlugTouched] = useState(false);
  const [membersTeam, setMembersTeam] = useState<AccessTeam | null>(null);
  const [userId, setUserId] = useState("");
  const [role, setRole] = useState("member");
  const members = useQuery({
    queryKey: ["access-team-members", membersTeam?.id],
    queryFn: () => listMembers(membersTeam!.id),
    enabled: Boolean(membersTeam),
  });

  const create = useMutation({
    mutationFn: () => createAccessTeam({ name: name.trim(), slug: slug.trim() || slugify(name) }),
    onSuccess: () => {
      toast.success("Team created");
      setOpen(false);
      setName("");
      setSlug("");
      setSlugTouched(false);
      void queryClient.invalidateQueries({ queryKey: ["access-teams"] });
    },
    onError: (error) => toast.error(errorMessage(error)),
  });

  const remove = useMutation({
    mutationFn: deleteAccessTeam,
    onSuccess: () => {
      toast.success("Team deleted");
      void queryClient.invalidateQueries({ queryKey: ["access-teams"] });
    },
    onError: (error) => toast.error(errorMessage(error)),
  });

  const addMember = useMutation({
    mutationFn: () => addTeamMember(membersTeam!.id, { userId, role }),
    onSuccess: () => {
      toast.success("Member added");
      setUserId("");
      void queryClient.invalidateQueries({ queryKey: ["access-team-members", membersTeam?.id] });
    },
    onError: (error) => toast.error(errorMessage(error)),
  });

  const removeMember = useMutation({
    mutationFn: (memberId: string) => removeTeamMember(membersTeam!.id, memberId),
    onSuccess: () => {
      toast.success("Member removed");
      void queryClient.invalidateQueries({ queryKey: ["access-team-members", membersTeam?.id] });
    },
    onError: (error) => toast.error(errorMessage(error)),
  });

  const userName = (id: string) =>
    users.data?.find((user) => user.id === id)?.displayName ??
    users.data?.find((user) => user.id === id)?.email ??
    id.slice(0, 8);
  const userEmail = (id: string) => users.data?.find((user) => user.id === id)?.email;

  async function deleteTeam(team: AccessTeam) {
    const ok = await confirm({
      title: `Delete team ${team.name}?`,
      description:
        "Members lose access to anything shared only with this team. This cannot be undone.",
      confirmLabel: "Delete team",
      destructive: true,
    });
    if (ok) remove.mutate(team.id);
  }

  async function dropMember(userIdToRemove: string) {
    const ok = await confirm({
      title: `Remove ${userName(userIdToRemove)} from ${membersTeam?.name ?? "this team"}?`,
      description: "They lose access to resources shared only with this team. You can add them again later.",
      confirmLabel: "Remove member",
      destructive: true,
    });
    if (ok) removeMember.mutate(userIdToRemove);
  }

  const memberIds = new Set((members.data ?? []).map((member) => member.userId));
  const addableUsers = (users.data ?? []).filter((user) => !memberIds.has(user.id));

  return (
    <div className="space-y-6">
      <PageHeader
        title="Access teams"
        description="An access team decides who can see shared agents, skills, connections and workers. It does not run anything; for groups of agents that work together, see Agent teams."
        actions={
          <Button onClick={() => setOpen(true)} disabled={!licensed}>
            <Plus /> New team
          </Button>
        }
      />

      {licenseKnown && !licensed ? (
        <Callout tone="neutral" title="More teams need a license">
          Everyone is in the Default team. Creating further access teams is a licensed feature; the
          installed license is shown under Settings, Platform.
        </Callout>
      ) : null}

      {teams.isLoading ? (
        <Card className="overflow-hidden">
          {Array.from({ length: 3 }).map((_, index) => (
            <div key={index} className="border-b px-5 py-3.5 last:border-b-0">
              <Skeleton className="h-9" />
            </div>
          ))}
        </Card>
      ) : teams.isError ? (
        <Callout tone="danger" title="Could not load teams">
          {errorMessage(teams.error)}
        </Callout>
      ) : !teams.data?.length ? (
        <EmptyState
          icon={<Users />}
          title="No access teams yet"
          description="Create a team, add people to it, then share an agent or connection with that team."
          action={
            <Button variant="outline" onClick={() => setOpen(true)} disabled={!licensed}>
              <Plus /> New team
            </Button>
          }
        />
      ) : (
        <Card className="overflow-hidden">
          {teams.data.map((team) => (
            <div
              key={team.id}
              className="flex flex-col gap-3 border-b px-4 py-3.5 last:border-b-0 sm:flex-row sm:items-center sm:justify-between sm:px-5"
            >
              <div className="min-w-0">
                <div className="flex flex-wrap items-center gap-2">
                  <Link
                    href={`/teams/${team.id}`}
                    className="truncate text-sm font-medium hover:underline"
                  >
                    {team.name}
                  </Link>
                  {team.slug === "default" ? <Badge variant="secondary">Default</Badge> : null}
                </div>
                <p className="mt-0.5 flex flex-wrap items-baseline gap-x-3 text-[13px] text-muted-foreground">
                  <span className="break-all font-mono text-xs">{team.slug}</span>
                  <span>Updated {relativeTime(team.updatedAt)}</span>
                </p>
              </div>
              <div className="flex shrink-0 items-center gap-2">
                <Button size="sm" variant="outline" onClick={() => setMembersTeam(team)}>
                  <Users /> Manage members
                </Button>
                {team.slug === "default" ? null : (
                  <DropdownMenu>
                    <DropdownMenuTrigger asChild>
                      <Button
                        size="icon-sm"
                        variant="ghost"
                        aria-label={`More actions for ${team.name}`}
                      >
                        <MoreHorizontal />
                      </Button>
                    </DropdownMenuTrigger>
                    <DropdownMenuContent align="end">
                      <DropdownMenuItem destructive onSelect={() => void deleteTeam(team)}>
                        <Trash2 /> Delete team
                      </DropdownMenuItem>
                    </DropdownMenuContent>
                  </DropdownMenu>
                )}
              </div>
            </div>
          ))}
        </Card>
      )}

      <Dialog
        open={open}
        onOpenChange={(next) => {
          setOpen(next);
          if (!next) {
            setName("");
            setSlug("");
            setSlugTouched(false);
          }
        }}
      >
        <DialogContent>
          <DialogHeader>
            <DialogTitle>New access team</DialogTitle>
            <DialogDescription>
              People in a team can see the resources shared with it. Teams never run agents.
            </DialogDescription>
          </DialogHeader>
          <form
            className="space-y-4"
            onSubmit={(event) => {
              event.preventDefault();
              if (name.trim() && !create.isPending) create.mutate();
            }}
          >
            <Field label="Name" htmlFor="team-name">
              <Input
                id="team-name"
                value={name}
                placeholder="Platform engineering"
                autoFocus
                onChange={(event) => {
                  setName(event.target.value);
                  if (!slugTouched) setSlug(slugify(event.target.value));
                }}
              />
            </Field>
            <Field
              label="Slug"
              htmlFor="team-slug"
              hint="A short, URL-safe identifier. Filled in from the name."
            >
              <Input
                id="team-slug"
                className="font-mono"
                value={slug}
                onChange={(event) => {
                  setSlugTouched(true);
                  setSlug(event.target.value);
                }}
              />
            </Field>
            <DialogFooter>
              <Button type="button" variant="outline" onClick={() => setOpen(false)}>
                Cancel
              </Button>
              <Button type="submit" loading={create.isPending} disabled={!name.trim()}>
                Create team
              </Button>
            </DialogFooter>
          </form>
        </DialogContent>
      </Dialog>

      <Dialog open={Boolean(membersTeam)} onOpenChange={(next) => !next && setMembersTeam(null)}>
        <DialogContent className="max-w-xl">
          <DialogHeader>
            <DialogTitle>Members of {membersTeam?.name}</DialogTitle>
            <DialogDescription>
              Leads manage the team&apos;s resources, members can use them, viewers can only see them.
            </DialogDescription>
          </DialogHeader>

          <div className="space-y-4">
            {members.isLoading ? (
              <Skeleton className="h-16" />
            ) : members.isError ? (
              <Callout tone="danger">{errorMessage(members.error)}</Callout>
            ) : members.data?.length ? (
              <ul className="divide-y rounded-lg border">
                {members.data.map((member) => (
                  <li
                    key={member.userId}
                    className="flex items-center justify-between gap-3 px-3 py-2"
                  >
                    <div className="min-w-0">
                      <p className="truncate text-sm font-medium">{userName(member.userId)}</p>
                      {userEmail(member.userId) && userName(member.userId) !== userEmail(member.userId) ? (
                        <p className="truncate text-[13px] text-muted-foreground">
                          {userEmail(member.userId)}
                        </p>
                      ) : null}
                    </div>
                    <div className="flex shrink-0 items-center gap-2">
                      <Badge variant="outline">{roleLabel(member.role)}</Badge>
                      <Button
                        size="icon-sm"
                        variant="ghost"
                        className="text-muted-foreground hover:text-destructive"
                        aria-label={`Remove ${userName(member.userId)} from team`}
                        onClick={() => void dropMember(member.userId)}
                      >
                        <UserMinus />
                      </Button>
                    </div>
                  </li>
                ))}
              </ul>
            ) : (
              <p className="rounded-lg border border-dashed px-3 py-4 text-center text-sm text-muted-foreground">
                No members yet. Add someone below.
              </p>
            )}

            <div className="space-y-1.5">
              <p className="text-sm font-medium">Add a member</p>
              <div className="grid gap-2 sm:grid-cols-[minmax(0,1fr)_auto_auto]">
                <Select
                  aria-label="User to add"
                  value={userId}
                  onChange={(event) => setUserId(event.target.value)}
                >
                  <option value="">Choose a user</option>
                  {addableUsers.map((user) => (
                    <option key={user.id} value={user.id}>
                      {user.displayName} ({user.email})
                    </option>
                  ))}
                </Select>
                <Select
                  aria-label="Team role"
                  className="sm:w-auto"
                  value={role}
                  onChange={(event) => setRole(event.target.value)}
                >
                  {MEMBER_ROLES.map((item) => (
                    <option key={item.value} value={item.value}>
                      {item.label}
                    </option>
                  ))}
                </Select>
                <Button
                  onClick={() => addMember.mutate()}
                  disabled={!userId}
                  loading={addMember.isPending}
                >
                  Add member
                </Button>
              </div>
            </div>
          </div>
          <DialogFooter>
            <Button variant="outline" onClick={() => setMembersTeam(null)}>
              Close
            </Button>
          </DialogFooter>
        </DialogContent>
      </Dialog>
    </div>
  );
}
