# Changelog fragments

Pull requests do not edit `CHANGELOG.md`. Each one adds a small file here instead, so two
pull requests never conflict over the `Unreleased` section. A release collects the files
into `CHANGELOG.md` (see [Releasing](../docs/development/releasing.md)).

## Adding a fragment

Create `changelog.d/<name>.<type>.md`:

- `<type>` is the Keep a Changelog section: `added`, `changed`, `deprecated`, `removed`,
  `fixed` or `security`.
- `<name>` is usually the issue number, for example `457.fixed.md`. Use a short lowercase
  slug when there is no issue (`metrics-token.fixed.md`). A pull request that needs two
  sections adds two files (`457.fixed.md`, `457.security.md`).

The content is the list item exactly as it should read in `CHANGELOG.md`: start with `- `,
indent continuation lines, write links relative to the repository root, and mark upgrade
steps with **Upgrade:**.

```markdown
- OIDC team sync grants the highest mapped role and skips writes that change
  nothing (#437). See [the OIDC guide](docs/development/oidc.md).
```

`pnpm docs:check` validates the fragments, and `node scripts/changelog.mjs preview` shows
the `Unreleased` section as it will read with them.
