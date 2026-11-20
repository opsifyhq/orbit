"use client";

import { useMemo, useRef, useState } from "react";
import { useMutation, useQueryClient } from "@tanstack/react-query";
import {
  BookOpen,
  ChevronDown,
  Download,
  Ellipsis,
  Pencil,
  Plus,
  Search,
  Trash2,
  Upload,
} from "lucide-react";
import { toast } from "sonner";

import { ResourceAccessFields, type ResourceAccessHandle } from "@/components/resource-access-fields";
import { cleanDescription } from "@/lib/presentation";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { Callout } from "@/components/ui/callout";
import { Card } from "@/components/ui/card";
import { Checkbox } from "@/components/ui/checkbox";
import { BulkAccessDialog } from "@/components/bulk-access-dialog";
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
import { ImportPortableButton } from "@/components/import-portable-dialog";
import { exportResource } from "@/components/export-resource-button";
import { Skeleton } from "@/components/ui/skeleton";
import { Textarea } from "@/components/ui/textarea";
import { api } from "@/lib/api";
import { queryKeys, unwrap, useAgentTeams, useSkills } from "@/lib/queries";
import { errorMessage } from "@orbit/api-client";
import { cn, relativeTime } from "@/lib/utils";
import type { Skill } from "@/lib/types";

const TEMPLATE = `---
name: example-skill
description: What this skill does and when the agent should use it.
---

# Instructions

Describe the workflow, constraints and any bundled files.
`;

function parseFiles(text: string): Record<string, string> | null {
  try {
    const value = JSON.parse(text);
    return value && typeof value === "object" && !Array.isArray(value) ? value : null;
  } catch {
    return null;
  }
}

function bundledPaths(skill: Skill): string[] {
  const files = skill.files;
  return files && typeof files === "object" && !Array.isArray(files) ? Object.keys(files) : [];
}

