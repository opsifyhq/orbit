#!/usr/bin/env node
// Lightweight documentation consistency check. No dependencies, no network, no ordering
// surprises: files and problems are sorted before anything is printed.
//
// Checks:
//   1. internal Markdown links (including images, reference definitions and anchors)
//   2. docs/navigation.yaml (every served page listed once; every listed path exists)
//   3. repository paths written in inline code, where they can be resolved unambiguously
//
// This complements the Rust tests in crates/orbit-api/src/docs.rs: those validate the
// embedded handbook, while this script also covers root docs, CHANGELOG and planning
// drafts, and can run before the Rust toolchain is available.

import { readdirSync, readFileSync, statSync } from 'node:fs';
import path from 'node:path';
import { fileURLToPath } from 'node:url';

const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..');
const SKIP_DIRS = new Set(['.git', 'node_modules', 'target', 'dist', '.next', '.turbo', 'coverage']);
const SOURCE_EXT = /\.(?:rs|ts|tsx|mjs|js|jsx|sh|py|yaml|yml|toml|json|md|sql|svg|png|css|lock)$/;
const ROOT_DIRS = new Set(['apps', 'crates', 'services', 'scripts', 'deploy', 'packages', 'docs', '.github']);
const ROOT_FILES = new Set([
  'AGENTS.md', 'README.md', 'CHANGELOG.md', 'CONTRIBUTING.md', 'LICENSE',
  'Cargo.toml', 'Cargo.lock', 'package.json', 'pnpm-workspace.yaml', 'rust-toolchain.toml', 'Makefile',
]);
// Bare paths in the design-system guide are relative to apps/web or its components dir.
const WEB_BASES = ['apps/web', 'apps/web/components', 'apps/web/lib'];
const WEB_PREFIXES = new Set(['app', 'components', 'lib', 'ui']);

const problems = [];

function report(file, line, message) {
  problems.push(`${file}${line ? `:${line}` : ''}: ${message}`);
}

function walk(dir) {
  const out = [];
  for (const entry of readdirSync(dir, { withFileTypes: true })) {
    if (SKIP_DIRS.has(entry.name) || (entry.name.startsWith('.') && entry.name !== '.github')) continue;
    const full = path.join(dir, entry.name);
    if (entry.isDirectory()) out.push(...walk(full));
    else if (entry.isFile()) out.push(full);
  }
  return out;
}

const allFiles = walk(root).map((file) => path.relative(root, file)).sort();
const markdownFiles = allFiles.filter((file) => file.endsWith('.md'));
const fileSet = new Set(allFiles);

