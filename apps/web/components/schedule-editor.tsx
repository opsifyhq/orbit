"use client";

import { useMemo, useRef, useState, type KeyboardEvent, type ReactNode } from "react";
import { CalendarClock, Code2 } from "lucide-react";

import { Button } from "@/components/ui/button";
import { Field } from "@/components/ui/field";
import { Input } from "@/components/ui/input";
import { Select } from "@/components/ui/select";
import {
  DAY_ORDER,
  DAY_SHORT,
  DEFAULT_MODEL,
  WEEKDAYS,
  WEEKEND,
  browserTimeZone,
  buildCron,
  clock,
  cronError,
  describeSchedule,
  formatClock,
  formatRun,
  nextRuns,
  parseCron,
  timeZones,
  type Frequency,
  type ScheduleModel,
} from "@/lib/schedule";
import { cn } from "@/lib/utils";

export interface ScheduleValue {
  cron: string;
  /** IANA zone; empty or missing means UTC. */
  timezone?: string;
}

const FREQUENCIES: Array<{ value: Frequency; label: string }> = [
  { value: "minutes", label: "Minutes" },
  { value: "hourly", label: "Hourly" },
  { value: "daily", label: "Daily" },
  { value: "weekly", label: "Weekly" },
  { value: "monthly", label: "Monthly" },
  { value: "custom", label: "Custom" },
];

const MINUTE_STEPS = [1, 2, 5, 10, 15, 20, 30];
const HOUR_STEPS = [1, 2, 3, 4, 6, 8, 12];

/** Option values for a select, keeping an unusual current value selectable. */
const withCurrent = (values: number[], current: number) =>
  values.includes(current) ? values : [...values, current].sort((a, b) => a - b);

/** A group of pill-shaped radio buttons that wraps on narrow screens. */
function PillGroup<T extends string>({
  label,
  value,
  options,
  onChange,
}: {
  label: string;
  value: T;
  options: Array<{ value: T; label: string }>;
  onChange: (value: T) => void;
}) {
  const refs = useRef<Array<HTMLButtonElement | null>>([]);
  function onKeyDown(event: KeyboardEvent, index: number) {
    const delta = event.key === "ArrowRight" || event.key === "ArrowDown" ? 1 : event.key === "ArrowLeft" || event.key === "ArrowUp" ? -1 : 0;
    if (!delta) return;
    event.preventDefault();
    const next = (index + delta + options.length) % options.length;
    onChange(options[next].value);
    refs.current[next]?.focus();
  }
  return (
    <div role="radiogroup" aria-label={label} className="flex flex-wrap gap-1.5">
      {options.map((option, index) => {
        const selected = option.value === value;
        return (
          <button
            key={option.value}
            ref={(node) => {
              refs.current[index] = node;
            }}
            type="button"
            role="radio"
            aria-checked={selected}
            tabIndex={selected ? 0 : -1}
            onClick={() => onChange(option.value)}
            onKeyDown={(event) => onKeyDown(event, index)}
            className={cn(
              "inline-flex h-8 items-center rounded-full border px-3 text-[13px] font-medium transition-colors focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring",
              selected
                ? "border-primary/50 bg-accent text-accent-foreground"
                : "bg-card text-muted-foreground hover:bg-muted/60 hover:text-foreground",
            )}
          >
            {option.label}
          </button>
        );
      })}
    </div>
  );
}

/** Label + control on one line where there is room; stacked on a phone. */
function Row({ label, htmlFor, children }: { label: string; htmlFor?: string; children: ReactNode }) {
  return (
    <div className="grid gap-1.5 sm:grid-cols-[6.5rem_minmax(0,1fr)] sm:items-center sm:gap-3">
      <label htmlFor={htmlFor} className="text-[13px] font-medium text-muted-foreground">
        {label}
      </label>
      <div className="flex min-w-0 flex-wrap items-center gap-2">{children}</div>
    </div>
  );
}

/**
 * Pick when something repeats without writing cron: a frequency, then only the
 * controls that frequency needs, in a chosen time zone. A plain-English summary and
 * the next few runs confirm the result; Custom keeps raw cron for anything else.
 */
