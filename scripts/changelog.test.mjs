import assert from 'node:assert/strict';
import { mkdtempSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import path from 'node:path';
import test from 'node:test';

import { collect, fragmentProblems, preview, readFragments } from './changelog.mjs';

const CHANGELOG = `# Changelog

Intro.

## [Unreleased]

### Fixed

- Existing fix.

### Upgrade

- Existing upgrade note.

## [0.1.0] - 2026-01-01

### Added

- First release.
`;

function fragmentDir(files) {
  const dir = mkdtempSync(path.join(tmpdir(), 'changelog-d-'));
  for (const [name, text] of Object.entries(files)) writeFileSync(path.join(dir, name), text);
  return dir;
}

test('valid fragments are read in natural name order', () => {
  const dir = fragmentDir({
    'README.md': '# not a fragment\n',
    '457.fixed.md': '- Fix 457.\n',
    '45.fixed.md': '- Fix 45,\n  continued.\n',
    'oidc.security.md': '- Security fix.\n',
  });
  const { fragments, problems } = readFragments(dir);
  assert.deepEqual(problems, []);
  assert.deepEqual(fragments.map((fragment) => fragment.name), ['45.fixed.md', '457.fixed.md', 'oidc.security.md']);
  assert.equal(fragments[0].text, '- Fix 45,\n  continued.');
});

test('bad names and content are reported', () => {
  assert.match(fragmentProblems('457.fix.md', '- x')[0], /name must be/);
  assert.match(fragmentProblems('457.Fixed.md', '- x')[0], /name must be/);
  assert.match(fragmentProblems('457.fixed.md', 'No bullet')[0], /must start with a Markdown list item/);
  assert.match(fragmentProblems('457.fixed.md', '### Fixed\n- x').join('\n'), /headings are added/);
  assert.match(fragmentProblems('457.fixed.md', '- x\nnot indented').join('\n'), /line 2/);
  assert.deepEqual(fragmentProblems('457.fixed.md', '- x\n  indented\n- y\n'), []);
});

test('collect merges Unreleased and fragments in Keep a Changelog order', () => {
  const fragments = [
    { name: '1.security.md', type: 'security', text: '- Security fix.' },
    { name: '2.fixed.md', type: 'fixed', text: '- Fragment fix.' },
    { name: '3.added.md', type: 'added', text: '- New thing.' },
  ];
  const out = collect(CHANGELOG, fragments, '0.2.0', '2026-02-02');
  assert.equal(
    out,
    `# Changelog

Intro.

## [Unreleased]

## [0.2.0] - 2026-02-02

### Added

- New thing.

### Fixed

- Existing fix.
- Fragment fix.

### Security

- Security fix.

### Upgrade

- Existing upgrade note.

## [0.1.0] - 2026-01-01

### Added

- First release.
`,
  );
});

test('collect refuses a version that exists, an empty release and bad input', () => {
  assert.throws(() => collect(CHANGELOG, [], '0.1.0', '2026-02-02'), /already has a 0.1.0 section/);
  const empty = CHANGELOG.replace(/### Fixed[\s\S]*?(?=## \[0\.1\.0\])/, '');
  assert.throws(() => collect(empty, [], '0.2.0', '2026-02-02'), /nothing to release/);
  assert.throws(() => collect(CHANGELOG, [], 'v0.2', '2026-02-02'), /not a version/);
  assert.throws(() => collect(CHANGELOG, [], '0.2.0', 'today'), /not a date/);
  assert.throws(() => collect('# Changelog\n', [], '0.2.0', '2026-02-02'), /no "## \[Unreleased\]"/);
});

test('preview shows Unreleased with the fragments merged in', () => {
  const out = preview(CHANGELOG, [{ name: '2.fixed.md', type: 'fixed', text: '- Fragment fix.' }]);
  assert.match(out, /### Fixed\n\n- Existing fix\.\n- Fragment fix\./);
  assert.match(out, /### Upgrade\n\n- Existing upgrade note\./);
});
