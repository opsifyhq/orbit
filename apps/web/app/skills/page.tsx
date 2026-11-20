"use client";

import { useMemo, useState } from "react";
import Link from "next/link";
import { useQueryClient } from "@tanstack/react-query";
import { BookOpen, Plus, Search, X } from "lucide-react";

import { BulkActionsBar } from "@/components/bulk-actions-bar";
import { BulkAccessDialog } from "@/components/bulk-access-dialog";
import { ImportPortableButton } from "@/components/import-portable-dialog";
import { Button } from "@/components/ui/button";
import { Callout } from "@/components/ui/callout";
import { Card } from "@/components/ui/card";
import { Checkbox } from "@/components/ui/checkbox";
import { EmptyState } from "@/components/ui/empty-state";
import { Input } from "@/components/ui/input";
import { PageHeader } from "@/components/ui/page-header";
import { Segmented } from "@/components/ui/segmented";
import { Skeleton } from "@/components/ui/skeleton";
import { cleanDescription } from "@/lib/presentation";
import { queryKeys, useSkills } from "@/lib/queries";
import { usageSummary, useSkillUsage, type SkillUser } from "@/lib/skill-usage";
import type { Skill } from "@/lib/types";
import { relativeTime } from "@/lib/utils";
import { errorMessage } from "@orbit/api-client";

type Filter = "all" | "used" | "unused";

function fileCount(skill: Skill): number {
  const files = skill.files;
  return files && typeof files === "object" && !Array.isArray(files) ? Object.keys(files).length : 0;
}

function SkillRow({
  skill,
  users,
  selected,
  onSelect,
}: {
  skill: Skill;
  users: SkillUser[];
  selected: boolean;
  onSelect: (checked: boolean) => void;
}) {
  const description = cleanDescription(skill.description);
  const files = fileCount(skill);
  return (
    <li className="group relative border-b last:border-b-0">
      <div className="grid grid-cols-[auto_minmax(0,1fr)] items-start gap-x-3 gap-y-1 px-4 py-3.5 transition-colors group-hover:bg-muted/50 sm:px-5 md:grid-cols-[1.5rem_minmax(0,1fr)_12rem_6.5rem] md:items-center md:gap-x-4">
        {/* Above the row link so ticking it does not open the skill. */}
        <Checkbox
          aria-label={`Select ${skill.name}`}
          className="relative z-10 mt-0.5 md:mt-0"
          checked={selected}
          onChange={(event) => onSelect(event.target.checked)}
        />
        <div className="min-w-0">
          <p className="flex min-w-0 items-baseline gap-2">
            <Link
              href={`/skills/${skill.id}`}
              className="truncate text-sm font-semibold after:absolute after:inset-0 focus-visible:outline-none focus-visible:after:outline focus-visible:after:outline-2 focus-visible:after:-outline-offset-2 focus-visible:after:outline-ring"
            >
              {skill.name}
            </Link>
            {skill.name !== skill.slug ? (
              <span className="hidden shrink-0 font-mono text-xs text-muted-foreground sm:inline">{skill.slug}</span>
            ) : null}
          </p>
          <p className={description ? "mt-0.5 line-clamp-1 text-[13px] text-muted-foreground" : "mt-0.5 text-[13px] text-warning"}>
            {description || "No description: agents cannot tell when to use it"}
          </p>
          <p className="mt-1 text-xs text-muted-foreground md:hidden">
            {usageSummary(users)}
            {files ? ` · ${files} file${files === 1 ? "" : "s"}` : ""} · {relativeTime(skill.updatedAt)}
          </p>
        </div>
        <div className="hidden min-w-0 text-[13px] md:block">
          <p className={users.length ? "truncate" : "text-muted-foreground"}>{usageSummary(users)}</p>
          {users.length ? (
            <p className="truncate text-xs text-muted-foreground">{users.map((user) => user.name).join(", ")}</p>
          ) : files ? (
            <p className="text-xs text-muted-foreground">{files} bundled file{files === 1 ? "" : "s"}</p>
          ) : null}
        </div>
        <p className="tabular hidden text-right text-[13px] text-muted-foreground md:block">{relativeTime(skill.updatedAt)}</p>
      </div>
    </li>
  );
}

