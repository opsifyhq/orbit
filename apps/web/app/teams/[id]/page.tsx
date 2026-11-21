"use client";

export const dynamic = "force-dynamic";

import Link from "next/link";
import { useParams } from "next/navigation";
import { useEffect, useMemo, useState } from "react";
import { useInfiniteQuery, useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { Boxes, ExternalLink, Pencil, UserMinus, Users } from "lucide-react";
import { toast } from "sonner";

import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { Callout } from "@/components/ui/callout";
import { Card, CardContent, CardHeader, CardTitle } from "@/components/ui/card";
import { useConfirm } from "@/components/ui/confirm";
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog";
import { EmptyState } from "@/components/ui/empty-state";
import { Field } from "@/components/ui/field";
import { PageHeader } from "@/components/ui/page-header";
import { Section } from "@/components/ui/section";
import { Select } from "@/components/ui/select";
import { Skeleton } from "@/components/ui/skeleton";
import { Tabs, TabsContent, TabsList, TabsTrigger } from "@/components/ui/tabs";
import {
  addTeamMember,
  getResourceSharing,
  listAccessTeams,
  listTeamMembers,
  listTeamResources,
  removeTeamMember,
  updateResourceSharing,
  type AccessTeam,
  type TeamResourceItem,
} from "@/lib/access-teams";
import { api } from "@/lib/api";
import { errorMessage } from "@orbit/api-client";
import { unwrap, useLicense } from "@/lib/queries";
import { formatDateTime, relativeTime } from "@/lib/utils";

type TeamTab = "overview" | "resources" | "members" | "access";
const TEAM_TABS: TeamTab[] = ["overview", "resources", "members", "access"];

const RESOURCE_TYPES = [
  "agents",
  "skills",
  "connections",
  "providers",
  "worker_profiles",
  "agent_teams",
  "triggers",
  "tool_policies",
  "env_mappings",
] as const;

const RESOURCE_LABELS: Record<string, string> = {
  agents: "Agents",
  skills: "Skills",
  connections: "Connections",
  providers: "Providers",
  worker_profiles: "Worker profiles",
  agent_teams: "Agent Teams",
  triggers: "Triggers",
  tool_policies: "Tool policies",
  env_mappings: "Environment mappings",
};

const ROLE_RULES = [
  {
    role: "lead",
    label: "Lead",
    detail: "Views and uses team resources, manages access and changes team membership.",
  },
  {
    role: "member",
    label: "Member",
    detail: "Views and uses team resources, but cannot change access or membership.",
  },
  {
    role: "viewer",
    label: "Viewer",
    detail: "Sees team resources for review; cannot run or manage them.",
  },
];

const VISIBILITY_RULES = [
  { value: "private", label: "Private", detail: "Owner and org owner/admin only." },
  { value: "team", label: "Team", detail: "Members of the owning team, plus org owner/admin." },
  {
    value: "org",
    label: "Organization",
    detail: "Everyone in the organization unless strict team isolation narrows it.",
  },
];

const roleLabel = (role: string) =>
  ROLE_RULES.find((item) => item.role === role)?.label ?? role;

const visibilityVariant = (value: string) =>
  value === "org" ? "warning" : value === "team" ? "info" : "outline";

function resourceHref(item: TeamResourceItem): string {
  switch (item.resourceType) {
    case "agents":
      return `/agents/${item.id}`;
    case "skills":
      return `/skills/${item.id}`;
    case "connections":
      return `/connections/${item.id}`;
    case "agent_teams":
      return `/agent-teams/${item.id}`;
    case "worker_profiles":
      return `/workers?edit=${item.id}`;
    case "providers":
      return "/models";
    case "env_mappings":
      return "/environment-mappings";
    case "triggers":
    case "tool_policies":
      return "/agents";
    default:
      return "/teams";
  }
}

function AccessEditorDialog({
  item,
  teams,
  open,
  onOpenChange,
  onSaved,
}: {
  item: TeamResourceItem;
  teams: AccessTeam[];
  open: boolean;
  onOpenChange: (open: boolean) => void;
  onSaved: () => void;
}) {
  const queryClient = useQueryClient();
  const [visibility, setVisibility] = useState(item.visibility);
  const [teamId, setTeamId] = useState(item.ownerTeamId ?? "");
  const sharing = useQuery({
    queryKey: ["sharing", item.resourceType, item.id],
    queryFn: () => getResourceSharing(item.resourceType, item.id),
    enabled: open,
  });

  useEffect(() => {
    if (sharing.data) {
      setVisibility(sharing.data.visibility);
      setTeamId(sharing.data.ownerTeamId ?? "");
    }
  }, [sharing.data]);

  const save = useMutation({
    mutationFn: () =>
      updateResourceSharing(item.resourceType, item.id, {
        visibility,
        ownerTeamId: teamId || null,
      }),
    onSuccess: () => {
      toast.success("Access updated");
      void queryClient.invalidateQueries({ queryKey: ["sharing"] });
      onSaved();
      onOpenChange(false);
    },
    onError: (error) => toast.error(errorMessage(error)),
  });

  return (
    <Dialog
      open={open}
      onOpenChange={(next) => {
        if (!next) setVisibility(item.visibility);
        onOpenChange(next);
      }}
    >
      <DialogContent>
        <DialogHeader>
          <DialogTitle>Edit access · {item.name}</DialogTitle>
          <DialogDescription>
            The same access editor used on the resource page. Changes are checked by the platform
            and written to the audit log.
          </DialogDescription>
        </DialogHeader>
        {sharing.isLoading ? (
          <Skeleton className="h-32" />
        ) : sharing.isError ? (
          <Callout tone="danger">{errorMessage(sharing.error)}</Callout>
        ) : (
          <div className="space-y-4">
            <Field label="Visibility" htmlFor="overview-visibility">
              <Select
                id="overview-visibility"
                value={visibility}
                onChange={(event) => setVisibility(event.target.value)}
              >
                {VISIBILITY_RULES.map((option) => (
                  <option key={option.value} value={option.value}>
                    {option.label}
                  </option>
                ))}
              </Select>
            </Field>
            <Field
              label="Owning team"
              htmlFor="overview-owner-team"
              hint="Leads of the owning team can manage it."
            >
              <Select
                id="overview-owner-team"
                value={teamId}
                onChange={(event) => setTeamId(event.target.value)}
              >
                <option value="">No owning team</option>
                {teams.map((team) => (
                  <option key={team.id} value={team.id}>
                    {team.name}
                  </option>
                ))}
              </Select>
            </Field>
          </div>
        )}
        <DialogFooter>
          <Button variant="outline" onClick={() => onOpenChange(false)}>
            Cancel
          </Button>
          <Button
            loading={save.isPending}
            disabled={Boolean(sharing.isLoading || sharing.isError)}
            onClick={() => save.mutate()}
          >
            Save access
          </Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  );
}

export default function TeamDetailPage() {
  const params = useParams<{ id: string }>();
  const teamId = params.id;
  const queryClient = useQueryClient();
  const confirm = useConfirm();
  const license = useLicense();
  const licensed =
    !license.isSuccess || Boolean(license.data?.license.features.includes("teams"));
  const [tab, setTab] = useState<TeamTab>("overview");
  const [type, setType] = useState("");
  const [editing, setEditing] = useState<TeamResourceItem | null>(null);
  const [userId, setUserId] = useState("");
  const [role, setRole] = useState("viewer");

  useEffect(() => {
    const fromHash = window.location.hash.slice(1) as TeamTab;
    if (TEAM_TABS.includes(fromHash)) setTab(fromHash);
  }, []);

  const teams = useQuery({ queryKey: ["access-teams"], queryFn: listAccessTeams });
  const team = teams.data?.find((candidate) => candidate.id === teamId);
  const users = useQuery({
    queryKey: ["users"],
    queryFn: async () => unwrap(await api.GET("/api/v1/users")),
  });
  const members = useQuery({
    queryKey: ["access-team-members", teamId],
    queryFn: () => listTeamMembers(teamId),
    enabled: Boolean(teamId),
  });
  const resources = useInfiniteQuery({
    queryKey: ["team-resources", teamId, type],
    queryFn: ({ pageParam }) =>
      listTeamResources(teamId, {
        type: type || undefined,
        cursor: pageParam,
        limit: 25,
      }),
    initialPageParam: undefined as string | undefined,
    getNextPageParam: (lastPage) => lastPage.nextCursor ?? undefined,
    enabled: Boolean(teamId),
  });

  const addMember = useMutation({
    mutationFn: () => addTeamMember(teamId, { userId, role }),
    onSuccess: () => {
      toast.success("Member added");
      setUserId("");
      void queryClient.invalidateQueries({ queryKey: ["access-team-members", teamId] });
      void queryClient.invalidateQueries({ queryKey: ["team-resources", teamId] });
    },
    onError: (error) => toast.error(errorMessage(error)),
  });
  const removeMember = useMutation({
    mutationFn: (memberId: string) => removeTeamMember(teamId, memberId),
    onSuccess: () => {
      toast.success("Member removed");
      void queryClient.invalidateQueries({ queryKey: ["access-team-members", teamId] });
      void queryClient.invalidateQueries({ queryKey: ["team-resources", teamId] });
    },
    onError: (error) => toast.error(errorMessage(error)),
  });

  const summary = resources.data?.pages[0]?.summary;
  const items = useMemo(
    () => resources.data?.pages.flatMap((page) => page.items) ?? [],
    [resources.data],
  );
  const grouped = useMemo(() => {
    const map = new Map<string, TeamResourceItem[]>();
    for (const item of items) {
      const group = map.get(item.resourceType) ?? [];
      group.push(item);
      map.set(item.resourceType, group);
    }
    return [...map.entries()].sort(
      ([a], [b]) =>
        RESOURCE_TYPES.indexOf(a as (typeof RESOURCE_TYPES)[number]) -
        RESOURCE_TYPES.indexOf(b as (typeof RESOURCE_TYPES)[number]),
    );
  }, [items]);

  const userName = (id: string) =>
    users.data?.find((user) => user.id === id)?.displayName ?? id.slice(0, 8);
  const userEmail = (id: string) => users.data?.find((user) => user.id === id)?.email;
  const memberIds = new Set((members.data ?? []).map((member) => member.userId));
  const addableUsers = (users.data ?? []).filter((user) => !memberIds.has(user.id));

  async function dropMember(id: string) {
    const ok = await confirm({
      title: `Remove ${userName(id)} from this team?`,
      description: "They lose access to resources shared only with this team.",
      confirmLabel: "Remove member",
      destructive: true,
    });
    if (ok) removeMember.mutate(id);
  }

  function selectTab(next: string) {
    const value = TEAM_TABS.includes(next as TeamTab) ? (next as TeamTab) : "overview";
    setTab(value);
    window.location.hash = value;
  }

  if (resources.isError) {
    return (
      <div className="space-y-6">
        <PageHeader title="Access team" back={{ href: "/teams", label: "Access teams" }} />
        <Callout tone="danger" title="This team is not available">
          {errorMessage(resources.error)} You may have lost access, or the team may have been
          deleted.
        </Callout>
      </div>
    );
  }

  return (
    <div className="space-y-6">
      <PageHeader
        title={team?.name ?? "Access team"}
        description="Who belongs to this team, what it owns, and what its members can do. Access teams do not run agents."
        back={{ href: "/teams", label: "Access teams" }}
        meta={
          team ? (
            <>
              <span className="break-all font-mono text-xs text-muted-foreground">{team.slug}</span>
              <span className="text-[13px] text-muted-foreground">
                Updated {relativeTime(team.updatedAt)}
              </span>
            </>
          ) : null
        }
        actions={
          <Button variant="outline" onClick={() => selectTab("members")}>
            <Users /> Members
          </Button>
        }
      />

      {license.isSuccess && !licensed ? (
        <Callout tone="neutral" title="Enterprise features are not licensed">
          Existing access still works, but changing team membership or resource access needs the
          teams and resource permissions features.
        </Callout>
      ) : null}

      <Tabs value={tab} onValueChange={selectTab}>
        <TabsList aria-label="Team sections">
          <TabsTrigger value="overview">Overview</TabsTrigger>
          <TabsTrigger value="resources">Resources</TabsTrigger>
          <TabsTrigger value="members">Members</TabsTrigger>
          <TabsTrigger value="access">How access works</TabsTrigger>
        </TabsList>

        <TabsContent value="overview" className="space-y-5">
          {!summary ? (
            <Card className="p-5">
              <Skeleton className="h-40" />
            </Card>
          ) : (
            <>
              <div className="grid grid-cols-2 gap-3 sm:grid-cols-4">
                {[
                  { label: "Leads", value: summary.roles.lead },
                  { label: "Members", value: summary.roles.member },
                  { label: "Viewers", value: summary.roles.viewer },
                  { label: "Resources", value: summary.resources.total },
                ].map((card) => (
                  <Card key={card.label} className="p-4">
                    <p className="text-[13px] text-muted-foreground">{card.label}</p>
                    <p className="mt-1 text-2xl font-semibold tabular">{card.value}</p>
                  </Card>
                ))}
              </div>

              {summary.orgVisibleCount > 0 ? (
                <Callout tone="warning" title="Wider than this team">
                  {summary.orgVisibleCount} resource
                  {summary.orgVisibleCount === 1 ? " is" : "s are"} owned by this team but marked
                  organization-visible. Check the Resources tab unless that sharing is intentional.
                </Callout>
              ) : null}

              <Section title="Resources by type" description="Visible resources relevant to this team.">
                <Card className="overflow-hidden">
                  <dl className="grid grid-cols-2 sm:grid-cols-3">
                    {RESOURCE_TYPES.map((resource) => (
                      <div
                        key={resource}
                        className="flex items-center justify-between gap-2 border-b border-r px-4 py-3 last:border-b-0"
                      >
                        <dt className="text-[13px] text-muted-foreground">
                          {RESOURCE_LABELS[resource]}
                        </dt>
                        <dd className="text-sm font-medium tabular">
                          {summary.resources[
                            resource === "worker_profiles"
                              ? "workerProfiles"
                              : resource === "agent_teams"
                                ? "agentTeams"
                                : resource === "tool_policies"
                                  ? "toolPolicies"
                                  : resource === "env_mappings"
                                    ? "envMappings"
                                    : resource
                          ]}
                        </dd>
                      </div>
                    ))}
                  </dl>
                </Card>
              </Section>
            </>
          )}
        </TabsContent>

        <TabsContent value="resources" className="space-y-5">
          <div className="flex flex-col gap-3 sm:flex-row sm:items-center sm:justify-between">
            <div className="w-full sm:max-w-xs">
              <Field label="Resource type" htmlFor="team-resource-type">
                <Select
                  id="team-resource-type"
                  value={type}
                  onChange={(event) => setType(event.target.value)}
                >
                  <option value="">All types</option>
                  {RESOURCE_TYPES.map((resource) => (
                    <option key={resource} value={resource}>
                      {RESOURCE_LABELS[resource]}
                    </option>
                  ))}
                </Select>
              </Field>
            </div>
            {resources.data?.pages[0] ? (
              <p className="text-[13px] text-muted-foreground">
                {resources.data.pages[0].total} visible resource
                {resources.data.pages[0].total === 1 ? "" : "s"} for this selection
              </p>
            ) : null}
          </div>

          {summary && summary.orgVisibleCount > 0 ? (
            <Callout tone="warning" title="Organization-visible resources">
              {summary.orgVisibleCount} of these resources are wider than the team. Their visibility
              badge says <Badge variant="warning">Organization</Badge>.
            </Callout>
          ) : null}

          {!summary && resources.isLoading ? (
            <Card className="p-5">
              <Skeleton className="h-48" />
            </Card>
          ) : grouped.length === 0 ? (
            <EmptyState
              icon={<Boxes />}
              title="No resources yet"
              description="Nothing is owned by or shared with this team for the selected type."
            />
          ) : (
            <div className="space-y-6">
              {grouped.map(([resource, group]) => (
                <Section
                  key={resource}
                  title={RESOURCE_LABELS[resource] ?? resource}
                  count={group.length}
                >
                  <Card className="overflow-hidden">
                    <ul className="divide-y">
                      {group.map((item) => (
                        <li
                          key={`${item.resourceType}-${item.id}`}
                          className="grid gap-3 px-4 py-3.5 sm:grid-cols-[minmax(0,1fr)_minmax(0,1.15fr)_auto] sm:items-start"
                        >
                          <div className="min-w-0">
                            <Link
                              href={resourceHref(item)}
                              className="inline-flex max-w-full items-start gap-1 text-sm font-medium hover:underline"
                            >
                              <span className="break-words">{item.name}</span>
                              <ExternalLink className="mt-0.5 h-3.5 w-3.5 shrink-0 text-muted-foreground" />
                            </Link>
                            <div className="mt-1 flex flex-wrap gap-1.5">
                              {item.isOwnerTeam ? (
                                <Badge variant="secondary">Owned by this team</Badge>
                              ) : (
                                <Badge variant="outline">Shared here</Badge>
                              )}
                            </div>
                          </div>
                          <dl className="grid min-w-0 gap-x-4 gap-y-1 text-[13px] sm:grid-cols-2">
                            <div>
                              <dt className="text-muted-foreground">Owner</dt>
                              <dd className="break-words">{item.ownerName ?? "Unassigned"}</dd>
                            </div>
                            <div>
                              <dt className="text-muted-foreground">Owning team</dt>
                              <dd className="break-words">
                                {item.ownerTeamId ? (team?.name ?? "This team") : "None"}
                              </dd>
                            </div>
                            <div>
                              <dt className="text-muted-foreground">Visibility</dt>
                              <dd>
                                <Badge variant={visibilityVariant(item.visibility)}>
                                  {item.visibility === "org" ? "Organization" : item.visibility}
                                </Badge>
                              </dd>
                            </div>
                            <div>
                              <dt className="text-muted-foreground">Updated</dt>
                              <dd title={formatDateTime(item.updatedAt)}>
                                {relativeTime(item.updatedAt)}
                              </dd>
                            </div>
                          </dl>
                          <div className="flex flex-wrap items-start gap-2 sm:justify-end">
                            {item.effectiveAccess.map((level) => (
                              <Badge key={level} variant="outline">
                                {level}
                              </Badge>
                            ))}
                            {item.effectiveAccess.includes("manage") ? (
                              <Button
                                size="sm"
                                variant="outline"
                                onClick={() => setEditing(item)}
                              >
                                <Pencil /> Access
                              </Button>
                            ) : null}
                          </div>
                        </li>
                      ))}
                    </ul>
                  </Card>
                </Section>
              ))}
              {resources.hasNextPage ? (
                <Button
                  variant="outline"
                  loading={resources.isFetchingNextPage}
                  onClick={() => void resources.fetchNextPage()}
                >
                  Load more
                </Button>
              ) : null}
              <p className="text-[13px] text-muted-foreground">
                Outputs are delivery records, not shareable resources: they have no owning team or
                visibility, so they are not listed here.
              </p>
            </div>
          )}
        </TabsContent>

        <TabsContent value="members" className="space-y-5">
          {summary && !summary.canManageTeam ? (
            <Callout tone="neutral" title="Read-only membership">
              Only team leads and organization owners/admins can add or remove members.
            </Callout>
          ) : null}
          <Section title="Current members" count={members.data?.length}>
            {members.isLoading ? (
              <Card className="p-5">
                <Skeleton className="h-24" />
              </Card>
            ) : members.isError ? (
              <Callout tone="danger">{errorMessage(members.error)}</Callout>
            ) : members.data?.length ? (
              <Card className="overflow-hidden">
                <ul className="divide-y">
                  {members.data.map((member) => (
                    <li
                      key={member.userId}
                      className="flex flex-col gap-2 px-4 py-3 sm:flex-row sm:items-center sm:justify-between"
                    >
                      <div className="min-w-0">
                        <p className="truncate text-sm font-medium">{userName(member.userId)}</p>
                        {userEmail(member.userId) ? (
                          <p className="truncate text-[13px] text-muted-foreground">
                            {userEmail(member.userId)}
                          </p>
                        ) : null}
                      </div>
                      <div className="flex items-center gap-2">
                        <Badge variant="outline">{roleLabel(member.role)}</Badge>
                        {summary?.canManageTeam ? (
                          <Button
                            size="icon-sm"
                            variant="ghost"
                            className="text-muted-foreground hover:text-destructive"
                            aria-label={`Remove ${userName(member.userId)}`}
                            onClick={() => void dropMember(member.userId)}
                          >
                            <UserMinus />
                          </Button>
                        ) : null}
                      </div>
                    </li>
                  ))}
                </ul>
              </Card>
            ) : (
              <EmptyState icon={<Users />} title="No members" description="Add someone below." />
            )}
          </Section>

          {summary?.canManageTeam ? (
            <Section title="Add a member">
              <Card className="p-4">
                <div className="grid gap-3 sm:grid-cols-[minmax(0,1fr)_auto_auto] sm:items-end">
                  <Field label="User" htmlFor="new-member-user">
                    <Select
                      id="new-member-user"
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
                  </Field>
                  <Field label="Role" htmlFor="new-member-role">
                    <Select
                      id="new-member-role"
                      value={role}
                      onChange={(event) => setRole(event.target.value)}
                    >
                      {ROLE_RULES.map((option) => (
                        <option key={option.role} value={option.role}>
                          {option.label}
                        </option>
                      ))}
                    </Select>
                  </Field>
                  <Button
                    loading={addMember.isPending}
                    disabled={!userId}
                    onClick={() => addMember.mutate()}
                  >
                    Add member
                  </Button>
                </div>
              </Card>
            </Section>
          ) : null}

          <Section title="What each role means">
            <Card className="overflow-hidden">
              <ul className="divide-y">
                {ROLE_RULES.map((option) => (
                  <li key={option.role} className="px-4 py-3">
                    <p className="text-sm font-medium">{option.label}</p>
                    <p className="mt-0.5 text-[13px] text-muted-foreground">{option.detail}</p>
                  </li>
                ))}
              </ul>
            </Card>
          </Section>
        </TabsContent>

        <TabsContent value="access" className="space-y-5">
          <Section title="Visibility" description="Visibility is stored on the resource, not in the prompt.">
            <Card className="overflow-hidden">
              <ul className="divide-y">
                {VISIBILITY_RULES.map((option) => (
                  <li key={option.value} className="px-4 py-3">
                    <Badge variant={visibilityVariant(option.value)}>{option.label}</Badge>
                    <p className="mt-1.5 text-[13px] text-muted-foreground">{option.detail}</p>
                  </li>
                ))}
              </ul>
            </Card>
          </Section>
          <Section title="Effective access" description="Visibility and team role combine into what you can actually do.">
            <Card className="overflow-hidden">
              <ul className="divide-y">
                {ROLE_RULES.map((option) => (
                  <li key={option.role} className="px-4 py-3">
                    <p className="text-sm font-medium">{option.label}</p>
                    <p className="mt-0.5 text-[13px] text-muted-foreground">{option.detail}</p>
                  </li>
                ))}
              </ul>
            </Card>
          </Section>
          <Callout tone="info" title="Owners and admins bypass these checks">
            Organization owners/admins can view and manage every resource. Strict team isolation
            (Settings → Access policy) stops other teams from seeing a team-owned resource even when
            its visibility says Organization.
          </Callout>
        </TabsContent>
      </Tabs>

      {editing ? (
        <AccessEditorDialog
          item={editing}
          teams={teams.data ?? []}
          open
          onOpenChange={(open) => !open && setEditing(null)}
          onSaved={() => void queryClient.invalidateQueries({ queryKey: ["team-resources", teamId] })}
        />
      ) : null}
    </div>
  );
}
