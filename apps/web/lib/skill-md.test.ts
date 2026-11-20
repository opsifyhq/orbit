import assert from "node:assert/strict";
import { test } from "node:test";

import { composeSkill, filePathError, parseSkill, slugError, slugify, yamlString } from "./skill-md.ts";

test("parses name, description, extra keys and body", () => {
  const doc = parseSkill(`---
name: release-notes
description: Use when drafting release notes.
license: MIT
allowed-tools: Bash Read
---

# Release notes

Body text.
`);
  assert.equal(doc.name, "release-notes");
  assert.equal(doc.description, "Use when drafting release notes.");
  assert.deepEqual(doc.extra, ["license: MIT", "allowed-tools: Bash Read"]);
  assert.equal(doc.body, "# Release notes\n\nBody text.\n");
});

test("unfolds block, wrapped and quoted descriptions", () => {
  assert.equal(parseSkill("---\nname: a\ndescription: >-\n  Use when\n  triaging.\n---\nx").description, "Use when triaging.");
  assert.equal(parseSkill("---\nname: a\ndescription: Use when\n  triaging.\n---\nx").description, "Use when triaging.");
  assert.equal(parseSkill('---\nname: a\ndescription: "Use: when, triaging"\n---\nx').description, "Use: when, triaging");
  assert.equal(parseSkill("---\nname: a\ndescription: 'It''s fine'\n---\nx").description, "It's fine");
});

test("content without frontmatter is all body", () => {
  const doc = parseSkill("# Just markdown\n");
  assert.equal(doc.name, "");
  assert.equal(doc.body, "# Just markdown\n");
});

test("compose writes the slug and description and keeps other keys", () => {
  const original = "---\nname: example-skill\ndescription: placeholder\nlicense: MIT\n---\n\n# Body\n";
  const doc = parseSkill(original);
  const next = composeSkill({ slug: "release-notes", description: "Use when: drafting notes", body: doc.body, extra: doc.extra });
  assert.equal(next, '---\nname: release-notes\ndescription: "Use when: drafting notes"\nlicense: MIT\n---\n\n# Body\n');
  const again = parseSkill(next);
  assert.equal(again.name, "release-notes");
  assert.equal(again.description, "Use when: drafting notes");
  assert.deepEqual(again.extra, ["license: MIT"]);
  assert.equal(again.body, "# Body\n");
});

test("yaml strings are quoted only when needed", () => {
  assert.equal(yamlString("Use when drafting notes."), "Use when drafting notes.");
  assert.equal(yamlString("- starts with a dash"), '"- starts with a dash"');
  assert.equal(yamlString("yes"), '"yes"');
  assert.equal(yamlString("2025 roadmap"), '"2025 roadmap"');
  assert.equal(yamlString(""), '""');
  assert.equal(yamlString("a # comment"), '"a # comment"');
});

test("slugs and file paths follow the API rules", () => {
  assert.equal(slugify("Release Notes Writer!"), "release-notes-writer");
  assert.equal(slugify("Café déjà vu"), "cafe-deja-vu");
  assert.equal(slugError("release-notes"), null);
  assert.ok(slugError("-bad"));
  assert.ok(slugError("Bad"));
  assert.ok(slugError("a".repeat(64)));
  assert.equal(filePathError("scripts/run.sh"), null);
  assert.equal(filePathError("templates/notes.md"), null);
  assert.ok(filePathError("scripts/run.txt"));
  assert.ok(filePathError("/etc/passwd"));
  assert.ok(filePathError("../up"));
  assert.ok(filePathError("SKILL.md"));
});