export default function SkillsPage() {
  const skills = useSkills();
  const { usersOf } = useSkillUsage();
  const queryClient = useQueryClient();
  const [search, setSearch] = useState("");
  const [filter, setFilter] = useState<Filter>("all");
  const [selected, setSelected] = useState<Set<string>>(new Set());
  const [bulkOpen, setBulkOpen] = useState(false);

  const all = skills.data ?? [];
  const counts = {
    all: all.length,
    used: all.filter((skill) => usersOf(skill.slug).length).length,
    unused: all.filter((skill) => !usersOf(skill.slug).length).length,
  };
  const shown = useMemo(() => {
    const terms = search.toLowerCase().split(/\s+/).filter(Boolean);
    return all.filter((skill) => {
      const used = usersOf(skill.slug).length > 0;
      if (filter === "used" && !used) return false;
      if (filter === "unused" && used) return false;
      const haystack = `${skill.name} ${skill.slug} ${skill.description}`.toLowerCase();
      return terms.every((term) => haystack.includes(term));
    });
  }, [all, search, filter, usersOf]);

  const allShownSelected = shown.length > 0 && shown.every((skill) => selected.has(skill.id));

  return (
    <div className="space-y-6">
      <PageHeader
        title="Skills"
        description="Reusable SKILL.md packages. An agent loads a skill when its description fits the task; attach skills to agents, teams or worker profiles."
        actions={
          <div className="flex flex-wrap gap-2">
            <ImportPortableButton label="Import" />
            <Button asChild>
              <Link href="/skills/new">
                <Plus /> New skill
              </Link>
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
        <Callout tone="danger" title="Skills could not be loaded">
          {errorMessage(skills.error)}
        </Callout>
      ) : !all.length ? (
        <EmptyState
          icon={<BookOpen />}
          title="No skills yet"
          description="Write a SKILL.md once, then attach it to any agent. Pi finds it under .pi/skills; OpenCode under .opencode/skills."
          action={
            <Button asChild variant="outline">
              <Link href="/skills/new">
                <Plus /> New skill
              </Link>
            </Button>
          }
        />
      ) : (
        <div className="space-y-4">
          <div className="flex flex-col gap-3 sm:flex-row sm:items-center sm:justify-between">
            <Segmented
              label="Show skills"
              value={filter}
              onChange={setFilter}
              options={[
                { value: "all", label: "All", count: counts.all },
                { value: "used", label: "In use", count: counts.used },
                { value: "unused", label: "Unused", count: counts.unused },
              ]}
            />
            {all.length > 3 ? (
              <div className="relative sm:w-72">
                <Search className="pointer-events-none absolute left-3 top-1/2 h-4 w-4 -translate-y-1/2 text-muted-foreground" />
                <Input
                  type="search"
                  className="pl-9 pr-9 [&::-webkit-search-cancel-button]:hidden"
                  placeholder="Filter by name, slug or description"
                  aria-label="Filter skills"
                  value={search}
                  onChange={(event) => setSearch(event.target.value)}
                />
                {search ? (
                  <button
                    type="button"
                    aria-label="Clear filter"
                    onClick={() => setSearch("")}
                    className="absolute right-1.5 top-1/2 flex h-6 w-6 -translate-y-1/2 items-center justify-center rounded text-muted-foreground hover:bg-muted hover:text-foreground"
                  >
                    <X className="h-3.5 w-3.5" />
                  </button>
                ) : null}
              </div>
            ) : null}
          </div>

          {selected.size ? (
            <BulkActionsBar
              resource="skills"
              selectedIds={[...selected]}
              allShownSelected={allShownSelected}
              onToggleAllShown={(checked) =>
                setSelected(
                  checked
                    ? new Set([...selected, ...shown.map((skill) => skill.id)])
                    : new Set([...selected].filter((id) => !shown.some((skill) => skill.id === id))),
                )
              }
              onChangeAccess={() => setBulkOpen(true)}
              onSucceeded={(ids) => {
                setSelected((current) => {
                  const next = new Set(current);
                  ids.forEach((id) => next.delete(id));
                  return next;
                });
                void queryClient.invalidateQueries({ queryKey: queryKeys.skills });
              }}
              onClear={() => setSelected(new Set())}
            />
          ) : null}

          <BulkAccessDialog
            open={bulkOpen}
            onOpenChange={setBulkOpen}
            resource="skills"
            ids={[...selected]}
            onDone={() => {
              setSelected(new Set());
              void queryClient.invalidateQueries({ queryKey: queryKeys.skills });
            }}
          />

          {shown.length ? (
            <Card className="overflow-hidden">
              <div className="grid grid-cols-[auto_minmax(0,1fr)] items-center gap-x-3 border-b bg-subtle px-4 py-2 text-xs font-medium text-muted-foreground sm:px-5 md:grid-cols-[1.5rem_minmax(0,1fr)_12rem_6.5rem] md:gap-x-4">
                <Checkbox
                  aria-label="Select all shown skills"
                  checked={allShownSelected}
                  onChange={(event) =>
                    setSelected(
                      event.target.checked
                        ? new Set([...selected, ...shown.map((skill) => skill.id)])
                        : new Set([...selected].filter((id) => !shown.some((skill) => skill.id === id))),
                    )
                  }
                />
                <span>Skill</span>
                <span className="hidden md:block">Used by</span>
                <span className="hidden text-right md:block">Updated</span>
              </div>
              <ul>
                {shown.map((skill) => (
                  <SkillRow
                    key={skill.id}
                    skill={skill}
                    users={usersOf(skill.slug)}
                    selected={selected.has(skill.id)}
                    onSelect={(checked) =>
                      setSelected((current) => {
                        const next = new Set(current);
                        if (checked) next.add(skill.id);
                        else next.delete(skill.id);
                        return next;
                      })
                    }
                  />
                ))}
              </ul>
            </Card>
          ) : (
            <EmptyState
              icon={<Search />}
              title="No skills match"
              description={search ? `Nothing matches "${search}".` : "No skills in this view."}
              action={
                <Button
                  variant="outline"
                  onClick={() => {
                    setSearch("");
                    setFilter("all");
                  }}
                >
                  Show all skills
                </Button>
              }
            />
          )}
        </div>
      )}
    </div>
  );
}
