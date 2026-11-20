"use client";

import { LoadFailure } from "@/components/load-failure";
import { useMemo, useState } from "react";
import Link from "next/link";
import { useParams, useRouter } from "next/navigation";
import { useMutation, useQueryClient } from "@tanstack/react-query";
import { Bot, ChevronRight, Download, Ellipsis, FileText, Pencil, Server, Trash2, Workflow } from "lucide-react";
import { toast } from "sonner";

import { exportResource } from "@/components/export-resource-button";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { Callout } from "@/components/ui/callout";
import { Card, CardContent, CardHeader, CardTitle } from "@/components/ui/card";
import { useConfirm } from "@/components/ui/confirm";
import { DescriptionList } from "@/components/ui/description-list";
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuSeparator,
  DropdownMenuTrigger,
} from "@/components/ui/dropdown-menu";
import { Markdown } from "@/components/ui/markdown";
import { PageHeader } from "@/components/ui/page-header";
import { Segmented } from "@/components/ui/segmented";
import { Skeleton } from "@/components/ui/skeleton";
import { cleanDescription } from "@/lib/presentation";
import { queryKeys, useSkill } from "@/lib/queries";
import { parseSkill } from "@/lib/skill-md";
import { deleteSkill } from "@/lib/skills";
import { useSkillUsage, type SkillUser } from "@/lib/skill-usage";
import { cn, formatDateTime, relativeTime, titleCase } from "@/lib/utils";
import { errorMessage } from "@orbit/api-client";

const USER_ICON: Record<SkillUser["kind"], React.ReactNode> = {
  agent: <Bot className="h-4 w-4" />,
  team: <Workflow className="h-4 w-4" />,
  profile: <Server className="h-4 w-4" />,
};
const USER_LABEL: Record<SkillUser["kind"], string> = {
  agent: "Agent",
  team: "Agent team",
  profile: "Worker profile",
};

function bundled(files: unknown): Array<{ path: string; content: string }> {
  if (!files || typeof files !== "object" || Array.isArray(files)) return [];
  return Object.entries(files as Record<string, unknown>)
    .map(([path, content]) => ({ path, content: typeof content === "string" ? content : JSON.stringify(content, null, 2) }))
    .sort((a, b) => a.path.localeCompare(b.path));
}

