"use client";

import type { KeyboardEvent, ReactNode } from "react";
import { cn } from "@/lib/utils";

/** A selectable option with a title and explanation. Use inside a radiogroup. */
export function ChoiceCard({
  selected,
  onSelect,
  title,
  description,
  icon,
  trailing,
  disabled,
  className,
}: {
  selected: boolean;
  onSelect: () => void;
  title: ReactNode;
  description?: ReactNode;
  icon?: ReactNode;
  trailing?: ReactNode;
  disabled?: boolean;
  className?: string;
}) {
  /** Arrow keys move through the group and select, as a native radio group does. */
  function onKeyDown(event: KeyboardEvent<HTMLButtonElement>) {
    const delta =
      event.key === "ArrowRight" || event.key === "ArrowDown"
        ? 1
        : event.key === "ArrowLeft" || event.key === "ArrowUp"
          ? -1
          : 0;
    if (!delta) return;
    const group = event.currentTarget.closest('[role="radiogroup"]');
    if (!group) return;
    const radios = Array.from(
      group.querySelectorAll<HTMLButtonElement>('button[role="radio"]:not(:disabled)'),
    );
    const index = radios.indexOf(event.currentTarget);
    if (index < 0 || radios.length < 2) return;
    event.preventDefault();
    const next = radios[(index + delta + radios.length) % radios.length];
    next.focus();
    next.click();
  }

  return (
    <button
      type="button"
      role="radio"
      aria-checked={selected}
      disabled={disabled}
      onClick={onSelect}
      onKeyDown={onKeyDown}
      className={cn(
        "flex w-full items-start gap-3 rounded-lg border bg-card p-3 text-left transition-colors disabled:cursor-not-allowed disabled:opacity-50",
        selected
          ? "border-primary bg-accent/50 ring-1 ring-inset ring-primary"
          : "hover:border-border-strong hover:bg-muted/40",
        className,
      )}
    >
      {icon ? (
        <span
          className={cn(
            "mt-0.5 shrink-0 [&_svg]:h-4 [&_svg]:w-4",
            selected ? "text-primary" : "text-muted-foreground",
          )}
        >
          {icon}
        </span>
      ) : null}
      <span className="min-w-0 flex-1">
        <span className="block text-sm font-medium">{title}</span>
        {description ? (
          <span className="mt-0.5 block text-[13px] text-muted-foreground">{description}</span>
        ) : null}
      </span>
      {trailing}
    </button>
  );
}
