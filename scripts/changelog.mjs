#!/usr/bin/env node
// Changelog fragments. Each pull request adds a file to changelog.d/ instead of editing
// CHANGELOG.md, so pull requests never conflict over the Unreleased section. A release
// collects the fragments into a version section.
//
//   node scripts/changelog.mjs check              validate every fragment (CI)
//   node scripts/changelog.mjs preview            print the Unreleased section with fragments
//   node scripts/changelog.mjs collect X.Y.Z [--date YYYY-MM-DD]
//                                                 write `## [X.Y.Z] - date` from Unreleased and
//                                                 the fragments, then delete the fragments
//
// Fragment file: changelog.d/<name>.<type>.md, where <type> is a Keep a Changelog section
// (added, changed, deprecated, removed, fixed, security) and <name> is usually the issue
// number ("457.fixed.md"). The content is one or more Markdown list items exactly as they
// should appear in CHANGELOG.md: each item starts with "- " and continuation lines are
// indented. Links are written relative to the repository root, as in CHANGELOG.md.

import { existsSync, readdirSync, readFileSync, unlinkSync, writeFileSync } from 'node:fs';
import path from 'node:path';
import { fileURLToPath } from 'node:url';

export const TYPES = ['added', 'changed', 'deprecated', 'removed', 'fixed', 'security'];
const HEADINGS = Object.fromEntries(TYPES.map((type) => [type, type[0].toUpperCase() + type.slice(1)]));
const NAME = /^[a-z0-9][a-z0-9._-]*\.(added|changed|deprecated|removed|fixed|security)\.md$/;
const VERSION = /^\d+\.\d+\.\d+(?:-[0-9A-Za-z.-]+)?$/;
const DATE = /^\d{4}-\d{2}-\d{2}$/;
const IGNORED = new Set(['README.md', '.gitkeep']);

