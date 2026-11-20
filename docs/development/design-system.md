# ORBIT web design system

The reference for anyone changing `apps/web`. It records the decisions behind the UI so
new screens look and behave like the existing ones. Tokens live in
`apps/web/app/globals.css` and `apps/web/tailwind.config.ts`; primitives in
`apps/web/components/ui`; the frame in `apps/web/components/shell`.

## Principles

1. **An instrument, not a brochure.** Operators come to see what is running, what failed
   and what needs them. Status and the next action come first; explanation second.
2. **One loud thing per screen.** Solid colour is reserved for the primary action.
   Everything else is hairlines, tints and type weight.
3. **Flat surfaces.** Cards are bordered, never shadowed. Only overlays (dialogs, menus,
   the command palette) are elevated.
4. **Status is never colour alone.** The orbit glyph changes shape per state and always
   travels with a label (or an `aria-label` when space forces it to stand alone).
5. **Nothing is desktop-only.** Every page is reachable and usable at 360px.
6. **Plain words.** Sentence case, active voice, the user's vocabulary. An action keeps
   one name from button to toast ("Archive agent" → "Agent archived").

## Tokens

Colours are HSL triplets consumed as `hsl(var(--token))`, so alpha modifiers work
(`bg-primary/10`). Use the Tailwind names, never raw `hsl(var(...))` in class strings.

| Role | Class | Use |
|---|---|---|
| Canvas | `bg-background` | Page ground |
| Surface | `bg-card` | Cards, controls |
| Sunken | `bg-subtle`, `bg-muted` | Table headers, code blocks, hover |
| Text | `text-foreground`, `text-muted-foreground` | Body, secondary |
| Lines | `border` (default), `border-border-strong` | Hairlines, emphasised edges |
| Brand / action | `bg-primary`, `text-primary` | Primary button, links, selection |
| Selection tint | `bg-accent`, `text-accent-foreground` | Selected rows and options |
| Semantic | `success`, `warning`, `info`, `destructive` | Text, tints, borders |
| Frame | `bg-ink`, `text-ink-foreground`, `text-ink-muted`, `text-ink-accent` | Sidebar, phone bars (theme-aware) |
| Scrim | `bg-overlay/55` | Behind dialogs and sheets |

The frame (sidebar, phone bars, sign-in) follows the theme: a white rail on the light canvas
and a darker rail on the dark one. Inside a frame region set `data-ink` so focus rings use
the frame's accent.

Semantic meaning is fixed: **info** = in flight, **success** = finished well / healthy,
**warning** = waiting on a person or degraded, **destructive** = failed or irreversible.

Radius: `rounded-md` controls, `rounded-lg` grouped options and callouts, `rounded-xl`
cards and dialogs, `rounded-full` only for avatars and count pills.

## Type

- **Archivo Variable** for everything. `.font-display` (600 weight, tight tracking) is for
  page titles, dialog titles and headline figures. Only the ORBIT wordmark uses the wide cut.
- **JetBrains Mono Variable** only for things that are code: IDs, YAML/JSON, cron, logs.
  Never for labels.
- Fonts are bundled from `@fontsource-variable/*`; nothing is fetched at runtime, so
  air-gapped installs render correctly.

| Use | Classes |
|---|---|
| Page title | `font-display text-2xl sm:text-[1.75rem]` (via `PageHeader`) |
| Section / card title | `text-[15px] font-semibold` |
| Body, controls | `text-sm` (14px) |
| Secondary text, hints | `text-[13px] text-muted-foreground` |
| Smallest allowed | `text-xs` (12px) |

No text below 12px. No uppercase labels, no letter-spaced eyebrows. Use `.tabular` on
numbers that sit in columns or update in place.

## Components

