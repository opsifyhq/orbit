"use client";

import { useEffect, useMemo, useRef, useState, type ReactNode } from "react";
import Link from "next/link";
import { ChevronLeft, FilePlus2, FileText, Trash2, Upload } from "lucide-react";
import { toast } from "sonner";

import { ResourceAccessFields, type ResourceAccessHandle } from "@/components/resource-access-fields";
import {
  SectionCard,
  SectionPills,
  SectionRail,
  sectionNeighbours,
  type NavSection,
} from "@/components/ui/section-nav";
import { Button } from "@/components/ui/button";
import { Callout } from "@/components/ui/callout";
import { Card, CardContent, CardHeader } from "@/components/ui/card";
import { Disclosure } from "@/components/ui/disclosure";
import { Field } from "@/components/ui/field";
import { Input } from "@/components/ui/input";
import { Markdown } from "@/components/ui/markdown";
import { Segmented } from "@/components/ui/segmented";
import { Textarea } from "@/components/ui/textarea";
import {
  MAX_FILES,
  SCRIPT_EXTENSIONS,
  composeSkill,
  filePathError,
  parseSkill,
  slugError,
  slugify,
} from "@/lib/skill-md";
import type { Skill } from "@/lib/types";
import { cn } from "@/lib/utils";

type SectionId = "identity" | "instructions" | "files" | "sharing";

const SECTION_TITLES: Record<SectionId, string> = {
  identity: "Identity",
  instructions: "Instructions",
  files: "Bundled files",
  sharing: "Access control",
};
const SECTION_ORDER: SectionId[] = ["identity", "instructions", "files", "sharing"];
const isSectionId = (value: string): value is SectionId => (SECTION_ORDER as string[]).includes(value);

const DEFAULT_BODY = `# Instructions

Describe, step by step, how the agent should do this task. Mention any bundled files by
their path, for example scripts/run.sh.
`;

export interface SkillInput {
  name: string;
  slug: string;
  description: string;
  content: string;
  files: Record<string, string>;
}

interface BundledFile {
  path: string;
  content: string;
}

interface Draft {
  name: string;
  slug: string;
  description: string;
  body: string;
  /** Frontmatter keys other than name and description, verbatim. */
  extra: string;
  files: BundledFile[];
}

function draftFrom(skill?: Skill): Draft {
  if (!skill) return { name: "", slug: "", description: "", body: DEFAULT_BODY, extra: "", files: [] };
  const doc = parseSkill(skill.content);
  const files = skill.files && typeof skill.files === "object" && !Array.isArray(skill.files)
    ? Object.entries(skill.files as Record<string, unknown>).map(([path, content]) => ({
        path,
        content: typeof content === "string" ? content : JSON.stringify(content),
      }))
    : [];
  return {
    name: skill.name,
    slug: skill.slug,
    description: skill.description || doc.description,
    body: doc.body,
    extra: doc.extra.join("\n"),
    files: files.sort((a, b) => a.path.localeCompare(b.path)),
  };
}

interface Issue {
  section: SectionId;
  message: string;
}

const sizeLabel = (text: string) => {
  const bytes = new TextEncoder().encode(text).length;
  return bytes < 1024 ? `${bytes} B` : `${(bytes / 1024).toFixed(1)} KB`;
};

function FormSection({
  id,
  active,
  go,
  description,
  children,
}: {
  id: SectionId;
  active: SectionId;
  go: (id: SectionId) => void;
  description: string;
  children: ReactNode;
}) {
  return (
    <SectionCard
      id={id}
      active={active}
      title={SECTION_TITLES[id]}
      description={description}
      onGo={go}
      {...sectionNeighbours(SECTION_ORDER, SECTION_TITLES, id)}
    >
      {children}
    </SectionCard>
  );
}

/** Read an uploaded file as UTF-8 text; null when it is binary. */
async function readText(file: File): Promise<string | null> {
  const buffer = await file.arrayBuffer();
  try {
    const text = new TextDecoder("utf-8", { fatal: true }).decode(buffer);
    return text.includes("\u0000") ? null : text;
  } catch {
    return null;
  }
}

/**
 * Skill Studio: the same one-section-at-a-time form as Agent and Team Studio. Name,
 * slug and description are fields, and Save writes them into the SKILL.md frontmatter,
 * so what agents read always matches what the list shows. Bundled files are a real
 * file list instead of JSON.
 */
