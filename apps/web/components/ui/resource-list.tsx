"use client";

import type { ReactNode } from "react";
import { Search, X } from "lucide-react";

import { Button } from "@/components/ui/button";
import { Card } from "@/components/ui/card";
import { Checkbox } from "@/components/ui/checkbox";
import { Input } from "@/components/ui/input";
import { Skeleton } from "@/components/ui/skeleton";
import { cn } from "@/lib/utils";

/**
 * The registry list pattern shared by Agents, Agent teams and Skills: a search field
 * and a filter, a selection bar for bulk actions, rows with aligned columns (name,
 * status, details, updated) that collapse to one block on a phone, and a count.
 *
 * Every row uses this grid so the header lines up with its cells, including the
 * checkbox column.
 */
export const LIST_GRID =
  "grid grid-cols-[auto_minmax(0,1fr)_auto] items-center gap-x-3 gap-y-2 px-4 sm:px-5 md:grid-cols-[1.5rem_minmax(0,1fr)_9rem_13rem_6.5rem] md:gap-x-4";

/** The same columns without the checkbox, for lists with no bulk actions (Team runs, Outputs). */
export const LIST_GRID_PLAIN =
  "grid grid-cols-[minmax(0,1fr)_auto] items-center gap-x-3 gap-y-2 px-4 sm:px-5 md:grid-cols-[minmax(0,1fr)_9rem_13rem_6.5rem] md:gap-x-4";

/** Classes for the whole-row link on a row's name. */
export const ROW_LINK =
  "truncate text-sm font-semibold after:absolute after:inset-0 focus-visible:outline-none focus-visible:after:outline focus-visible:after:outline-2 focus-visible:after:-outline-offset-2 focus-visible:after:outline-ring";

export function ListToolbar({ children }: { children: ReactNode }) {
  return <div className="flex flex-col gap-3 md:flex-row md:items-center">{children}</div>;
}

export function SearchField({
  value,
  onChange,
  placeholder,
  label,
}: {
  value: string;
  onChange: (value: string) => void;
  placeholder: string;
  label: string;
}) {
  return (
    <div className="relative md:w-80">
      <Search className="pointer-events-none absolute left-3 top-1/2 h-4 w-4 -translate-y-1/2 text-muted-foreground" />
      <Input
        type="search"
        className="pl-9 pr-9 [&::-webkit-search-cancel-button]:hidden"
        placeholder={placeholder}
        aria-label={label}
        value={value}
        onChange={(event) => onChange(event.target.value)}
      />
      {value ? (
        <button
          type="button"
          aria-label="Clear filter"
          onClick={() => onChange("")}
          className="absolute right-1.5 top-1/2 flex h-6 w-6 -translate-y-1/2 items-center justify-center rounded text-muted-foreground hover:bg-muted hover:text-foreground"
        >
          <X className="h-3.5 w-3.5" />
        </button>
      ) : null}
    </div>
  );
}

/** Shown once something is selected: select all shown, the bulk actions, clear. */
export function SelectionBar({
  count,
  allShownSelected,
  onSelectAllShown,
  onClear,
  noun,
  children,
}: {
  count: number;
  allShownSelected: boolean;
  onSelectAllShown: (checked: boolean) => void;
  onClear: () => void;
  /** Plural, for the select-all label: "agents". */
  noun: string;
  children: ReactNode;
}) {
  if (!count) return null;
  return (
    <div className="flex flex-wrap items-center gap-3 rounded-lg border bg-subtle px-3 py-2.5">
      <Checkbox
        aria-label={`Select all shown ${noun}`}
        checked={allShownSelected}
        onChange={(event) => onSelectAllShown(event.target.checked)}
      />
      <span className="text-[13px] font-medium">{count} selected</span>
      {children}
      <Button size="sm" variant="ghost" onClick={onClear}>
        Clear selection
      </Button>
    </div>
  );
}

/**
 * The list card with its column header. `columns` are the name, status, details and
 * updated labels. Set `selectable={false}` (and leave out each row's checkbox) for a list
 * without bulk actions, so rows do not keep an empty checkbox column.
 */
export function ListCard({
  columns,
  selectable = true,
  children,
}: {
  columns: [string, string, string, string];
  selectable?: boolean;
  children: ReactNode;
}) {
  const [name, status, details, updated] = columns;
  return (
    <Card className="overflow-hidden">
      <div
        aria-hidden
        className={cn(selectable ? LIST_GRID : LIST_GRID_PLAIN, "hidden border-b bg-subtle py-2 text-xs font-medium text-muted-foreground md:grid")}
      >
        {selectable ? <span /> : null}
        <span>{name}</span>
        <span>{status}</span>
        <span>{details}</span>
        <span className="text-right">{updated}</span>
      </div>
      <ul>{children}</ul>
    </Card>
  );
}

/**
 * One row. On a phone it is checkbox · name block · status; from `md` the details and
 * updated cells take their columns. The name cell holds the whole-row link. Without a
 * `checkbox` the row uses the plain grid, matching `ListCard selectable={false}`.
 */
export function ListRow({
  checkbox,
  name,
  status,
  details,
  updated,
  footer,
}: {
  checkbox?: ReactNode;
  name: ReactNode;
  status: ReactNode;
  details: ReactNode;
  updated: ReactNode;
  /** Actions under the row (outside the row link), e.g. restore an archived agent. */
  footer?: ReactNode;
}) {
  return (
    <li className="group relative border-b last:border-b-0">
      <div className={cn(checkbox ? LIST_GRID : LIST_GRID_PLAIN, "py-3.5 transition-colors group-hover:bg-muted/50")}>
        {checkbox ? <div className="relative z-10 flex">{checkbox}</div> : null}
        <div className="min-w-0">{name}</div>
        <div className="justify-self-end md:justify-self-start">{status}</div>
        <div className="hidden min-w-0 text-[13px] md:block">{details}</div>
        <p className="tabular hidden text-right text-[13px] text-muted-foreground md:block">{updated}</p>
      </div>
      {footer ? <div className="relative flex flex-wrap gap-2 px-4 pb-3.5 sm:px-5">{footer}</div> : null}
    </li>
  );
}

export function ListSkeleton({ rows = 4 }: { rows?: number }) {
  return (
    <Card className="overflow-hidden">
      {Array.from({ length: rows }).map((_, index) => (
        <div key={index} className="border-b px-5 py-4 last:border-b-0">
          <Skeleton className="h-10" />
        </div>
      ))}
    </Card>
  );
}

/** "3 agents." or "2 of 5 agents match." under the list. */
export function ListCount({
  shown,
  total,
  singular,
  plural,
  note,
}: {
  shown: number;
  total: number;
  singular: string;
  plural: string;
  note?: string;
}) {
  return (
    <p className="tabular text-[13px] text-muted-foreground">
      {shown === total ? `${shown} ${shown === 1 ? singular : plural}.` : `${shown} of ${total} ${plural} match.`}
      {note ? ` ${note}` : ""}
    </p>
  );
}

/** Toggle one id in a selection set. */
export function toggleId(current: Set<string>, id: string, checked: boolean): Set<string> {
  const next = new Set(current);
  if (checked) next.add(id);
  else next.delete(id);
  return next;
}

/** Add or remove every shown id. */
export function toggleShown(current: Set<string>, shownIds: string[], checked: boolean): Set<string> {
  return checked
    ? new Set([...current, ...shownIds])
    : new Set([...current].filter((id) => !shownIds.includes(id)));
}
