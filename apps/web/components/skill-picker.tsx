"use client";

import { useId, useMemo, useState, type ReactNode } from "react";
import { Search, X } from "lucide-react";

import { Checkbox } from "@/components/ui/checkbox";
import { Input } from "@/components/ui/input";
import { Segmented } from "@/components/ui/segmented";
import { cleanDescription } from "@/lib/presentation";
import { cn } from "@/lib/utils";

type SkillOption = { slug: string; name: string; description?: string | null };

/**
 * Choose skills from a list that can be long. The list keeps a fixed height and scrolls,
 * descriptions are cleaned and clamped to two lines, and a search plus a "Selected"
 * filter make a choice among dozens of skills quick to make and quick to check.
 */
export function SkillPicker({
  skills,
  value,
  onChange,
  empty,
  className,
}: {
  skills: SkillOption[];
  value: string[];
  onChange: (next: string[]) => void;
  empty?: ReactNode;
  className?: string;
}) {
  const searchId = useId();
  const [query, setQuery] = useState("");
  const [show, setShow] = useState<"all" | "selected">("all");

  const sorted = useMemo(
    () => [...skills].sort((a, b) => a.name.localeCompare(b.name, undefined, { sensitivity: "base" })),
    [skills],
  );
  const visible = useMemo(() => {
    const needle = query.trim().toLowerCase();
    return sorted.filter((skill) => {
      if (show === "selected" && !value.includes(skill.slug)) return false;
      if (!needle) return true;
      return [skill.name, skill.slug, skill.description ?? ""].some((text) =>
        text.toLowerCase().includes(needle),
      );
    });
  }, [sorted, query, show, value]);

  // Selected slugs with no skill behind them: deleted since, or not visible to you. The
  // API refuses to save an agent that references them, so they are shown and removable.
  const missing = value.filter((slug) => !skills.some((skill) => skill.slug === slug));
  const missingNotice = missing.length ? (
    <div role="alert" className="rounded-lg border border-warning/35 bg-warning/[0.08] p-3 text-[13px]">
      <p className="font-medium">
        {missing.length === 1 ? "A selected skill no longer exists" : `${missing.length} selected skills no longer exist`}
      </p>
      <p className="mt-0.5 text-muted-foreground">
        Deleted, or not visible to you. Remove {missing.length === 1 ? "it" : "them"} to save.
      </p>
      <ul className="mt-2 flex flex-wrap gap-1.5">
        {missing.map((slug) => (
          <li key={slug}>
            <button
              type="button"
              onClick={() => onChange(value.filter((item) => item !== slug))}
              aria-label={`Remove ${slug}`}
              className="inline-flex items-center gap-1 rounded-md border bg-card px-2 py-1 font-mono text-xs hover:bg-muted"
            >
              {slug} <X className="h-3 w-3" aria-hidden />
            </button>
          </li>
        ))}
      </ul>
    </div>
  ) : null;

  if (skills.length === 0) {
    return (
      <div className="space-y-2.5">
        {missingNotice}
        <div className="text-[13px] text-muted-foreground">{empty ?? "No skills yet."}</div>
      </div>
    );
  }

  const toggle = (slug: string, checked: boolean) =>
    onChange(checked ? [...value, slug] : value.filter((item) => item !== slug));
  const searchable = skills.length > 6;

  return (
    <div className={cn("space-y-2.5", className)}>
      {missingNotice}
      <div className="flex flex-wrap items-center gap-2">
        {searchable ? (
          <div className="relative min-w-0 flex-1 basis-48">
            <Search
              aria-hidden
              className="pointer-events-none absolute left-2.5 top-1/2 h-3.5 w-3.5 -translate-y-1/2 text-muted-foreground"
            />
            <label htmlFor={searchId} className="sr-only">
              Search skills
            </label>
            <Input
              id={searchId}
              value={query}
              onChange={(event) => setQuery(event.target.value)}
              placeholder="Search skills"
              className="pl-8"
            />
          </div>
        ) : null}
        <Segmented
          label="Show skills"
          value={show}
          size="sm"
          onChange={setShow}
          options={[
            { value: "all", label: "All", count: skills.length },
            { value: "selected", label: "Selected", count: value.length },
          ]}
        />
        <div className="ml-auto flex items-center gap-3">
          <button
            type="button"
            onClick={() => {
              const visibleSlugs = visible.map((skill) => skill.slug);
              onChange([...new Set([...value, ...visibleSlugs])]);
            }}
            disabled={visible.length === 0 || visible.every((skill) => value.includes(skill.slug))}
            className="rounded text-xs font-medium text-primary underline-offset-4 hover:underline disabled:cursor-not-allowed disabled:text-muted-foreground disabled:no-underline"
          >
            Select all{visible.length !== skills.length ? ` ${visible.length}` : ""}
          </button>
          {value.length ? (
            <button
              type="button"
              onClick={() => onChange([])}
              className="rounded text-xs font-medium text-muted-foreground hover:text-foreground"
            >
              Select none
            </button>
          ) : null}
        </div>
      </div>

      <ul className="max-h-[22rem] divide-y overflow-y-auto rounded-lg border bg-card">
        {visible.length === 0 ? (
          <li className="px-3 py-6 text-center text-[13px] text-muted-foreground">
            {show === "selected" && !query ? "No skills selected yet." : "No skills match."}
            {query || show === "selected" ? (
              <>
                {" "}
                <button
                  type="button"
                  onClick={() => {
                    setQuery("");
                    setShow("all");
                  }}
                  className="rounded font-medium text-primary underline-offset-4 hover:underline"
                >
                  {query ? "Clear filters" : "Show all skills"}
                </button>
              </>
            ) : null}
          </li>
        ) : null}
        {visible.map((skill) => {
          const description = cleanDescription(skill.description);
          const checked = value.includes(skill.slug);
          return (
            <li key={skill.slug}>
              <label
                className={cn(
                  "flex cursor-pointer items-start gap-3 px-3 py-2.5 transition-colors hover:bg-muted/50",
                  checked && "bg-accent/40",
                )}
              >
                <Checkbox
                  className="mt-0.5"
                  checked={checked}
                  onChange={(event) => toggle(skill.slug, event.target.checked)}
                />
                <span className="min-w-0 flex-1">
                  <span className="flex flex-wrap items-baseline gap-x-2">
                    <span className="text-sm font-medium">{skill.name}</span>
                    {skill.name !== skill.slug ? (
                      <span className="font-mono text-xs text-muted-foreground">{skill.slug}</span>
                    ) : null}
                  </span>
                  <span
                    className="mt-0.5 line-clamp-2 text-[13px] text-muted-foreground"
                    title={description || undefined}
                  >
                    {description || "No description."}
                  </span>
                </span>
              </label>
            </li>
          );
        })}
      </ul>
    </div>
  );
}