export default function SkillPage() {
  const params = useParams<{ id: string }>();
  const skillId = params.id;
  const router = useRouter();
  const queryClient = useQueryClient();
  const confirm = useConfirm();
  const skill = useSkill(skillId);
  const { usersOf, isLoading: usageLoading } = useSkillUsage();
  const [view, setView] = useState<"rendered" | "source">("rendered");
  const [openFile, setOpenFile] = useState<string | null>(null);

  const remove = useMutation({
    mutationFn: () => deleteSkill(skillId),
    onSuccess: async () => {
      toast.success("Skill deleted");
      await queryClient.invalidateQueries({ queryKey: queryKeys.skills });
      router.push("/skills");
    },
    onError: (error) => toast.error(errorMessage(error)),
  });

  const doc = useMemo(() => (skill.data ? parseSkill(skill.data.content) : null), [skill.data]);

  if (skill.isLoading) {
    return (
      <div className="space-y-4">
        <Skeleton className="h-10 w-64" />
        <Skeleton className="h-96 rounded-xl" />
      </div>
    );
  }
  if (!skill.data || !doc) {
    return (
      <LoadFailure
        noun="skill"
        error={skill.error}
        back={{ href: "/skills", label: "Back to skills" }}
        onRetry={() => void skill.refetch()}
        retrying={skill.isFetching}
      />
    );
  }

  const data = skill.data;
  const users = usersOf(data.slug);
  const files = bundled(data.files);
  const description = cleanDescription(data.description);
  const nameDrift = doc.name !== data.slug;
  const descriptionDrift = (doc.description || "") !== (data.description || "").replace(/\s+/g, " ").trim();

  async function confirmDelete() {
    const names = users.map((user) => user.name);
    const shown = names.slice(0, 4).join(", ") + (names.length > 4 ? ` and ${names.length - 4} more` : "");
    const confirmed = await confirm({
      title: `Delete ${data.name}?`,
      description: users.length
        ? users.length === 1
          ? `${shown} uses this skill. It runs without it from now on, and its settings show the missing skill so you can remove it before saving them again. This cannot be undone.`
          : `${shown} use this skill. They run without it from now on, and their settings show the missing skill so you can remove it before saving them again. This cannot be undone.`
        : "Nothing uses this skill. This cannot be undone.",
      confirmLabel: "Delete skill",
      destructive: true,
    });
    if (confirmed) remove.mutate();
  }

  return (
    <div className="space-y-6">
      <PageHeader
        back={{ href: "/skills", label: "Skills" }}
        title={data.name}
        description={description || undefined}
        meta={
          <>
            <code className="rounded bg-muted px-1.5 py-0.5 font-mono text-xs">{data.slug}</code>
            <Badge variant="outline">Version {data.version}</Badge>
            <span className="text-[13px] text-muted-foreground">Updated {relativeTime(data.updatedAt)}</span>
          </>
        }
        actions={
          <>
            <Button asChild variant="outline">
              <Link href={`/skills/${data.id}/edit`}>
                <Pencil /> Edit
              </Link>
            </Button>
            <DropdownMenu>
              <DropdownMenuTrigger asChild>
                <Button variant="outline" size="icon" aria-label="More actions">
                  <Ellipsis />
                </Button>
              </DropdownMenuTrigger>
              <DropdownMenuContent align="end">
                <DropdownMenuItem onSelect={() => void exportResource("skills", data.id, data.name, "markdown")}>
                  <Download /> Export SKILL.md
                </DropdownMenuItem>
                <DropdownMenuItem onSelect={() => void exportResource("skills", data.id, data.name)}>
                  <Download /> Export with files (YAML)
                </DropdownMenuItem>
                <DropdownMenuSeparator />
                <DropdownMenuItem destructive onSelect={() => void confirmDelete()}>
                  <Trash2 /> Delete skill
                </DropdownMenuItem>
              </DropdownMenuContent>
            </DropdownMenu>
          </>
        }
      />

      {!description ? (
        <Callout tone="warning" title="No description" action={<Button asChild size="sm" variant="outline"><Link href={`/skills/${data.id}/edit`}>Add one</Link></Button>}>
          Agents read the description to decide when to load a skill, so without one this skill is rarely used.
        </Callout>
      ) : null}
      {nameDrift || descriptionDrift ? (
        <Callout
          tone="warning"
          title="SKILL.md does not match this page"
          action={<Button asChild size="sm" variant="outline"><Link href={`/skills/${data.id}/edit`}>Edit and save</Link></Button>}
        >
          {nameDrift ? (
            <>
              Agents see the name <code className="font-mono text-xs">{doc.name || "(none)"}</code> instead of{" "}
              <code className="font-mono text-xs">{data.slug}</code>.{" "}
            </>
          ) : null}
          {descriptionDrift ? "The description in SKILL.md differs from the one shown here. " : null}
          Saving it once in the editor writes the name and description into SKILL.md.
        </Callout>
      ) : null}

      <div className="grid gap-6 lg:grid-cols-[minmax(0,1fr)_20rem] lg:items-start [&>*]:min-w-0">
        <div className="space-y-6">
          <Card>
            <CardHeader className="flex-row flex-wrap items-center justify-between gap-2 space-y-0">
              <CardTitle>Instructions</CardTitle>
              <Segmented
                label="Instructions view"
                size="sm"
                value={view}
                onChange={setView}
                options={[
                  { value: "rendered", label: "Formatted" },
                  { value: "source", label: "SKILL.md" },
                ]}
              />
            </CardHeader>
            <CardContent>
              {view === "rendered" ? (
                doc.body.trim() ? (
                  <Markdown>{doc.body}</Markdown>
                ) : (
                  <p className="text-sm text-muted-foreground">No instructions written.</p>
                )
              ) : (
                <pre className="code-block max-h-[60vh] whitespace-pre-wrap break-words">{data.content}</pre>
              )}
            </CardContent>
          </Card>

          <Card>
            <CardHeader>
              <CardTitle>Bundled files</CardTitle>
              <p className="text-[13px] text-muted-foreground">Copied next to SKILL.md; scripts are made executable.</p>
            </CardHeader>
            <CardContent>
              {files.length ? (
                <ul className="divide-y rounded-lg border">
                  {files.map((file) => {
                    const open = openFile === file.path;
                    return (
                      <li key={file.path}>
                        <button
                          type="button"
                          aria-expanded={open}
                          onClick={() => setOpenFile(open ? null : file.path)}
                          className="flex w-full items-center gap-2 px-3 py-2 text-left hover:bg-muted/50"
                        >
                          <ChevronRight className={cn("h-3.5 w-3.5 shrink-0 text-muted-foreground transition-transform", open && "rotate-90")} />
                          <FileText className="h-3.5 w-3.5 shrink-0 text-muted-foreground" />
                          <span className="min-w-0 flex-1 truncate font-mono text-xs">{file.path}</span>
                          <span className="tabular shrink-0 text-[11px] text-muted-foreground">
                            {file.content.split("\n").length} lines
                          </span>
                        </button>
                        {open ? (
                          <pre className="code-block mx-3 mb-3 max-h-80 whitespace-pre-wrap break-words">{file.content}</pre>
                        ) : null}
                      </li>
                    );
                  })}
                </ul>
              ) : (
                <p className="text-[13px] text-muted-foreground">None. The skill is just its SKILL.md.</p>
              )}
            </CardContent>
          </Card>
        </div>

        <div className="space-y-6">
          <Card>
            <CardHeader>
              <CardTitle>Used by</CardTitle>
            </CardHeader>
            <CardContent>
              {usageLoading ? (
                <Skeleton className="h-16" />
              ) : users.length ? (
                <ul className="-mx-2 space-y-0.5">
                  {users.map((user) => (
                    <li key={`${user.kind}-${user.id}`}>
                      <Link
                        href={user.href}
                        className="flex items-center gap-2.5 rounded-md px-2 py-1.5 text-sm hover:bg-muted/60"
                      >
                        <span className="text-muted-foreground">{USER_ICON[user.kind]}</span>
                        <span className="min-w-0 flex-1 truncate font-medium">{user.name}</span>
                        <span className="shrink-0 text-xs text-muted-foreground">{USER_LABEL[user.kind]}</span>
                      </Link>
                    </li>
                  ))}
                </ul>
              ) : (
                <p className="text-[13px] text-muted-foreground">
                  Nothing uses it yet. Attach it in an agent&apos;s{" "}
                  <span className="font-medium text-foreground">Connectors and skills</span>, a team&apos;s Skills, or a
                  worker profile.
                </p>
              )}
            </CardContent>
          </Card>

          <Card>
            <CardHeader>
              <CardTitle>Details</CardTitle>
            </CardHeader>
            <CardContent>
              <DescriptionList
                items={[
                  { label: "Slug", value: data.slug, mono: true },
                  { label: "Folder", value: `.pi/skills/${data.slug}`, mono: true },
                  { label: "Version", value: data.version },
                  { label: "Visibility", value: titleCase(data.visibility) },
                  { label: "Created", value: formatDateTime(data.createdAt) },
                  { label: "Updated", value: formatDateTime(data.updatedAt) },
                ]}
              />
            </CardContent>
          </Card>
        </div>
      </div>
    </div>
  );
}
