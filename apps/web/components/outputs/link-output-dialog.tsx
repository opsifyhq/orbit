"use client";

import { useState } from "react";
import { useQueryClient } from "@tanstack/react-query";
import { toast } from "sonner";

import { Button } from "@/components/ui/button";
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
  DialogTrigger,
} from "@/components/ui/dialog";
import { Field } from "@/components/ui/field";
import { Select } from "@/components/ui/select";
import { kindLabel } from "@/lib/output-meta";
import { retargetOutput, updateOutput, type OutputView } from "@/lib/outputs";
import { errorMessage } from "@orbit/api-client";

/** Attach an existing output to the agent/team whose page opened this dialog. */
export function LinkOutputDialog({
  outputs,
  scope,
  id,
}: {
  outputs: OutputView[];
  scope: "agent" | "team";
  id: string;
}) {
  const queryClient = useQueryClient();
  const [open, setOpen] = useState(false);
  const [selected, setSelected] = useState("");
  const [saving, setSaving] = useState(false);
  const already = (output: OutputView) =>
    scope === "agent"
      ? (output.agentIds ?? (output.agentId ? [output.agentId] : [])).includes(id)
      : output.teamId === id;
  const candidates = outputs.filter((output) => !already(output));

  async function link() {
    const output = outputs.find((item) => item.id === selected);
    if (!output) return;
    setSaving(true);
    try {
      await updateOutput(output.id, retargetOutput(output, scope, id));
      toast.success(`${output.name} linked`);
      setOpen(false);
      setSelected("");
      void queryClient.invalidateQueries({ queryKey: ["outputs"] });
      void queryClient.invalidateQueries({ queryKey: ["sharing"] });
    } catch (error) {
      toast.error(errorMessage(error));
    } finally {
      setSaving(false);
    }
  }

  return (
    <Dialog open={open} onOpenChange={setOpen}>
      <DialogTrigger asChild>
        <button type="button" className="text-primary underline-offset-4 hover:underline">
          Link existing
        </button>
      </DialogTrigger>
      <DialogContent>
        <DialogHeader>
          <DialogTitle>Link an existing output</DialogTitle>
          <DialogDescription>
            Select an existing notification. Its destination and message stay unchanged.
          </DialogDescription>
        </DialogHeader>
        <Field label="Output" htmlFor="link-output">
          <Select id="link-output" value={selected} onChange={(event) => setSelected(event.target.value)}>
            <option value="">Choose an output</option>
            {candidates.map((output) => (
              <option key={output.id} value={output.id}>
                {output.name} ({kindLabel(output.kind)})
              </option>
            ))}
          </Select>
        </Field>
        {!candidates.length ? (
          <p className="text-[13px] text-muted-foreground">Every output is already linked here.</p>
        ) : null}
        <DialogFooter>
          <Button variant="outline" onClick={() => setOpen(false)}>
            Cancel
          </Button>
          <Button onClick={() => void link()} loading={saving} disabled={!selected}>
            Link
          </Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  );
}
