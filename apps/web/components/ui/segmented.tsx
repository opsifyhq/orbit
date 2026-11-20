"use client";

import * as React from "react";
import { cn } from "@/lib/utils";

export interface SegmentOption<T extends string> {
  value: T;
  label: React.ReactNode;
  count?: number;
}

/**
 * One-of-many filter. A radio group in behaviour (arrow keys move the choice) so a
 * keyboard user lands on it once instead of tabbing through every option.
 */
export function Segmented<T extends string>({
  value,
  onChange,
  options,
  label,
  className,
  size = "default",
}: {
  value: T;
  onChange: (value: T) => void;
  options: ReadonlyArray<SegmentOption<T>>;
  label: string;
  className?: string;
  size?: "default" | "sm";
}) {
  const refs = React.useRef<Array<HTMLButtonElement | null>>([]);

  function onKeyDown(event: React.KeyboardEvent, index: number) {
    const delta =
      event.key === "ArrowRight" || event.key === "ArrowDown"
        ? 1
        : event.key === "ArrowLeft" || event.key === "ArrowUp"
          ? -1
          : 0;
    if (!delta) return;
    event.preventDefault();
    const next = (index + delta + options.length) % options.length;
    onChange(options[next].value);
    refs.current[next]?.focus();
  }

  return (
    <div
      role="radiogroup"
      aria-label={label}
      className={cn(
        "no-scrollbar inline-flex max-w-full items-center gap-0.5 overflow-x-auto rounded-lg border bg-muted/60 p-0.5",
        // A clipped option fades out, so the row reads as scrollable on a phone.
        // The trailing padding keeps the last option clear of the fade when nothing overflows.
        "max-sm:pr-6 max-sm:[mask-image:linear-gradient(to_right,#000_calc(100%-24px),transparent)]",
        className,
      )}
    >
      {options.map((option, index) => {
        const active = option.value === value;
        return (
          <button
            key={option.value}
            ref={(node) => {
              refs.current[index] = node;
            }}
            type="button"
            role="radio"
            aria-checked={active}
            tabIndex={active ? 0 : -1}
            onClick={() => onChange(option.value)}
            onKeyDown={(event) => onKeyDown(event, index)}
            className={cn(
              // The track scrolls, so it clips: draw the focus ring inside the segment.
              "inline-flex shrink-0 items-center gap-1.5 whitespace-nowrap rounded-[7px] font-medium transition-colors focus-visible:-outline-offset-2",
              size === "sm" ? "h-7 px-2.5 text-[13px]" : "h-8 px-3 text-[13px]",
              active
                ? "bg-card text-foreground ring-1 ring-border-strong/70"
                : "text-muted-foreground hover:text-foreground",
            )}
          >
            {option.label}
            {option.count !== undefined ? (
              <span className={cn("tabular text-xs", active ? "text-muted-foreground" : "opacity-70")}>
                {option.count}
              </span>
            ) : null}
          </button>
        );
      })}
    </div>
  );
}