function withoutFences(source) {
  const out = [];
  let fence = null;
  for (const line of source.split('\n')) {
    const marker = line.trimStart().match(/^(```+|~~~+)/);
    if (marker) {
      const token = marker[1].slice(0, 3);
      if (!fence) fence = token;
      else if (fence === token) fence = null;
      out.push('');
      continue;
    }
    out.push(fence ? '' : line);
  }
  return out;
}

/** GitHub-style heading ids, with the same de-duplication as the embedded docs. */
function anchors(lines) {
  const ids = [];
  const seen = new Map();
  let inFence = false;
  for (const line of lines) {
    const trimmed = line.trimStart();
    if (trimmed.startsWith('```') || trimmed.startsWith('~~~')) {
      inFence = !inFence;
      continue;
    }
    if (inFence) continue;
    const heading = line.match(/^\s{0,3}#{1,6}\s+(.+?)\s*#*\s*$/);
    if (!heading) continue;
    const text = heading[1]
      .replace(/!?\[([^\]]*)\]\([^)]*\)/g, '$1')
      .replace(/[`*_~]/g, '');
    const base = [...text.trim()].map((char) => {
      if (/\s/u.test(char)) return '-';
      if (/[\p{L}\p{N}\-_]/u.test(char)) return char.toLowerCase();
      return '';
    }).join('');
    const count = seen.get(base) ?? 0;
    seen.set(base, count + 1);
    ids.push(count === 0 ? base : `${base}-${count}`);
  }
  return new Set(ids);
}

const anchorsByFile = new Map();
const proseByFile = new Map();
const sourceByFile = new Map();
for (const file of markdownFiles) {
  const source = readFileSync(path.join(root, file), 'utf8');
  const unfenced = withoutFences(source);
  const lines = unfenced.map((line) =>
    line.replace(/`[^`]*`/g, (match) => ' '.repeat(match.length)));
  proseByFile.set(file, lines);
  sourceByFile.set(file, source.split('\n'));
  anchorsByFile.set(file, anchors(unfenced));
}

// ------------------------------------------------------------------ links

function decode(target) {
  try {
    return decodeURIComponent(target);
  } catch {
    return target;
  }
}

function isExternal(target) {
  return /^(?:[a-z][a-z\d+.-]*:|\/\/|\/|#|mailto:)/i.test(target);
}

function exists(fromFile, target) {
  const resolved = path.normalize(path.join(path.dirname(fromFile), target));
  if (fileSet.has(resolved)) return { resolved, isMarkdown: resolved.endsWith('.md') };
  if (!resolved.includes('..') && fileSet.has(path.join(resolved, 'README.md'))) {
    return { resolved: path.join(resolved, 'README.md'), isMarkdown: true };
  }
  return null;
}

let linkCount = 0;
for (const file of markdownFiles) {
  for (const [index, line] of proseByFile.get(file).entries()) {
    // A line may contain more than one link; inline code was blanked above.
    for (const match of line.matchAll(/(?:!?\[[^\]]*\]\(\s*)(<?)([^)\s>]+)>?/g)) {
      checkLink(file, index + 1, match[2]);
    }
    const definition = line.match(/^\s*\[[^\]]+\]:\s*(\S+)/);
    if (definition) checkLink(file, index + 1, definition[1]);
  }
}

function checkLink(file, line, raw) {
  if (isExternal(raw)) return;
  linkCount += 1;
  const [before, ...rest] = decode(raw).split('#');
  const anchor = rest.join('#');
  const target = before.replace(/\?.*$/, '');
  if (!target) return; // #anchor in the current page
  const found = exists(file, target);
  if (!found) {
    report(file, line, `broken link \`${raw}\` (no such file)`);
    return;
  }
  if (anchor && found.isMarkdown && !anchorsByFile.get(found.resolved)?.has(anchor)) {
    report(file, line, `broken link \`${raw}\` (no heading \`#${anchor}\` in ${found.resolved})`);
  }
}

// ------------------------------------------------------------- navigation

function unquote(value) {
  return value.trim().replace(/^["']|["']$/g, '');
}

const navPath = 'docs/navigation.yaml';
const navLines = readFileSync(path.join(root, navPath), 'utf8').split('\n');
const sections = new Set();
const listed = [];
let section = null;
for (const [index, line] of navLines.entries()) {
  const id = line.match(/^\s*- id:\s*(\S+)/);
  if (id) {
    section = unquote(id[1]);
    if (sections.has(section)) report(navPath, index + 1, `duplicate section \`${section}\``);
    sections.add(section);
    continue;
  }
  const page = line.match(/^\s*- path:\s*(\S+)/);
  if (!page) continue;
  if (!section) {
    report(navPath, index + 1, 'page listed outside a section');
    continue;
  }
  listed.push({ section, path: unquote(page[1]), line: index + 1 });
}

const listedPaths = new Set();
for (const entry of listed) {
  if (listedPaths.has(entry.path)) report(navPath, entry.line, `\`${entry.path}\` is listed twice`);
  listedPaths.add(entry.path);
  if (entry.path.split('/').includes('..') || !entry.path.endsWith('.md')) {
    report(navPath, entry.line, `invalid page path \`${entry.path}\``);
  } else if (!fileSet.has(`docs/${entry.path}`)) {
    report(navPath, entry.line, `\`${entry.path}\` does not exist`);
  }
}
const servedPages = markdownFiles
  .filter((file) => file.startsWith('docs/') && !file.startsWith('docs/planning/'))
  .map((file) => file.slice('docs/'.length));
for (const page of servedPages.sort()) {
  if (!listedPaths.has(page)) report(navPath, 0, `\`${page}\` is missing from the sidebar`);
}

// ----------------------------------------------------------- code paths

function expandBraces(token) {
  const match = token.match(/\{([^{}]+)\}/);
  if (!match) return [token];
  return match[1].split(',').flatMap((choice) =>
    expandBraces(token.replace(match[0], choice)));
}

function resolveCodePath(token) {
  const candidates = [];
  const first = token.split('/')[0];
  if (WEB_PREFIXES.has(first)) {
    for (const base of WEB_BASES) candidates.push(path.join(base, token));
  } else if (ROOT_DIRS.has(first)) {
    candidates.push(token);
  } else if (ROOT_FILES.has(token)) {
    candidates.push(token);
  }
  return candidates.find((candidate) => fileSet.has(candidate) || statSafe(path.join(root, candidate)));
}

function statSafe(file) {
  try {
    return statSync(file).isDirectory();
  } catch {
    return false;
  }
}

let codePathCount = 0;
for (const file of markdownFiles) {
  for (const [index, line] of proseByFile.get(file).entries()) {
    // Unlike links, code spans are meaningful here: read the original line.
    for (const match of sourceByFile.get(file)[index].matchAll(/`([^`\n]+)`/g)) {
      let token = match[1].trim().replace(/[.,;:]+$/, '').replace(/\/$/, '');
      if (/[<>*?|="\s]/.test(token) || token.includes('://')) continue;
      if (!ROOT_DIRS.has(token.split('/')[0]) && !ROOT_FILES.has(token) && !WEB_PREFIXES.has(token.split('/')[0])) continue;
      for (const expanded of expandBraces(token)) {
        if (!SOURCE_EXT.test(expanded) && !ROOT_DIRS.has(expanded) && !ROOT_FILES.has(expanded)) continue;
        codePathCount += 1;
        if (!resolveCodePath(expanded)) report(file, index + 1, `unknown repository path \`${expanded}\``);
      }
    }
  }
}

// ------------------------------------------------------------------ result

problems.sort();
if (problems.length) {
  console.error(`Documentation check failed (${problems.length} problem${problems.length === 1 ? '' : 's'}):`);
  for (const problem of problems) console.error(`  ${problem}`);
  process.exit(1);
}
console.log(
  `Documentation check passed: ${markdownFiles.length} Markdown files, ${linkCount} internal links, ` +
  `${listed.length} navigation pages, ${codePathCount} repository paths.`
);