export function SkillForm({
  skill,
  submitLabel,
  submitting,
  onSubmit,
  cancelHref,
  serverIssues = [],
}: {
  skill?: Skill;
  submitLabel: string;
  submitting?: boolean;
  onSubmit: (input: SkillInput) => Promise<void> | void;
  cancelHref: string;
  /** Validation details from the last failed save (`path` + `message`). */
  serverIssues?: Array<{ path: string; message: string }>;
}) {
  const editing = Boolean(skill);
  const [draft, setDraft] = useState<Draft>(() => draftFrom(skill));
  const [slugTouched, setSlugTouched] = useState(editing);
  const [selectedFile, setSelectedFile] = useState(0);
  const [bodyView, setBodyView] = useState<"write" | "preview">("write");
  const accessRef = useRef<ResourceAccessHandle>(null);
  const uploadRef = useRef<HTMLInputElement>(null);
  const set = <K extends keyof Draft>(key: K, value: Draft[K]) => setDraft((current) => ({ ...current, [key]: value }));

  // What the stored SKILL.md said, to explain a save that brings it back in line.
  const stored = useMemo(() => (skill ? parseSkill(skill.content) : null), [skill]);

  const setFile = (index: number, patch: Partial<BundledFile>) =>
    setDraft((current) => ({
      ...current,
      files: current.files.map((file, i) => (i === index ? { ...file, ...patch } : file)),
    }));
  const removeFile = (index: number) => {
    setDraft((current) => ({ ...current, files: current.files.filter((_, i) => i !== index) }));
    setSelectedFile((current) => Math.max(0, current >= index ? current - 1 : current));
  };
  function addFile(path = "", content = "") {
    setDraft((current) => ({ ...current, files: [...current.files, { path, content }] }));
    setSelectedFile(draft.files.length);
  }
  async function upload(list: FileList | null) {
    if (!list?.length) return;
    const files = [...draft.files];
    const refused: string[] = [];
    let last = selectedFile;
    for (const file of Array.from(list)) {
      const text = await readText(file);
      if (text === null) {
        refused.push(file.name);
        continue;
      }
      const existing = files.findIndex((item) => item.path === file.name);
      if (existing >= 0) {
        files[existing] = { path: file.name, content: text };
        last = existing;
      } else {
        files.push({ path: file.name, content: text });
        last = files.length - 1;
      }
    }
    setDraft((current) => ({ ...current, files }));
    setSelectedFile(last);
    if (refused.length) {
      toast.error(`${refused.join(", ")} ${refused.length === 1 ? "is not a text file" : "are not text files"}. Only text files can be bundled.`);
    }
    if (uploadRef.current) uploadRef.current.value = "";
  }

  // ------------------------------------------------------------------ validation
  const issues: Issue[] = [];
  if (!draft.name.trim()) issues.push({ section: "identity", message: "Enter a name." });
  if (draft.name.trim().length > 120) issues.push({ section: "identity", message: "Keep the name to 120 characters." });
  if (!editing) {
    const problem = slugError(draft.slug);
    if (problem) issues.push({ section: "identity", message: problem });
  }
  if (!draft.body.trim()) issues.push({ section: "instructions", message: "Write the instructions." });
  if (/^---\s*$/m.test(draft.extra)) issues.push({ section: "instructions", message: "Extra frontmatter cannot contain a --- line." });
  if (draft.files.length > MAX_FILES) issues.push({ section: "files", message: `A skill can bundle at most ${MAX_FILES} files.` });
  const pathCounts = new Map<string, number>();
  for (const file of draft.files) pathCounts.set(file.path, (pathCounts.get(file.path) ?? 0) + 1);
  const fileError = (file: BundledFile) =>
    filePathError(file.path) ?? ((pathCounts.get(file.path) ?? 0) > 1 ? "Another file has this path." : null);
  const badFiles = draft.files.filter((file) => fileError(file));
  if (badFiles.length) {
    issues.push({ section: "files", message: `Fix the path of ${badFiles.map((file) => file.path || "an unnamed file").join(", ")}.` });
  }
  const sectionOfPath = (path: string): SectionId =>
    path.startsWith("files") ? "files" : path === "content" ? "instructions" : path.startsWith("sharing") ? "sharing" : "identity";
  for (const issue of serverIssues) issues.push({ section: sectionOfPath(issue.path), message: issue.message });

  const [showIssues, setShowIssues] = useState(false);
  useEffect(() => {
    if (serverIssues.length) setShowIssues(true);
  }, [serverIssues]);
  const issueCount = (id: SectionId) => (showIssues ? issues.filter((issue) => issue.section === id).length : 0);

  // ------------------------------------------------------------------ sections
  const [active, setActive] = useState<SectionId>("identity");
  const [focusRequest, setFocusRequest] = useState(0);
  useEffect(() => {
    const sync = () => {
      const fromHash = window.location.hash.replace(/^#/, "");
      if (isSectionId(fromHash)) setActive(fromHash);
    };
    sync();
    window.addEventListener("hashchange", sync);
    return () => window.removeEventListener("hashchange", sync);
  }, []);
  useEffect(() => {
    if (!focusRequest) return;
    const node = document.getElementById(`section-${active}`);
    node?.scrollIntoView({ block: "start" });
    node?.querySelector<HTMLElement>("input:not([type=hidden]):not(:disabled), textarea, select")?.focus({ preventScroll: true });
  }, [focusRequest, active]);
  function go(id: SectionId) {
    setActive(id);
    setFocusRequest((count) => count + 1);
    window.history.replaceState(null, "", `#${id}`);
  }

  async function submit() {
    const local = issues.filter((issue) => !serverIssues.some((server) => server.message === issue.message));
    if (local.length) {
      setShowIssues(true);
      go(local[0].section);
      toast.error(local.length === 1 ? local[0].message : "Fix the highlighted sections");
      return;
    }
    if ((await accessRef.current?.save()) === false) return;
    const extra = draft.extra.split(/\r?\n/).filter((line, i, all) => line.trim() !== "" || (i > 0 && i < all.length - 1));
    await onSubmit({
      name: draft.name.trim(),
      slug: draft.slug.trim(),
      description: draft.description.replace(/\s+/g, " ").trim(),
      content: composeSkill({ slug: draft.slug.trim(), description: draft.description, body: draft.body, extra }),
      files: Object.fromEntries(draft.files.map((file) => [file.path, file.content])),
    });
  }

  // ------------------------------------------------------------------ summaries
  const plural = (n: number, word: string) => `${n} ${word}${n === 1 ? "" : "s"}`;
  const summary: Record<SectionId, { value: string; missing?: boolean }> = {
    identity: {
      value: draft.name.trim() ? (draft.description.trim() ? draft.name.trim() : `${draft.name.trim()} · no description`) : "Name not set",
      missing: !draft.name.trim() || !draft.description.trim(),
    },
    instructions: {
      value: draft.body.trim() ? `${draft.body.trim().split(/\s+/).length} words` : "Not written",
      missing: !draft.body.trim(),
    },
    files: { value: draft.files.length ? plural(draft.files.length, "file") : "None" },
    sharing: { value: editing ? "Who can see and use it" : "After the first save" },
  };

  const navSections: NavSection<SectionId>[] = SECTION_ORDER.map((id) => ({
    id,
    title: SECTION_TITLES[id],
    value: summary[id].value,
    missing: summary[id].missing,
    problems: issueCount(id),
  }));
  const sectionNav = (
    <SectionRail label="Skill sections" sections={navSections} active={active} onGo={go} wrapValues />
  );

  const sectionIssues = (id: SectionId) =>
    showIssues && issues.some((issue) => issue.section === id) ? (
      <Callout tone="danger" title="Fix before saving">
        <ul className="list-disc space-y-0.5 pl-4">
          {issues
            .filter((issue) => issue.section === id)
            .map((issue) => (
              <li key={issue.message}>{issue.message}</li>
            ))}
        </ul>
      </Callout>
    ) : null;

  const extraLines = draft.extra.trim() ? draft.extra.trim().split(/\n/).length : 0;
  const current = draft.files[selectedFile];
  // A file just added has no path yet: say so only once something is typed or Save is tried.
  const currentError = current && (current.path || showIssues) ? fileError(current) : null;
  const renamed = stored && stored.name && stored.name !== draft.slug;
  const describedDifferently = stored && stored.description && stored.description !== draft.description.replace(/\s+/g, " ").trim();

  return (
    <div className="pb-24 lg:pb-0">
      {/* Phones and tablets: the sections as a scrollable row of steps. */}
      <SectionPills label="Skill sections" sections={navSections} active={active} onGo={go} className="mb-4" />

      <div className="grid gap-6 lg:grid-cols-[15rem_minmax(0,1fr)] lg:items-start">
        <aside className="hidden lg:sticky lg:top-6 lg:block">
          {sectionNav}
          <div className="mt-4 space-y-2 border-t pt-4">
            {showIssues && issues.length ? (
              <p className="text-[13px] text-destructive" role="status">
                {issues.length} {issues.length === 1 ? "issue needs" : "issues need"} fixing.
              </p>
            ) : null}
            <Button onClick={() => void submit()} loading={submitting} size="lg" className="w-full">
              {submitLabel}
            </Button>
            <Button asChild variant="ghost" className="w-full">
              <Link href={cancelHref}>Cancel</Link>
            </Button>
          </div>
        </aside>

        <div className="min-w-0 max-w-3xl space-y-5">
          <FormSection id="identity" active={active} go={go} description="How people and agents recognise the skill.">
            {sectionIssues("identity")}
            <div className="grid gap-4 sm:grid-cols-2">
              <Field label="Name" htmlFor="skill-name" hint="Shown in lists and pickers.">
                <Input
                  id="skill-name"
                  value={draft.name}
                  onChange={(event) => {
                    const name = event.target.value;
                    setDraft((current) => ({ ...current, name, slug: slugTouched ? current.slug : slugify(name) }));
                  }}
                  placeholder="Release notes writer"
                />
              </Field>
              <Field
                label="Slug"
                htmlFor="skill-slug"
                hint={
                  editing
                    ? "Fixed after creation: agents and teams refer to the skill by it."
                    : "The skill's folder and its name in SKILL.md. Agents and teams refer to it by this."
                }
                error={!editing && showIssues ? slugError(draft.slug) : undefined}
              >
                <Input
                  id="skill-slug"
                  className="font-mono"
                  value={draft.slug}
                  disabled={editing}
                  onChange={(event) => {
                    setSlugTouched(true);
                    set("slug", event.target.value);
                  }}
                  placeholder="release-notes-writer"
                  spellCheck={false}
                  autoComplete="off"
                />
              </Field>
            </div>
            <Field
              label="Description"
              htmlFor="skill-description"
              hint="Agents read this to decide when to load the skill. Say what it does and when to use it."
            >
              <Textarea
                id="skill-description"
                className="min-h-[72px]"
                value={draft.description}
                onChange={(event) => set("description", event.target.value)}
              />
            </Field>
            {draft.name.trim() && !draft.description.trim() ? (
              <Callout tone="warning" title="Add a description">
                Without one, agents cannot tell when this skill is useful, so they rarely load it.
              </Callout>
            ) : null}
            {renamed || describedDifferently ? (
              <Callout tone="info" title="Saving brings SKILL.md in line">
                {renamed ? (
                  <>
                    Its frontmatter names the skill <code className="font-mono text-xs">{stored?.name}</code>; saving
                    changes that to <code className="font-mono text-xs">{draft.slug}</code> so harnesses find it in its folder.{" "}
                  </>
                ) : null}
                {describedDifferently ? "Its frontmatter has a different description; saving replaces it with the one above." : null}
              </Callout>
            ) : null}
          </FormSection>

          <FormSection
            id="instructions"
            active={active}
            go={go}
            description="The body of SKILL.md: what the agent follows once it loads the skill. Name and description are added for you."
          >
            {sectionIssues("instructions")}
            <div className="flex flex-wrap items-center justify-between gap-2">
              <Segmented
                label="Instructions view"
                size="sm"
                value={bodyView}
                onChange={setBodyView}
                options={[
                  { value: "write", label: "Write" },
                  { value: "preview", label: "Preview" },
                ]}
              />
              <span className="text-xs text-muted-foreground">Markdown</span>
            </div>
            {bodyView === "write" ? (
              <Textarea
                id="skill-body"
                aria-label="Instructions"
                className="min-h-[320px] font-mono text-base md:text-xs leading-relaxed"
                spellCheck={false}
                value={draft.body}
                onChange={(event) => set("body", event.target.value)}
              />
            ) : (
              <div className="min-h-[320px] rounded-lg border bg-card p-4">
                {draft.body.trim() ? <Markdown>{draft.body}</Markdown> : <p className="text-sm text-muted-foreground">Nothing written yet.</p>}
              </div>
            )}
            <Disclosure
              title="Other frontmatter"
              summary={extraLines ? `${extraLines} ${extraLines === 1 ? "line" : "lines"}` : "Optional keys such as license or allowed-tools"}
            >
              <Field
                label="YAML lines"
                htmlFor="skill-extra"
                hint="Added under name and description, as written."
              >
                <Textarea
                  id="skill-extra"
                  className="min-h-[80px] font-mono text-base md:text-xs"
                  spellCheck={false}
                  value={draft.extra}
                  onChange={(event) => set("extra", event.target.value)}
                  placeholder={"license: MIT\nallowed-tools: Bash Read"}
                />
              </Field>
            </Disclosure>
          </FormSection>

          <FormSection
            id="files"
            active={active}
            go={go}
            description="Scripts, templates and references copied next to SKILL.md. Text files only."
          >
            {sectionIssues("files")}
            <div className="flex flex-wrap gap-2">
              <Button type="button" variant="outline" size="sm" onClick={() => addFile()}>
                <FilePlus2 /> New file
              </Button>
              <Button type="button" variant="outline" size="sm" onClick={() => uploadRef.current?.click()}>
                <Upload /> Upload files
              </Button>
              <input
                ref={uploadRef}
                type="file"
                multiple
                className="hidden"
                aria-label="Upload bundled files"
                onChange={(event) => void upload(event.target.files)}
              />
            </div>
            {draft.files.length ? (
              <div className="grid gap-4 md:grid-cols-[13rem_minmax(0,1fr)]">
                <ul className="max-h-80 space-y-0.5 overflow-y-auto rounded-lg border bg-card p-1" aria-label="Bundled files">
                  {draft.files.map((file, index) => {
                    const error = fileError(file);
                    return (
                      <li key={index}>
                        <button
                          type="button"
                          onClick={() => setSelectedFile(index)}
                          aria-current={index === selectedFile ? "true" : undefined}
                          className={cn(
                            "flex w-full items-center gap-2 rounded-md px-2 py-1.5 text-left transition-colors",
                            index === selectedFile ? "bg-accent text-accent-foreground" : "hover:bg-muted/60",
                          )}
                        >
                          <FileText className={cn("h-3.5 w-3.5 shrink-0", error ? "text-destructive" : "text-muted-foreground")} />
                          <span className={cn("min-w-0 flex-1 truncate font-mono text-xs", error && "text-destructive")}>
                            {file.path || "untitled"}
                          </span>
                          <span className="tabular shrink-0 text-xs text-muted-foreground">{sizeLabel(file.content)}</span>
                        </button>
                      </li>
                    );
                  })}
                </ul>
                {current ? (
                  <div className="min-w-0 space-y-3">
                    <div className="flex items-end gap-2">
                      <Field label="Path" htmlFor="skill-file-path" error={currentError} className="min-w-0 flex-1">
                        <Input
                          id="skill-file-path"
                          className="font-mono"
                          value={current.path}
                          onChange={(event) => setFile(selectedFile, { path: event.target.value })}
                          placeholder="scripts/run.sh"
                          spellCheck={false}
                          autoComplete="off"
                        />
                      </Field>
                      <Button
                        type="button"
                        variant="ghost"
                        size="icon"
                        aria-label={`Remove ${current.path || "this file"}`}
                        className={cn(currentError && "mb-7")}
                        onClick={() => removeFile(selectedFile)}
                      >
                        <Trash2 />
                      </Button>
                    </div>
                    <Textarea
                      aria-label={`Contents of ${current.path || "this file"}`}
                      className="min-h-[240px] font-mono text-base md:text-xs"
                      spellCheck={false}
                      value={current.content}
                      onChange={(event) => setFile(selectedFile, { content: event.target.value })}
                    />
                  </div>
                ) : null}
              </div>
            ) : (
              <p className="text-[13px] text-muted-foreground">
                No files. Most skills need none; add scripts under <code className="font-mono text-xs">scripts/</code> (
                {SCRIPT_EXTENSIONS.join(", ")}) and they are made executable.
              </p>
            )}
          </FormSection>

          {/* Kept mounted while hidden so its pending edits are still saved with the skill. */}
          <div className={cn(active !== "sharing" && "hidden")}>
            <Card id="section-sharing" className="scroll-mt-20" aria-labelledby="section-sharing-title">
              <CardHeader>
                <h2 id="section-sharing-title" className="text-[15px] font-semibold leading-6">
                  {SECTION_TITLES.sharing}
                </h2>
                <p className="text-[13px] text-muted-foreground">Who can see this skill and attach it to agents.</p>
              </CardHeader>
              <CardContent className="space-y-5">
                {sectionIssues("sharing")}
                <ResourceAccessFields ref={accessRef} resource="skills" resourceId={skill?.id} />
                <div className="flex items-center justify-between gap-3 border-t pt-4">
                  <Button type="button" variant="ghost" onClick={() => go("files")}>
                    <ChevronLeft /> {SECTION_TITLES.files}
                  </Button>
                </div>
              </CardContent>
            </Card>
          </div>
        </div>
      </div>

      {/* Phones and tablets: Save stays within thumb reach. */}
      <div className="fixed inset-x-0 bottom-14 z-30 border-t bg-background/95 p-3 backdrop-blur lg:hidden">
        <div className="flex items-center justify-between gap-3">
          <Button asChild variant="ghost">
            <Link href={cancelHref}>Cancel</Link>
          </Button>
          <Button onClick={() => void submit()} loading={submitting} size="lg">
            {submitLabel}
          </Button>
        </div>
      </div>
    </div>
  );
}
