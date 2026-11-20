"use client";

import { useRef, useState } from "react";
import { useQueryClient } from "@tanstack/react-query";
import { FileUp, Upload } from "lucide-react";
import { toast } from "sonner";

import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { Callout } from "@/components/ui/callout";
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog";
import { Field } from "@/components/ui/field";
import { Segmented } from "@/components/ui/segmented";
import { Textarea } from "@/components/ui/textarea";
import {
  PortableError,
  runImport,
  type ImportItemReport,
  type ImportMode,
  type ImportReport,
} from "@/lib/portable";
import { errorMessage } from "@orbit/api-client";

const MODES: ReadonlyArray<{ value: ImportMode; label: string }> = [
  { value: "create", label: "Create only" },
  { value: "upsert", label: "Create or update" },
  { value: "update", label: "Update only" },
];

const MODE_HINT: Record<ImportMode, string> = {
  create: "Nothing is overwritten. A name that already exists is reported as a conflict.",
  upsert: "Existing resources with the same name are updated; new ones are created.",
  update: "Only resources that already exist are changed; nothing is created.",
};

const KIND_LABEL: Record<string, string> = {
  Agent: "Agent",
  Skill: "Skill",
  Connection: "Connection",
  Provider: "Model provider",
  WorkerProfile: "Worker profile",
  AgentTeam: "Agent team",
  Trigger: "Trigger",
  ToolPolicy: "Tool policy",
};

function statusBadge(item: ImportItemReport) {
  switch (item.status) {
    case "ok":
      return (
        <Badge variant="success">
          {item.action === "update" ? "Will update" : "Will create"}
        </Badge>
      );
    case "conflict":
      return <Badge variant="warning">Already exists</Badge>;
    case "unresolved_references":
      return <Badge variant="warning">Missing references</Badge>;
    default:
      return <Badge variant="destructive">Invalid</Badge>;
  }
}

function ItemRow({ item }: { item: ImportItemReport }) {
  const problems = item.issues.length + item.unresolvedReferences.length;
  return (
    <li className="space-y-2 border-b px-4 py-3 last:border-b-0">
      <div className="flex flex-wrap items-center gap-2">
        <Badge variant="outline">{KIND_LABEL[item.kind] ?? item.kind}</Badge>
        <span className="min-w-0 break-words text-sm font-medium">{item.name}</span>
        <span className="ml-auto">{statusBadge(item)}</span>
      </div>
      {problems ? (
        <ul className="space-y-1 text-[13px]">
          {item.unresolvedReferences.map((ref) => (
            <li key={`${ref.kind}:${ref.reference}:${ref.path}`} className="text-warning">
              Missing {ref.kind.replace("_", " ")} <span className="font-mono break-all">{ref.reference}</span>
              <span className="text-muted-foreground"> (in {ref.path}). Import or create it first.</span>
            </li>
          ))}
          {item.issues.map((issue, index) => (
            <li key={`${issue.path}:${index}`} className={item.status === "conflict" ? "text-warning" : "text-destructive"}>
              {issue.path ? <span className="font-mono break-all">{issue.path}: </span> : null}
              {issue.message}
            </li>
          ))}
        </ul>
      ) : null}
      {item.warnings.length ? (
        <ul className="space-y-1 text-[13px] text-muted-foreground">
          {item.warnings.map((warning, index) => (
            <li key={index}>{warning}</li>
          ))}
        </ul>
      ) : null}
    </li>
  );
}

/**
 * Import portable ORBIT files: YAML/JSON documents, a SKILL.md or a multi-document
 * bundle. The file is validated first (nothing is written) and the report shows what
 * would be created or updated, what already exists and which references are missing.
 */