/** Validate one fragment; returns a list of problems (empty when valid). */
export function fragmentProblems(name, text) {
  const problems = [];
  if (!NAME.test(name)) {
    problems.push(`name must be <name>.<type>.md with a lowercase name and type one of ${TYPES.join(', ')}`);
  }
  const lines = text.replace(/\s+$/, '').split('\n');
  if (!lines[0] || !lines[0].startsWith('- ')) {
    problems.push('content must start with a Markdown list item ("- ")');
  }
  lines.forEach((line, index) => {
    if (/^#/.test(line)) problems.push(`line ${index + 1}: headings are added by the release; remove it`);
    else if (line && !line.startsWith('- ') && !/^\s/.test(line)) {
      problems.push(`line ${index + 1}: start a new item with "- " or indent a continuation line`);
    }
  });
  return problems;
}

/** Read and validate every fragment in dir, sorted by name (numbers in natural order). */
export function readFragments(dir) {
  if (!existsSync(dir)) return { fragments: [], problems: [] };
  const fragments = [];
  const problems = [];
  const names = readdirSync(dir)
    .filter((name) => !IGNORED.has(name))
    .sort((a, b) => a.localeCompare(b, 'en', { numeric: true }));
  for (const name of names) {
    const text = readFileSync(path.join(dir, name), 'utf8');
    const found = fragmentProblems(name, text);
    if (found.length) problems.push(...found.map((problem) => `changelog.d/${name}: ${problem}`));
    else fragments.push({ name, type: name.match(NAME)[1], text: text.replace(/\s+$/, '') });
  }
  return { fragments, problems };
}

/** Split CHANGELOG.md into the text before Unreleased, its subsections, and the rest. */
export function parseChangelog(source) {
  const lines = source.split('\n');
  const start = lines.findIndex((line) => /^## \[Unreleased\]/.test(line));
  if (start < 0) throw new Error('CHANGELOG.md has no "## [Unreleased]" section');
  let end = lines.findIndex((line, index) => index > start && /^## \[/.test(line));
  if (end < 0) end = lines.length;
  const sections = [];
  const preamble = [];
  let current = null;
  for (const line of lines.slice(start + 1, end)) {
    const heading = line.match(/^### (.+)$/);
    if (heading) {
      current = { title: heading[1].trim(), body: [] };
      sections.push(current);
    } else if (current) current.body.push(line);
    else preamble.push(line);
  }
  if (preamble.some((line) => line.trim())) {
    throw new Error('text under "## [Unreleased]" must sit below a "### <Type>" heading');
  }
  return {
    head: lines.slice(0, start + 1).join('\n'),
    sections: sections.map((section) => ({ title: section.title, body: trimBlank(section.body).join('\n') })),
    tail: lines.slice(end).join('\n'),
  };
}

function trimBlank(lines) {
  let first = 0;
  let last = lines.length;
  while (first < last && !lines[first].trim()) first += 1;
  while (last > first && !lines[last - 1].trim()) last -= 1;
  return lines.slice(first, last);
}

/** Merge Unreleased subsections and fragments into ordered `### Type` blocks. */
export function mergeSections(sections, fragments) {
  const bodies = new Map();
  for (const section of sections) {
    if (!section.body) continue;
    const key = section.title.toLowerCase();
    bodies.set(key, [...(bodies.get(key) ?? []), section.body]);
  }
  const blocks = [];
  for (const type of TYPES) {
    const entries = [...(bodies.get(type) ?? [])];
    for (const fragment of fragments) if (fragment.type === type) entries.push(fragment.text);
    if (entries.length) blocks.push({ title: HEADINGS[type], entries });
  }
  // Headings Keep a Changelog does not define (e.g. "Upgrade") keep their order, after the known ones.
  for (const section of sections) {
    const key = section.title.toLowerCase();
    if (TYPES.includes(key) || !section.body) continue;
    const existing = blocks.find((block) => block.title.toLowerCase() === key);
    if (existing) existing.entries.push(section.body);
    else blocks.push({ title: section.title, entries: [section.body] });
  }
  return blocks.map((block) => `### ${block.title}\n\n${block.entries.join('\n')}`).join('\n\n');
}

/** Return the new CHANGELOG.md text for a release. */
export function collect(source, fragments, version, date) {
  if (!VERSION.test(version)) throw new Error(`"${version}" is not a version (X.Y.Z)`);
  if (!DATE.test(date)) throw new Error(`"${date}" is not a date (YYYY-MM-DD)`);
  if (source.includes(`## [${version}]`)) throw new Error(`CHANGELOG.md already has a ${version} section`);
  const { head, sections, tail } = parseChangelog(source);
  const body = mergeSections(sections, fragments);
  if (!body) throw new Error('nothing to release: Unreleased is empty and there are no fragments');
  return `${head}\n\n## [${version}] - ${date}\n\n${body}\n${tail ? `\n${tail}` : ''}`;
}

/** The Unreleased section as it would read with the fragments merged in. */
export function preview(source, fragments) {
  const { sections } = parseChangelog(source);
  return `## [Unreleased]\n\n${mergeSections(sections, fragments)}\n`;
}

function main(argv) {
  const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..');
  const dir = path.join(root, 'changelog.d');
  const changelogPath = path.join(root, 'CHANGELOG.md');
  const [command, ...rest] = argv;
  const { fragments, problems } = readFragments(dir);
  if (problems.length) {
    for (const problem of problems) console.error(problem);
    console.error(`\n${problems.length} changelog fragment problem(s); see changelog.d/README.md`);
    return 1;
  }
  const source = readFileSync(changelogPath, 'utf8');
  if (command === 'check') {
    parseChangelog(source);
    console.log(`Changelog check passed: ${fragments.length} fragment(s).`);
    return 0;
  }
  if (command === 'preview') {
    process.stdout.write(preview(source, fragments));
    return 0;
  }
  if (command === 'collect') {
    const version = rest[0];
    const dateFlag = rest.indexOf('--date');
    const date = dateFlag >= 0 ? rest[dateFlag + 1] : new Date().toISOString().slice(0, 10);
    if (!version) throw new Error('usage: node scripts/changelog.mjs collect X.Y.Z [--date YYYY-MM-DD]');
    writeFileSync(changelogPath, collect(source, fragments, version, date));
    for (const fragment of fragments) unlinkSync(path.join(dir, fragment.name));
    console.log(`CHANGELOG.md: wrote ${version} from ${fragments.length} fragment(s) and Unreleased.`);
    return 0;
  }
  console.error('usage: node scripts/changelog.mjs check | preview | collect X.Y.Z [--date YYYY-MM-DD]');
  return 2;
}

if (process.argv[1] && path.resolve(process.argv[1]) === fileURLToPath(import.meta.url)) {
  try {
    process.exitCode = main(process.argv.slice(2));
  } catch (error) {
    console.error(error.message);
    process.exitCode = 1;
  }
}
