"use client";

import { useMemo, useState } from "react";
import Link from "next/link";
import { useQueryClient } from "@tanstack/react-query";
import { BookOpen, Plus } from "lucide-react";

import { BulkActionsBar } from "@/components/bulk-actions-bar";
import { BulkAccessDialog } from "@/components/bulk-access-dialog";
import { ImportPortableButton } from "@/components/import-portable-dialog";
import { Button } from "@/components/ui/button";
import { Callout } from "@/components/ui/callout";
import { Checkbox } from "@/components/ui/checkbox";
import { EmptyState } from "@/components/ui/empty-state";
import { PageHeader } from "@/components/ui/page-header";
import {
  ListCard,
  ListCount,
  ListRow,
  ListSkeleton,
  ListToolbar,
  ROW_LINK,
  SearchField,
  toggleId,
  toggleShown,
} from "@/components/ui/resource-list";
import { Segmented } from "@/components/ui/segmented";
import { Status, type StatusMeta } from "@/components/ui/status";
import { cleanDescription } from "@/lib/presentation";
import { queryKeys, useSkills } from "@/lib/queries";
import { usageSummary, useSkillUsage } from "@/lib/skill-usage";
import type { Skill } from "@/lib/types";
import { relativeTime } from "@/lib/utils";
import { errorMessage } from "@orbit/api-client";

type Filter = "all" | "used" | "unused";

const FILTER_LABEL: Record<Filter, string> = {
  all: "All",
  used: "In use",
  unused: "Unused",
};

const IN_USE: StatusMeta = { label: "In use", tone: "success", shape: "solid" };
const UNUSED: StatusMeta = { label: "Unused", tone: "neutral", shape: "dashed" };

function fileCount(skill: Skill): number {
  const files = skill.files;
  return files && typeof files === "object" && !Array.isArray(files) ? Object.keys(files).length : 0;
}

export default function SkillsPage() {
  const skills = useSkills();
  const { usersOf } = useSkillUsage();
  const queryClient = useQueryClient();
  const [search, setSearch] = useState("");
  const [filter, setFilter] = useState<Filter>("all");
  const [selected, setSelected] = useState<Set<string>>(new Set());
  const [bulkOpen, setBulkOpen] = useState(false);

  const all = useMemo(() => skills.data ?? [], [skills.data]);
  const counts = useMemo(() => {
    const used = all.filter((skill) => usersOf(skill.slug).length > 0).length;
    return { all: all.length, used, unused: all.length - used } satisfies Record<Filter, number>;
  }, [all, usersOf]);
  const visible = useMemo(() => {
    const terms = search.toLowerCase().split(/\s+/).filter(Boolean);
    return all.filter((skill) => {
      const used = usersOf(skill.slug).length > 0;
      if (filter === "used" && !used) return false;
      if (filter === "unused" && used) return false;
      const haystack = `${skill.name} ${skill.slug} ${skill.description}`.toLowerCase();
      return terms.every((term) => haystack.includes(term));
    });
  }, [all, search, filter, usersOf]);

  return (
    <div className="space-y-6">
      <PageHeader
        title="Skills"
        description="Reusable SKILL.md packages. An agent loads a skill when its description fits the task; attach skills to agents, teams or worker profiles."
        actions={
          <div className="flex flex-wrap gap-2">
            <ImportPortableButton />
            <Button asChild>
              <Link href="/skills/new">
                <Plus /> New skill
              </Link>
            </Button>
          </div>
        }
      />

      <ListToolbar>
        <SearchField
          value={search}
          onChange={setSearch}
          placeholder="Filter by name, slug or description"
          label="Filter skills"
        />
        <Segmented
          label="Skill usage"
          value={filter}
          onChange={setFilter}
          options={(Object.keys(FILTER_LABEL) as Filter[]).map((value) => ({
            value,
            label: FILTER_LABEL[value],
            count: skills.data ? counts[value] : undefined,
          }))}
        />
      </ListToolbar>

      {selected.size ? (
        <BulkActionsBar
          resource="skills"
          selectedIds={[...selected]}
          allShownSelected={visible.length > 0 && visible.every((skill) => selected.has(skill.id))}
          onToggleAllShown={(checked) => setSelected((current) => toggleShown(current, visible.map((skill) => skill.id), checked))}
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

      {skills.isLoading ? (
        <ListSkeleton />
      ) : skills.isError ? (
        <Callout tone="danger" title="Skills could not be loaded">
          {errorMessage(skills.error)}
        </Callout>
      ) : visible.length ? (
        <>
          <ListCard columns={["Skill", "Status", "Used by", "Updated"]}>
            {visible.map((skill) => {
              const users = usersOf(skill.slug);
              const description = cleanDescription(skill.description);
              const files = fileCount(skill);
              return (
                <ListRow
                  key={skill.id}
                  checkbox={
                    <Checkbox
                      aria-label={`Select ${skill.name}`}
                      checked={selected.has(skill.id)}
                      onChange={(event) => setSelected((current) => toggleId(current, skill.id, event.target.checked))}
                    />
                  }
                  name={
                    <>
                      <p className="flex items-center gap-2">
                        <Link href={`/skills/${skill.id}`} className={ROW_LINK}>
                          {skill.name}
                        </Link>
                        <span className="tabular shrink-0 text-xs text-muted-foreground">v{skill.version}</span>
                      </p>
                      <p className="mt-0.5 line-clamp-1 text-[13px] text-muted-foreground">
                        {description || "No description: agents cannot tell when to use it"}
                      </p>
                      <p className="mt-1.5 flex flex-wrap items-center gap-x-3 gap-y-1 text-xs text-muted-foreground md:hidden">
                        <span className="font-mono">{skill.slug}</span>
                        <span>{usageSummary(users)}</span>
                        <span>{relativeTime(skill.updatedAt)}</span>
                      </p>
                    </>
                  }
                  status={<Status meta={users.length ? IN_USE : UNUSED} />}
                  details={
                    <>
                      <p className="truncate">{users.length ? users.map((user) => user.name).join(", ") : "Nothing yet"}</p>
                      <p className="mt-0.5 truncate text-xs text-muted-foreground">
                        <span className="font-mono">{skill.slug}</span>
                        {files ? ` · ${files} file${files === 1 ? "" : "s"}` : ""}
                      </p>
                    </>
                  }
                  updated={relativeTime(skill.updatedAt)}
                />
              );
            })}
          </ListCard>
          <ListCount shown={visible.length} total={counts[filter]} singular="skill" plural="skills" />
        </>
      ) : (
        <EmptyState
          icon={<BookOpen />}
          title={
            search
              ? `No skills match “${search}”`
              : all.length === 0
                ? "No skills yet"
                : `No ${FILTER_LABEL[filter].toLowerCase()} skills`
          }
          description={
            search
              ? "Try a shorter term, or clear the filter to see every skill."
              : all.length === 0
                ? "Write a SKILL.md once, then attach it to any agent. Pi finds it under .pi/skills; OpenCode under .opencode/skills."
                : "Skills in the other group are one tap away in the filter."
          }
          action={
            search ? (
              <Button variant="outline" onClick={() => setSearch("")}>
                Clear filter
              </Button>
            ) : all.length === 0 ? (
              <Button asChild variant="outline">
                <Link href="/skills/new">
                  <Plus /> New skill
                </Link>
              </Button>
            ) : (
              <Button variant="outline" onClick={() => setFilter("all")}>
                Show all skills
              </Button>
            )
          }
        />
      )}
    </div>
  );
}