export function ScheduleEditor({
  id,
  value,
  onChange,
}: {
  id: string;
  value: ScheduleValue;
  onChange: (next: ScheduleValue) => void;
}) {
  const parsed = parseCron(value.cron);
  // The model is local so a choice survives a round trip that would read differently
  // (seven ticked days read back as "daily", for instance).
  const [model, setModel] = useState<ScheduleModel>(() => parsed);
  const [frequency, setFrequency] = useState<Frequency>(parsed.frequency);
  // Follow a value that changed from outside (a reset, or a list that shifted).
  const [synced, setSynced] = useState(value.cron);
  if (value.cron !== synced) {
    setSynced(value.cron);
    setModel(parsed);
    if (frequency !== "custom") setFrequency(parsed.frequency);
  }
  const timezone = value.timezone || "UTC";
  const localZone = useMemo(browserTimeZone, []);
  // Your own zone first, then UTC, then the rest; an unknown stored zone stays selectable.
  const zones = useMemo(() => {
    const rest = timeZones().filter((zone) => zone !== localZone && zone !== "UTC" && zone !== timezone);
    return [...new Set([localZone, "UTC", timezone, ...rest])];
  }, [localZone, timezone]);

  function apply(next: ScheduleModel) {
    const cron = buildCron(next);
    setModel(next);
    setSynced(cron);
    onChange({ ...value, cron });
  }

  function pickFrequency(next: Frequency) {
    setFrequency(next);
    if (next === "custom") return;
    const base = parsed.frequency === "custom" ? model : { ...model, ...parsed };
    const every =
      next === "minutes"
        ? base.frequency === "minutes"
          ? base.every
          : DEFAULT_MODEL.every
        : next === "hourly" && base.frequency === "hourly"
          ? base.every
          : 1;
    const days = base.days.length ? base.days : WEEKDAYS;
    apply({ ...base, frequency: next, every, days });
  }

  const time = clock(model.hour, model.minute);
  const setTime = (text: string) => {
    const match = /^(\d{1,2}):(\d{2})/.exec(text);
    if (match) apply({ ...model, hour: Number(match[1]), minute: Number(match[2]) });
  };
  const timeInput = (
    <Input
      id={`${id}-time`}
      type="time"
      step={60}
      className="w-32 tabular"
      value={time}
      onChange={(event) => setTime(event.target.value)}
      required
    />
  );

  const error = cronError(value.cron, timezone);
  const runs = error ? [] : nextRuns(value.cron, timezone, 3);
  const showZone = frequency !== "minutes" && !(frequency === "hourly" && model.every === 1);
  const otherZone = timezone !== localZone;
  const sameDays = (a: number[], b: number[]) => a.length === b.length && a.every((day) => b.includes(day));

  return (
    <div className="space-y-4">
      <PillGroup label="Repeats" value={frequency} options={FREQUENCIES} onChange={pickFrequency} />

      <div className="space-y-3">
        {frequency === "minutes" ? (
          <Row label="Every" htmlFor={`${id}-every`}>
            <Select
              id={`${id}-every`}
              className="w-auto"
              value={model.every}
              onChange={(event) => apply({ ...model, every: Number(event.target.value) })}
            >
              {withCurrent(MINUTE_STEPS, model.every).map((n) => (
                <option key={n} value={n}>
                  {n === 1 ? "minute" : `${n} minutes`}
                </option>
              ))}
            </Select>
          </Row>
        ) : null}

        {frequency === "hourly" ? (
          <>
            <Row label="Every" htmlFor={`${id}-every`}>
              <Select
                id={`${id}-every`}
                className="w-auto"
                value={model.every}
                onChange={(event) => apply({ ...model, every: Number(event.target.value) })}
              >
                {withCurrent(HOUR_STEPS, model.every).map((n) => (
                  <option key={n} value={n}>
                    {n === 1 ? "hour" : `${n} hours`}
                  </option>
                ))}
              </Select>
            </Row>
            <Row label="At minute" htmlFor={`${id}-minute`}>
              <Select
                id={`${id}-minute`}
                className="w-auto tabular"
                value={model.minute}
                onChange={(event) => apply({ ...model, minute: Number(event.target.value) })}
              >
                {withCurrent([0, 5, 10, 15, 20, 25, 30, 35, 40, 45, 50, 55], model.minute).map((n) => (
                  <option key={n} value={n}>
                    {n === 0 ? ":00 (on the hour)" : `:${String(n).padStart(2, "0")}`}
                  </option>
                ))}
              </Select>
            </Row>
          </>
        ) : null}

        {frequency === "weekly" ? (
          <Row label="On">
            <div role="group" aria-label="Days of the week" className="grid w-full grid-cols-7 gap-1 sm:flex sm:w-auto">
              {DAY_ORDER.map((day) => {
                const on = model.days.includes(day);
                return (
                  <button
                    key={day}
                    type="button"
                    aria-pressed={on}
                    // At least one day stays ticked: a weekly schedule with none never runs.
                    onClick={() =>
                      on
                        ? model.days.length > 1 && apply({ ...model, days: model.days.filter((d) => d !== day) })
                        : apply({ ...model, days: [...model.days, day] })
                    }
                    className={cn(
                      "h-9 min-w-0 rounded-md border px-0 text-[13px] font-medium sm:h-8 sm:w-11 transition-colors focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring",
                      on ? "border-primary bg-primary text-primary-foreground" : "bg-card text-muted-foreground hover:bg-muted/60",
                    )}
                  >
                    {DAY_SHORT[day]}
                  </button>
                );
              })}
            </div>
            <div className="flex gap-1">
              <Button
                type="button"
                size="sm"
                variant="ghost"
                aria-pressed={sameDays(model.days, WEEKDAYS)}
                onClick={() => apply({ ...model, days: WEEKDAYS })}
              >
                Weekdays
              </Button>
              <Button
                type="button"
                size="sm"
                variant="ghost"
                aria-pressed={sameDays(model.days, WEEKEND)}
                onClick={() => apply({ ...model, days: WEEKEND })}
              >
                Weekend
              </Button>
            </div>
          </Row>
        ) : null}

        {frequency === "monthly" ? (
          <Row label="On day" htmlFor={`${id}-dom`}>
            <Select
              id={`${id}-dom`}
              className="w-auto tabular"
              value={model.dayOfMonth}
              onChange={(event) => apply({ ...model, dayOfMonth: Number(event.target.value) })}
            >
              {Array.from({ length: 31 }, (_, i) => i + 1).map((n) => (
                <option key={n} value={n}>
                  {n}
                </option>
              ))}
            </Select>
            {model.dayOfMonth > 28 ? (
              <span className="text-[13px] text-muted-foreground">Months without this day are skipped.</span>
            ) : null}
          </Row>
        ) : null}

        {frequency === "daily" || frequency === "weekly" || frequency === "monthly" ? (
          <Row label="At" htmlFor={`${id}-time`}>
            {timeInput}
          </Row>
        ) : null}

        {frequency === "custom" ? (
          <Field
            label="Cron expression"
            htmlFor={`${id}-cron`}
            error={error}
            hint="Five fields: minute, hour, day of month, month, day of week. For example 0 9 * * 1-5."
          >
            <Input
              id={`${id}-cron`}
              className="font-mono sm:max-w-xs"
              value={value.cron}
              onChange={(event) => onChange({ ...value, cron: event.target.value })}
              placeholder="0 9 * * 1-5"
              spellCheck={false}
              autoComplete="off"
            />
          </Field>
        ) : null}

        {showZone ? (
          <Row label="Time zone" htmlFor={`${id}-zone`}>
            <Select
              id={`${id}-zone`}
              className="w-full sm:w-72"
              value={timezone}
              onChange={(event) =>
                onChange({ ...value, timezone: event.target.value === "UTC" ? undefined : event.target.value })
              }
            >
              {zones.map((zone) => (
                <option key={zone} value={zone}>
                  {zone.replace(/_/g, " ")}
                  {zone === localZone ? " (your time zone)" : ""}
                </option>
              ))}
            </Select>
          </Row>
        ) : null}
      </div>

      <div className="rounded-lg border bg-muted/40 p-3" aria-live="polite">
        <p className="flex items-start gap-2 text-sm font-medium">
          <CalendarClock className="mt-0.5 h-4 w-4 shrink-0 text-muted-foreground" aria-hidden />
          <span className="min-w-0">{error ? "This schedule cannot run yet" : describeSchedule(value.cron, value.timezone)}</span>
        </p>
        {runs.length ? (
          <div className="mt-2 pl-6 text-[13px] text-muted-foreground">
            <p>Next runs</p>
            <ul className="mt-0.5 space-y-0.5">
              {runs.map((run) => (
                <li key={run.getTime()} className="tabular text-foreground">
                  {formatRun(run, timezone)}
                  {otherZone ? (
                    <span className="text-muted-foreground"> · {formatClock(run, localZone)} your time</span>
                  ) : null}
                </li>
              ))}
            </ul>
          </div>
        ) : error && frequency !== "custom" ? (
          <p className="mt-1 pl-6 text-[13px] text-destructive">{error}</p>
        ) : null}
        {frequency !== "custom" ? (
          <div className="mt-2 flex flex-wrap items-center gap-x-2 pl-6 text-xs text-muted-foreground">
            <span>
              Cron <code className="font-mono">{value.cron}</code>
            </span>
            <button
              type="button"
              onClick={() => setFrequency("custom")}
              className="inline-flex items-center gap-1 font-medium text-primary underline-offset-4 hover:underline"
            >
              <Code2 className="h-3 w-3" aria-hidden /> Edit as cron
            </button>
          </div>
        ) : null}
      </div>
    </div>
  );
}