export function ImportPortableButton({
  label = "Import",
  variant = "outline",
}: {
  label?: string;
  variant?: "outline" | "default" | "ghost";
}) {
  const queryClient = useQueryClient();
  const fileInput = useRef<HTMLInputElement>(null);
  const [open, setOpen] = useState(false);
  const [content, setContent] = useState("");
  const [filename, setFilename] = useState("");
  const [mode, setMode] = useState<ImportMode>("create");
  const [report, setReport] = useState<ImportReport | null>(null);
  const [failure, setFailure] = useState<PortableError | Error | null>(null);
  const [busy, setBusy] = useState<"check" | "import" | null>(null);

  function reset() {
    setContent("");
    setFilename("");
    setMode("create");
    setReport(null);
    setFailure(null);
    setBusy(null);
  }

  async function check(nextContent = content, nextFilename = filename, nextMode = mode) {
    if (!nextContent.trim()) return;
    setBusy("check");
    setFailure(null);
    try {
      setReport(await runImport(nextContent, nextFilename, nextMode, true));
    } catch (error) {
      setReport(null);
      setFailure(error instanceof Error ? error : new Error(errorMessage(error)));
    } finally {
      setBusy(null);
    }
  }

  async function load(file: File) {
    const text = await file.text();
    setContent(text);
    setFilename(file.name);
    await check(text, file.name, mode);
  }

  async function apply() {
    setBusy("import");
    setFailure(null);
    try {
      const result = await runImport(content, filename, mode, false);
      await queryClient.invalidateQueries();
      const created = result.items.filter((item) => item.action === "create").length;
      const updated = result.items.filter((item) => item.action === "update").length;
      const needsCredentials = result.items.filter((item) => item.credentialsRequired);
      const summary = [created ? `${created} created` : "", updated ? `${updated} updated` : ""]
        .filter(Boolean)
        .join(", ");
      if (needsCredentials.length) {
        toast.warning(`Imported: ${summary}`, {
          description: `Set credentials for ${needsCredentials.map((item) => item.name).join(", ")}; files never carry them.`,
        });
      } else {
        toast.success(`Imported: ${summary}`);
      }
      setOpen(false);
      reset();
    } catch (error) {
      // The server re-validates: show its field errors instead of a bare toast or a stale
      // "ready to import" report from the dry run.
      setReport(null);
      setFailure(error instanceof Error ? error : new Error(errorMessage(error)));
    } finally {
      setBusy(null);
    }
  }

  const toCreate = report?.items.filter((item) => item.action === "create").length ?? 0;
  const toUpdate = report?.items.filter((item) => item.action === "update").length ?? 0;
  const problems = report ? report.items.filter((item) => item.status !== "ok").length : 0;
  const hasConflict = report?.items.some((item) => item.status === "conflict") ?? false;

  return (
    <>
      <Button
        variant={variant}
        onClick={() => {
          reset();
          setOpen(true);
        }}
      >
        <Upload /> {label}
      </Button>
      <Dialog
        open={open}
        onOpenChange={(next) => {
          if (busy === "import") return;
          setOpen(next);
          if (!next) reset();
        }}
      >
        <DialogContent className="max-w-2xl">
          <DialogHeader>
            <DialogTitle>Import from file</DialogTitle>
            <DialogDescription>
              Agents, skills, connections, model providers, worker profiles and agent teams, as YAML or
              JSON, or a SKILL.md. Several documents in one file are imported together. Files never
              contain credentials: set them on the resource afterwards.
            </DialogDescription>
          </DialogHeader>

          <input
            ref={fileInput}
            type="file"
            accept=".yaml,.yml,.json,.md,.markdown,text/yaml,application/json,text/markdown"
            className="hidden"
            onChange={(event) => {
              const file = event.target.files?.[0];
              event.target.value = "";
              if (file) void load(file);
            }}
          />

          <div className="min-w-0 space-y-4">
            <div className="flex flex-wrap items-center gap-3">
              <Button type="button" variant="outline" onClick={() => fileInput.current?.click()}>
                <FileUp /> Choose file
              </Button>
              <span className="min-w-0 truncate text-[13px] text-muted-foreground">
                {filename || "or paste the contents below"}
              </span>
            </div>

            <Field label="Contents" htmlFor="import-content">
              <Textarea
                id="import-content"
                className="min-h-32 font-mono text-base md:text-[13px]"
                spellCheck={false}
                placeholder={"apiVersion: orbit.io/v1\nkind: Skill\nmetadata:\n  name: ..."}
                value={content}
                onChange={(event) => {
                  setContent(event.target.value);
                  setReport(null);
                  setFailure(null);
                }}
              />
            </Field>

            <Field label="If a name already exists" hint={MODE_HINT[mode]}>
              <Segmented
                label="Import mode"
                value={mode}
                options={MODES}
                onChange={(next) => {
                  setMode(next);
                  if (content.trim()) void check(content, filename, next);
                }}
              />
            </Field>

            {failure ? (
              <Callout tone="danger" title={failure.message}>
                {failure instanceof PortableError && failure.details.length ? (
                  <ul className="mt-1 max-h-40 space-y-0.5 overflow-y-auto">
                    {failure.details.map((detail, index) => (
                      <li key={index} className="break-words">
                        <span className="font-mono">{detail.path}</span>: {detail.message}
                      </li>
                    ))}
                  </ul>
                ) : null}
              </Callout>
            ) : null}

            {report ? (
              <div className="space-y-3">
                {report.valid ? (
                  <Callout tone="info" title="Ready to import">
                    {[toCreate ? `${toCreate} to create` : "", toUpdate ? `${toUpdate} to update` : ""]
                      .filter(Boolean)
                      .join(", ")}
                    . Nothing has been written yet.
                  </Callout>
                ) : (
                  <Callout tone="warning" title={`${problems} of ${report.items.length} need attention`}>
                    Nothing is imported until every document is valid.
                    {hasConflict && mode === "create" ? " Choose “Create or update” to change existing resources." : ""}
                  </Callout>
                )}
                <ul className="overflow-hidden rounded-lg border">
                  {report.items.map((item, index) => (
                    <ItemRow key={`${item.kind}:${item.name}:${index}`} item={item} />
                  ))}
                </ul>
              </div>
            ) : null}
          </div>

          <DialogFooter>
            <Button type="button" variant="outline" disabled={busy === "import"} onClick={() => setOpen(false)}>
              Cancel
            </Button>
            {!report ? (
              <Button type="button" loading={busy === "check"} disabled={!content.trim()} onClick={() => void check()}>
                Check file
              </Button>
            ) : (
              <Button
                type="button"
                loading={busy === "import"}
                disabled={!report.valid}
                onClick={() => void apply()}
              >
                Import
              </Button>
            )}
          </DialogFooter>
        </DialogContent>
      </Dialog>
    </>
  );
}