export default function SkillsPage() {
  const accessRef = useRef<ResourceAccessHandle>(null);
  const skills = useSkills();
  const teams = useAgentTeams();
  const queryClient = useQueryClient();
  const confirm = useConfirm();
  const [open, setOpen] = useState(false);
  const [editing, setEditing] = useState<Skill | null>(null);
  const [viewing, setViewing] = useState<Skill | null>(null);
  const [search, setSearch] = useState("");
  const [selected, setSelected] = useState<Set<string>>(new Set());
  const [bulkOpen, setBulkOpen] = useState(false);
  const [name, setName] = useState("");
  const [slug, setSlug] = useState("");
  const [description, setDescription] = useState("");
  const [content, setContent] = useState(TEMPLATE);
  const [filesText, setFilesText] = useState("{}");
  const [showFiles, setShowFiles] = useState(false);
  const fileInputRef = useRef<HTMLInputElement>(null);

  function handleFileUpload(event: React.ChangeEvent<HTMLInputElement>) {
    const files = event.target.files;
    if (!files) return;
    // If the text is invalid, start from the stored files rather than silently
    // replacing them with `{}` as soon as a new file is picked.
    const stored = editing?.files;
    const fallback = stored && typeof stored === "object" && !Array.isArray(stored)
      ? (stored as Record<string, string>)
      : {};
    const current: Record<string, string> = { ...(parseFiles(filesText) ?? fallback) };
    Array.from(files).forEach((file) => {
      const reader = new FileReader();
      reader.onload = (e) => {
        const content = e.target?.result as string;
        current[file.name] = content;
        setFilesText(JSON.stringify(current, null, 2));
      };
      reader.readAsText(file);
    });
    // Reset input so the same file can be uploaded again.
    if (fileInputRef.current) fileInputRef.current.value = "";
  }

  const reset = () => {
    setEditing(null);
    setName("");
    setSlug("");
    setDescription("");
    setContent(TEMPLATE);
    setFilesText("{}");
    setShowFiles(false);
  };

  function openCreate() {
    reset();
    setOpen(true);
  }

  function openEdit(skill: Skill) {
    setViewing(null);
    setEditing(skill);
    setName(skill.name);
    setSlug(skill.slug);
    setDescription(skill.description);
    setContent(skill.content);
    setFilesText(JSON.stringify(skill.files ?? {}, null, 2));
    setShowFiles(bundledPaths(skill).length > 0);
    setOpen(true);
  }

  const save = useMutation({
    mutationFn: async () => {
      if (editing) {
        return unwrap(
          await api.PATCH("/api/v1/skills/{skill_id}", {
            params: { path: { skill_id: editing.id } },
            body: { name, description, content, files: parseFiles(filesText) ?? {} },
          }),
        );
      }
      return unwrap(
        await api.POST("/api/v1/skills", {
          body: { name, slug, description, content, files: parseFiles(filesText) ?? {} },
        }),
      );
    },
    onSuccess: () => {
      toast.success(editing ? "Skill updated" : "Skill created");
      setOpen(false);
      reset();
      void queryClient.invalidateQueries({ queryKey: queryKeys.skills });
    },
    onError: (error) => toast.error(errorMessage(error)),
  });

  async function saveSkill() {
    if (filesInvalid) {
      toast.error("Fix the bundled files JSON before saving.");
      return;
    }
    if ((await accessRef.current?.save()) === false) return;
    save.mutate();
  }

  const remove = useMutation({
    mutationFn: async (id: string) =>
      unwrap(
        await api.DELETE("/api/v1/skills/{skill_id}", {
          params: { path: { skill_id: id } },
        }),
      ),
    onSuccess: () => {
      toast.success("Skill deleted");
      setViewing(null);
      void queryClient.invalidateQueries({ queryKey: queryKeys.skills });
    },
    onError: (error) => toast.error(errorMessage(error)),
  });

  // Teams are the only place a skill's use is visible without opening each agent.
  const teamsUsing = useMemo(() => {
    const map = new Map<string, string[]>();
    for (const team of teams.data ?? []) {
      for (const slugName of team.definition.skills ?? []) {
        map.set(slugName, [...(map.get(slugName) ?? []), team.name]);
      }
    }
    return map;
  }, [teams.data]);

  async function deleteSkill(skill: Skill) {
    const used = teamsUsing.get(skill.slug)?.length ?? 0;
    const confirmed = await confirm({
      title: `Delete ${skill.name}?`,
      description: used
        ? `${used === 1 ? "One agent team uses" : `${used} agent teams use`} this skill. Agents and teams that reference it lose it. This cannot be undone.`
        : "Agents that reference it lose it. This cannot be undone.",
      confirmLabel: "Delete skill",
      destructive: true,
    });
    if (confirmed) remove.mutate(skill.id);
  }

  const shown = useMemo(() => {
    const query = search.trim().toLowerCase();
    const items = skills.data ?? [];
    if (!query) return items;
    return items.filter((skill) =>
      [skill.name, skill.slug, skill.description].some((value) => value.toLowerCase().includes(query)),
    );
  }, [skills.data, search]);

  const parsedFiles = parseFiles(filesText);
  const filesInvalid = filesText.trim() !== "" && parsedFiles === null;

  return (
    <div className="space-y-6">
      <PageHeader
        title="Skills"
        description="Reusable SKILL.md packages that are copied into an agent's workspace when it runs. Attach them to an agent in its settings, or to a team."
        actions={
          <div className="flex flex-wrap gap-2">
            <ImportPortableButton label="Import" />
            <Button onClick={openCreate}>
              <Plus /> New skill
            </Button>
          </div>
        }
      />

      {skills.isLoading ? (
        <Card className="overflow-hidden">
          {Array.from({ length: 3 }).map((_, index) => (
            <div key={index} className="border-b px-5 py-4 last:border-b-0">
              <Skeleton className="h-10" />
            </div>
          ))}
        </Card>
      ) : skills.isError ? (
        <Callout tone="danger" title="Could not load skills">
          {errorMessage(skills.error)}
        </Callout>
      ) : !skills.data?.length ? (
        <EmptyState
          icon={<BookOpen />}
          title="No skills yet"
          description="Write a SKILL.md once, then attach it to any agent. Pi finds it under .pi/skills; OpenCode under .opencode/skills."
          action={
            <Button variant="outline" onClick={openCreate}>
              <Plus /> New skill
            </Button>
          }
        />
      ) : (
        <>
          {skills.data.length > 4 ? (
            <div className="relative max-w-sm">
              <Search className="pointer-events-none absolute left-3 top-1/2 h-4 w-4 -translate-y-1/2 text-muted-foreground" />
              <Input
                aria-label="Search skills"
                className="pl-9"
                placeholder="Search skills"
                value={search}
                onChange={(event) => setSearch(event.target.value)}
              />
            </div>
          ) : null}

          {selected.size ? (
            <div className="flex flex-wrap items-center gap-3 rounded-lg border bg-subtle px-3 py-2.5">
              <Checkbox
                aria-label="Select all shown skills"
                checked={shown.every((skill) => selected.has(skill.id))}
                onChange={(event) =>
                  setSelected(
                    event.target.checked
                      ? new Set([...selected, ...shown.map((skill) => skill.id)])
                      : new Set([...selected].filter((id) => !shown.some((skill) => skill.id === id))),
                  )
                }
              />
              <span className="text-[13px] font-medium">{selected.size} selected</span>
              <Button size="sm" variant="outline" onClick={() => setBulkOpen(true)}>
                Change access
              </Button>
              <Button size="sm" variant="ghost" onClick={() => setSelected(new Set())}>
                Clear selection
              </Button>
            </div>
          ) : null}

          <BulkAccessDialog
            open={bulkOpen}
            onOpenChange={setBulkOpen}
            resource="skills"
            ids={[...selected]}
            onDone={() => {
              setSelected(new Set());
              void queryClient.invalidateQueries({ queryKey: ["skills"] });
            }}
          />

          {shown.length === 0 ? (
            <EmptyState
              icon={<Search />}
              title="No skills match"
              description="Try a different name or clear the search."
              action={
                <Button variant="outline" onClick={() => setSearch("")}>
                  Clear search
                </Button>
              }
            />
          ) : (
            <Card className="overflow-hidden">
              {shown.map((skill) => {
                const users = teamsUsing.get(skill.slug) ?? [];
                const files = bundledPaths(skill);
                return (
                  <div
                    key={skill.id}
                    className="flex flex-col gap-3 border-b px-4 py-4 last:border-b-0 sm:px-5 md:flex-row md:items-center md:gap-5"
                  >
                    <Checkbox
                      aria-label={`Select ${skill.name}`}
                      checked={selected.has(skill.id)}
                      onChange={(event) =>
                        setSelected((current) => {
                          const next = new Set(current);
                          if (event.target.checked) next.add(skill.id);
                          else next.delete(skill.id);
                          return next;
                        })
                      }
                    />
                    <div className="min-w-0 flex-1 space-y-1">
                      <div className="flex flex-wrap items-center gap-2">
                        <button
                          type="button"
                          onClick={() => setViewing(skill)}
                          className="min-w-0 truncate rounded text-left text-sm font-medium hover:text-primary hover:underline"
                        >
                          {skill.name}
                        </button>
                        <span className="break-all font-mono text-xs text-muted-foreground">{skill.slug}</span>
                        <Badge variant="outline">Version {skill.version}</Badge>
                      </div>
                      <p className="line-clamp-2 text-[13px] text-muted-foreground">
                        {cleanDescription(skill.description) || "No description. Agents cannot tell when to load this skill."}
                      </p>
                      <p className="text-[13px] text-muted-foreground">
                        {users.length
                          ? `Used by ${users.length === 1 ? "team" : "teams"} ${users.join(", ")}.`
                          : "Not attached to any team."}{" "}
                        {files.length ? `${files.length} bundled ${files.length === 1 ? "file" : "files"}. ` : ""}
                        Updated {relativeTime(skill.updatedAt)}.
                      </p>
                    </div>
                    <div className="flex shrink-0 items-center gap-2">
                      <Button size="sm" variant="outline" onClick={() => setViewing(skill)}>
                        View
                      </Button>
                      <Button size="sm" variant="outline" onClick={() => openEdit(skill)}>
                        <Pencil /> Edit
                      </Button>
                      <DropdownMenu>
                        <DropdownMenuTrigger asChild>
                          <Button size="icon-sm" variant="ghost" aria-label={`More actions for ${skill.name}`}>
                            <Ellipsis />
                          </Button>
                        </DropdownMenuTrigger>
                        <DropdownMenuContent align="end">
                          <DropdownMenuItem onSelect={() => void exportResource("skills", skill.id, skill.name, "markdown")}>
                            <Download /> Export SKILL.md
                          </DropdownMenuItem>
                          <DropdownMenuItem onSelect={() => void exportResource("skills", skill.id, skill.name)}>
                            <Download /> Export with files (YAML)
                          </DropdownMenuItem>
                          <DropdownMenuItem destructive onSelect={() => void deleteSkill(skill)}>
                            <Trash2 /> Delete skill
                          </DropdownMenuItem>
                        </DropdownMenuContent>
                      </DropdownMenu>
                    </div>
                  </div>
                );
              })}
            </Card>
          )}
        </>
      )}

      <Dialog open={Boolean(viewing)} onOpenChange={(next) => !next && setViewing(null)}>
        <DialogContent className="max-w-3xl">
          <DialogHeader>
            <DialogTitle>{viewing?.name}</DialogTitle>
            <DialogDescription>
              {cleanDescription(viewing?.description) || "No description."}
            </DialogDescription>
          </DialogHeader>
          {viewing ? (
            <div className="min-w-0 space-y-4">
              <div className="space-y-1.5">
                <p className="text-[13px] font-medium">SKILL.md</p>
                <pre className="code-block max-h-[50vh] whitespace-pre-wrap break-words">{viewing.content}</pre>
              </div>
              {bundledPaths(viewing).length ? (
                <div className="space-y-1.5">
                  <p className="text-[13px] font-medium">Bundled files</p>
                  <ul className="flex flex-wrap gap-1.5">
                    {bundledPaths(viewing).map((path) => (
                      <li key={path}>
                        <Badge variant="secondary" className="font-mono">
                          {path}
                        </Badge>
                      </li>
                    ))}
                  </ul>
                </div>
              ) : null}
            </div>
          ) : null}
          <DialogFooter>
            <Button variant="outline" onClick={() => setViewing(null)}>
              Close
            </Button>
            {viewing ? (
              <Button onClick={() => openEdit(viewing)}>
                <Pencil /> Edit skill
              </Button>
            ) : null}
          </DialogFooter>
        </DialogContent>
      </Dialog>

      <Dialog
        open={open}
        onOpenChange={(next) => {
          setOpen(next);
          if (!next) reset();
        }}
      >
        <DialogContent className="max-w-3xl">
          <DialogHeader>
            <DialogTitle>{editing ? `Edit ${editing.name}` : "New skill"}</DialogTitle>
            <DialogDescription>
              SKILL.md follows the Agent Skills format. Agents read the description to decide when to load it.
            </DialogDescription>
          </DialogHeader>
          <div className="space-y-4">
            <div className="grid gap-4 sm:grid-cols-2">
              <Field label="Name" htmlFor="skill-name">
                <Input
                  id="skill-name"
                  value={name}
                  onChange={(event) => {
                    setName(event.target.value);
                    if (!editing) {
                      setSlug(
                        event.target.value
                          .toLowerCase()
                          .replace(/[^a-z0-9]+/g, "-")
                          .replace(/^-|-$/g, "")
                          .slice(0, 64),
                      );
                    }
                  }}
                  placeholder="GitLab triage"
                />
              </Field>
              <Field
                label="Slug"
                htmlFor="skill-slug"
                hint={editing ? "The slug cannot change after creation." : "Short identifier used when attaching the skill."}
              >
                <Input
                  id="skill-slug"
                  className="font-mono"
                  value={slug}
                  disabled={Boolean(editing)}
                  onChange={(event) => setSlug(event.target.value)}
                  placeholder="gitlab-triage"
                />
              </Field>
            </div>
            <Field
              label="Description"
              htmlFor="skill-description"
              hint="Say when the agent should use this skill."
            >
              <Input
                id="skill-description"
                value={description}
                onChange={(event) => setDescription(event.target.value)}
                placeholder="Use when triaging GitLab issues and proposing labels."
              />
            </Field>
            <Field label="SKILL.md" htmlFor="skill-content">
              <Textarea
                id="skill-content"
                className="min-h-[280px] font-mono text-xs"
                spellCheck={false}
                value={content}
                onChange={(event) => setContent(event.target.value)}
              />
            </Field>

            <div className="rounded-lg border">
              <button
                type="button"
                aria-expanded={showFiles}
                onClick={() => setShowFiles((value) => !value)}
                className="flex w-full items-center justify-between gap-3 rounded-lg px-3 py-2.5 text-left"
              >
                <span>
                  <span className="block text-sm font-medium">Bundled files</span>
                  <span className="block text-[13px] text-muted-foreground">
                    {parsedFiles
                      ? `${Object.keys(parsedFiles).length} ${Object.keys(parsedFiles).length === 1 ? "file" : "files"}. Scripts and other files the skill needs.`
                      : "Scripts and other files the skill needs."}
                  </span>
                </span>
                <ChevronDown
                  className={cn("h-4 w-4 shrink-0 text-muted-foreground transition-transform", showFiles && "rotate-180")}
                />
              </button>
              {showFiles ? (
                <div className="space-y-2 border-t px-3 py-3">
                  <div className="flex flex-wrap items-center justify-between gap-2">
                    <p className="text-[13px] text-muted-foreground">
                      JSON mapping each path to its content. Scripts go under{" "}
                      <code className="font-mono text-xs">scripts/</code> and may end in .sh, .bash, .py, .js,
                      .ts, .rb, .pl or .go.
                    </p>
                    <Button type="button" size="sm" variant="outline" onClick={() => fileInputRef.current?.click()}>
                      <Upload /> Add files
                    </Button>
                    <input
                      ref={fileInputRef}
                      type="file"
                      multiple
                      className="hidden"
                      aria-label="Add bundled files"
                      onChange={handleFileUpload}
                    />
                  </div>
                  <Field
                    label="Files as JSON"
                    htmlFor="skill-files"
                    error={filesInvalid ? "This is not valid JSON. Fix it before saving." : undefined}
                  >
                    <Textarea
                      id="skill-files"
                      className="min-h-[120px] font-mono text-xs"
                      spellCheck={false}
                      value={filesText}
                      onChange={(event) => setFilesText(event.target.value)}
                      placeholder='{"scripts/run.sh": "#!/bin/bash\necho hello"}'
                    />
                  </Field>
                </div>
              ) : null}
            </div>
          </div>
          {filesInvalid ? (
            <Callout tone="danger" title="Fix the bundled files JSON before saving">
              The Files as JSON field is not valid JSON. Correct it, then try saving again.
            </Callout>
          ) : null}
          <ResourceAccessFields ref={accessRef} resource="skills" resourceId={editing?.id} />
          <DialogFooter>
            <Button variant="outline" onClick={() => setOpen(false)}>
              Cancel
            </Button>
            <Button
              onClick={() => void saveSkill()}
              loading={save.isPending}
              disabled={!name.trim() || !slug.trim() || !content.trim() || filesInvalid}
            >
              {editing ? "Save skill" : "Create skill"}
            </Button>
          </DialogFooter>
        </DialogContent>
      </Dialog>
    </div>
  );
}