| Component | File | Notes |
|---|---|---|
| `PageHeader` | `ui/page-header.tsx` | `title`, `description`, `actions`, `back` (parent link), `meta` (status and facts about the object) |
| `Section` | `ui/section.tsx` | Titled region with optional `count`, `description`, `actions` |
| `Card` | `ui/card.tsx` | Bordered surface. List cards use `overflow-hidden` and rows with `border-b last:border-b-0` |
| `Button` | `ui/button.tsx` | `default` (one per view: the sidebar's Start work counts on desktop, and an empty state repeats the header action as `outline`), `outline`, `ghost`, `destructive`, `link`; sizes `sm`, `default`, `lg`, `icon`, `icon-sm`. `loading` disables and shows a spinner. Icon-only buttons need `aria-label` |
| `Badge` | `ui/badge.tsx` | Tinted label: `secondary`, `outline`, `default`, `success`, `warning`, `info`, `destructive` |
| `Status`, `StatusGlyph` | `ui/status.tsx` | The orbit glyph. Use `ExecutionStatusBadge` / `AgentStatusBadge` from `ui/status-badge.tsx` |
| `Segmented` | `ui/segmented.tsx` | One-of-many filter or view switch (radiogroup, arrow keys). Not for navigation between pages |
| `Tabs` | `ui/tabs.tsx` | Underlined; for sections of one object |
| `Input`, `Textarea`, `Select` | `ui/*.tsx` | Share `controlClass`. `Select` is the native element on purpose |
| `Checkbox`, `CheckboxField` | `ui/checkbox.tsx` | Native checkbox; `CheckboxField` is the titled, fully clickable row |
| `Switch` | `ui/switch.tsx` | For settings that apply immediately |
| `Field` | `ui/field.tsx` | Label + control + hint/error; pass `optional` instead of writing "(optional)" |
| `ChoiceCard` | `ui/choice-card.tsx` | Radio option with title and explanation; wrap a set in `role="radiogroup"` |
| `Disclosure` | `ui/disclosure.tsx` | Collapsed group of settings most people skip ("Advanced"); `summary` says what is inside or currently set |
| `Callout` | `ui/callout.tsx` | Inline page message: `info`, `warning`, `danger`, `neutral`, with optional `action` |
| `EmptyState` | `ui/empty-state.tsx` | What belongs here plus the action that fills it |
| `DescriptionList` | `ui/description-list.tsx` | Label/value facts; `mono` for IDs |
| `Dialog` | `ui/dialog.tsx` | Tasks that need focus. `DialogFooter` sticks to the bottom of a long dialog: cancel (`outline`) left, action right, and say beside it what is missing when the action is disabled |
| `useConfirm` | `ui/confirm.tsx` | Replaces `window.confirm`. Title asks the question, button names the action, focus starts on Cancel |
| `DropdownMenu` | `ui/dropdown-menu.tsx` | Overflow actions. Destructive items last, after a separator |
| `Markdown` | `ui/markdown.tsx` | Agent answers and other model-written text, including messages in the execution timeline (folded past ~14 lines). Raw HTML is never interpreted |
| `TagList` | `ui/tag-list.tsx` | A short row of tags; the rest fold behind "+N more". Use instead of a comma-joined value |
| `SkillPicker` | `skill-picker.tsx` | Choosing skills: fixed-height scrolling list, search past six items, an "All / Selected" filter, descriptions cleaned and clamped to two lines |
| `Skeleton` | `ui/skeleton.tsx` | Loading placeholder shaped like what will appear |

## Patterns

**Page skeleton.** `<div className="space-y-6">` → `PageHeader` → optional `Callout`s →
toolbar (search left, `Segmented` filters) → content. The shell supplies width and
padding; pages never set their own max-width.

**Lists.** Things people compare (agents, executions, decisions) are rows inside one
`Card`, with aligned columns from `md` up and a stacked two-line layout below. A column
header row (`bg-subtle text-xs font-medium text-muted-foreground`) appears once the columns do
(`ExecutionListHeader`, the agents list); compact previews such as Overview's recent list omit it.
Use cards in a grid only for a catalogue of unlike things (connectors, harnesses).

**Registry lists** (Agents, Agent teams, Skills, Team runs) share `components/ui/resource-list.tsx`:
`ListToolbar` with `SearchField` (always shown, left) and a `Segmented` filter with counts;
`SelectionBar` once a row is ticked (select all shown, bulk actions, clear); `ListCard` whose
header uses the same `LIST_GRID` as `ListRow`, so labels sit over their cells including the
checkbox column; rows are name · status · details · updated, with the name cell holding the
whole-row link (`ROW_LINK`) plus a small fact (`v5`, `2 agents`) and a one-line description;
`ListCount` underneath ("3 agents." / "2 of 5 agents match."). Status uses `<Status>`
glyphs: lifecycle for agents, how work arrives for teams, in use or not for skills.

**Detail pages.** `PageHeader` with `back` and `meta`; primary action first, then
secondary buttons, then a `DropdownMenu` for rare and destructive actions. Body is
`grid lg:grid-cols-[minmax(0,1fr)_21rem]`: the work on the left, facts on the right.

**Forms.** One column of `Field`s; two columns only for short related pairs. Hints say
what the value does, not what the control is. Validate on submit and show errors next
to the field; disable the submit button only while required input is missing, and say
what is missing beside it.

**Long editors.** An object with many groups of settings (Agent Studio) shows one
section at a time. A section nav lists every section with its current value, so it doubles
as the summary, and marks sections with errors; Save stays in view under it. Each section
ends with Back / Next. On phones the nav is a row of step pills. The URL hash names the
section (`#model`). A failed save opens the first section with a problem.

**Imported text.** Descriptions written for machines (SKILL.md frontmatter) go through
`cleanDescription()` before display: YAML block markers and wrapping quotes are formatting,
not content. Clamp them (`line-clamp-2`) and put the full text in `title`.

**Unfinished features.** A control that is not wired to anything is disabled and says why
(planned harnesses, licensed features, options the API does not accept yet). Never collect
input that is then ignored.

**Destructive actions.** Always `useConfirm` with `destructive: true`, a title that
names the object, and one sentence on what is lost and whether it can be undone.
Prefer a reversible step (archive) and say so.

**States.** Every data region handles loading (`Skeleton`), error (`Callout
tone="danger"` with the server message), empty (`EmptyState`) and filtered-empty (say
that filters hide results and offer "Clear filters").

**Filters.** Client-side and instant where the set is small; otherwise keep them in the
URL (`?status=failed`) so views can be linked and survive the back button.

**Execution events.** `lib/execution-events.ts` turns raw harness events into timeline
lines (`describeEvent`) and decides what belongs in the default "Story" view (`isKeyEvent`).
Teach it a new harness's event shapes there, not in the page; unknown shapes fall back to
the server's message.

**Vocabulary.** One name per concept, taken from the shared source where one exists:
autonomy levels from `AUTONOMY_LEVELS` in `@orbit/contracts` via `autonomyName()`; product
names via `brandName()`; server identifiers via `humanizeIdentifier()` (never print
`camelCase` or `snake_case` keys). Times are 24-hour (`formatDateTime`).

**Feedback.** `toast.success` for a completed action (past tense, no exclamation),
`toast.error` with the server message for a failed one. Toasts sit bottom-right, above
the tab bar on phones.

**Motion.** Only in response to an action (dialog, sheet, menu) or to show live work
(the orbiting satellite). No page-load fades. `prefers-reduced-motion` disables all of it.

## Navigation

Defined once in `components/shell/nav.ts`. Groups follow the operator's questions:
what is happening (Overview, Executions, Approvals) → what the agents are (Build) → what
they can reach (Connect) → where they run (Infrastructure) → who may do what (Admin).
"Start work" is the standing primary action. Build and Connect are open by default;
Infrastructure and Admin start folded (remembered per browser), and the group that holds
the current page is always open. The phone tab bar carries four daily
destinations and "More", which lists every page. `⌘K` / `Ctrl+K` opens the command
palette (pages, agents, actions).

Adding a page: add it to `NAV_GROUPS` with `keywords` for the palette. Do not add a
sixth group without removing one.

## Accessibility checklist for a change

- Reachable and operable by keyboard; focus visible (global `:focus-visible` outline).
- Icon-only controls have `aria-label`; decorative icons are not announced.
- Text contrast ≥ 4.5:1 against its surface in both themes (tokens are tuned for this;
  check any new tint).
- Touch targets ≥ 32px, primary ones ≥ 36px.
- Usable at 360px without horizontal scrolling; long IDs use `break-all` or `truncate`.
