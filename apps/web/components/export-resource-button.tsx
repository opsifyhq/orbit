"use client";

import { toast } from "sonner";

import { downloadExport, type ExportFormat } from "@/lib/portable";
import { errorMessage } from "@orbit/api-client";

/**
 * Download a secret-free, portable export of a shareable resource (YAML by default;
 * `markdown` gives a SKILL.md for skills). Credentials are never included: the file
 * says so, and they are set again after importing.
 */
export async function exportResource(
  resource: string,
  resourceId: string,
  name: string,
  format: ExportFormat = "yaml",
) {
  try {
    await downloadExport(resource, resourceId, name, format);
    toast.success(format === "markdown" ? "SKILL.md downloaded" : "Export downloaded");
  } catch (error) {
    toast.error(`Export failed: ${errorMessage(error)}`);
  }
}
