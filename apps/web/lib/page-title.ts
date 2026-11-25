/**
 * The page part of the document title. Create and edit pages get their own title
 * ("New agent", "Edit skill") so browser tabs and history can tell them apart from the
 * list they belong to; other pages use their section's label.
 */
const NOUNS: Record<string, string> = {
  agents: "agent",
  "agent-teams": "agent team",
  skills: "skill",
  connections: "connection",
  models: "model provider",
  outputs: "output",
  environments: "environment",
};

export function pageTitle(pathname: string, sectionLabel: string): string {
  const parts = pathname.split("/").filter(Boolean);
  const noun = NOUNS[parts[0] ?? ""];
  if (noun && parts.length === 2 && parts[1] === "new") return `New ${noun}`;
  if (noun && parts.length === 3 && parts[2] === "edit") return `Edit ${noun}`;
  return sectionLabel;
}
