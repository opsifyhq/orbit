/**
 * SKILL.md is a markdown body under a YAML frontmatter block. Harnesses read the
 * frontmatter `name` and `description` to decide when to load a skill, and the
 * Agent Skills format expects `name` to match the skill's folder (its slug). The
 * skill form edits name and description as fields and writes them into the
 * frontmatter here, so what agents read always matches what the form shows. Other
 * frontmatter keys (license, allowed-tools, metadata, …) are kept as written.
 */

export interface SkillDocument {
  /** Frontmatter lines other than `name` and `description`, verbatim. */
  extra: string[];
  /** `description` from the frontmatter, unfolded to one line; empty when missing. */
  description: string;
  /** `name` from the frontmatter; empty when missing. */
  name: string;
  /** The markdown after the frontmatter. */
  body: string;
}

const OPEN = /^---[ \t]*\r?\n/;
const CLOSE = /^---[ \t]*$/m;

function unquote(value: string): string {
  const text = value.trim();
  if (text.length >= 2 && text.startsWith('"') && text.endsWith('"')) {
    try {
      return JSON.parse(text) as string;
    } catch {
      return text.slice(1, -1);
    }
  }
  if (text.length >= 2 && text.startsWith("'") && text.endsWith("'")) {
    return text.slice(1, -1).replace(/''/g, "'");
  }
  return text;
}

const isTopLevelKey = (line: string) => /^[A-Za-z0-9_-]+\s*:/.test(line);

/** Split a SKILL.md into its managed fields, other frontmatter lines and body. */
export function parseSkill(content: string): SkillDocument {
  const text = content.replace(/^﻿/, "");
  const open = OPEN.exec(text);
  if (!open) return { extra: [], name: "", description: "", body: text };
  const rest = text.slice(open[0].length);
  const close = CLOSE.exec(rest);
  if (!close) return { extra: [], name: "", description: "", body: text };

  const lines = rest.slice(0, close.index).replace(/\r?\n$/, "").split(/\r?\n/);
  const body = rest.slice(close.index + close[0].length).replace(/^\r?\n/, "").replace(/^\r?\n/, "");
  const extra: string[] = [];
  let name = "";
  let description = "";

  for (let i = 0; i < lines.length; i++) {
    const line = lines[i];
    const match = /^(name|description)\s*:\s?(.*)$/.exec(line);
    if (!match) {
      if (line.trim() !== "" || extra.length) extra.push(line);
      continue;
    }
    // A value may continue on indented lines (block scalars or plain wrapped text).
    const continuation: string[] = [];
    while (i + 1 < lines.length && !isTopLevelKey(lines[i + 1]) && /^\s+\S|^\s*$/.test(lines[i + 1])) {
      continuation.push(lines[++i].trim());
    }
    const head = match[2].trim();
    const block = /^[>|][+-]?$/.test(head);
    const parts = block ? continuation : [head, ...continuation];
    const value = block && head.startsWith("|")
      ? parts.join("\n").trim()
      : unquote(parts.filter(Boolean).join(" "));
    if (match[1] === "name") name = value;
    else description = value;
  }
  while (extra.length && extra[extra.length - 1].trim() === "") extra.pop();
  return { extra, name, description, body };
}

/** A YAML scalar for one line of text, quoted only when plain style would misread it. */
export function yamlString(value: string): string {
  const text = value.replace(/\s+/g, " ").trim();
  if (text === "") return '""';
  const plain =
    !/^[\s\-?:,\[\]{}#&*!|>'"%@`]/.test(text) &&
    !/(:\s|\s#)/.test(text) &&
    !/:$/.test(text) &&
    !/^(true|false|yes|no|null|~|on|off)$/i.test(text) &&
    !/^[-+]?(\d|\.\d)/.test(text);
  return plain ? text : JSON.stringify(text);
}

/** Write the SKILL.md for these fields, keeping other frontmatter lines as they were. */
export function composeSkill({
  slug,
  description,
  body,
  extra = [],
}: {
  slug: string;
  description: string;
  body: string;
  extra?: string[];
}): string {
  const head = [`name: ${yamlString(slug)}`, `description: ${yamlString(description)}`, ...extra];
  const text = body.replace(/^\s*\n/, "");
  return `---\n${head.join("\n")}\n---\n\n${text}${text.endsWith("\n") ? "" : "\n"}`;
}

/** Turn a display name into a slug: lowercase letters, digits and hyphens. */
export function slugify(name: string): string {
  return name
    .toLowerCase()
    .normalize("NFKD")
    .replace(/[̀-ͯ]/g, "")
    .replace(/[^a-z0-9]+/g, "-")
    .replace(/^-+|-+$/g, "")
    .slice(0, 63);
}

/** Same rule as the API: lowercase letters, digits, - or _, starting with a letter or digit. */
export function slugError(slug: string): string | null {
  if (!slug) return "Enter a slug.";
  if (slug.length > 63) return "Keep the slug to 63 characters.";
  if (!/^[a-z0-9][a-z0-9_-]*$/.test(slug)) return "Use lowercase letters, digits, - or _, starting with a letter or digit.";
  return null;
}

export const SCRIPT_EXTENSIONS = [".sh", ".bash", ".py", ".js", ".ts", ".rb", ".pl", ".go"];
export const MAX_FILES = 50;

/** Why a bundled file path would be refused, or null. Mirrors the API's checks. */
export function filePathError(path: string): string | null {
  if (!path.trim()) return "Enter a path.";
  if (path !== path.trim()) return "Remove the spaces around the path.";
  if (path.startsWith("/")) return "Use a path relative to the skill folder.";
  if (path.includes("..")) return "A path cannot contain “..”.";
  if (path.includes("\\")) return "Use / between folders.";
  if (path.length > 200) return "Keep the path to 200 characters.";
  if (path === "SKILL.md") return "SKILL.md is the skill itself; pick another name.";
  const script = path.startsWith("scripts/") || path.includes("/scripts/");
  if (script && !SCRIPT_EXTENSIONS.some((ext) => path.endsWith(ext))) {
    return `Scripts must end in ${SCRIPT_EXTENSIONS.join(", ")}.`;
  }
  return null;
}
