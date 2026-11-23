import { brandName, capabilityLabel, humanizeIdentifier } from "@/lib/presentation";
import type { ExecutionEventRecord } from "@/lib/types";

/**
 * Turns raw harness/worker events into the story of a run: what the agent said,
 * what it did, what the platform allowed, and what a person decided.
 *
 * Written against real OpenCode and mock-harness event streams. Unknown shapes fall
 * back to the server's own message, so nothing is ever hidden by a failed guess.
 */

export type EventTone = "plain" | "muted" | "agent" | "action" | "warning" | "danger" | "success";

export interface EventView {
  /** Short label for who or what this line is about. */
  label: string;
  tone: EventTone;
  title: string;
  /** One line of supporting detail (a path, a command, arguments). */
  detail?: string;
  /** A decision the platform or a person made about this action. */
  badge?: { text: string; tone: "neutral" | "success" | "warning" | "danger" };
  /** Agent prose: rendered as text rather than in the mono log face. */
  prose?: boolean;
}

function data(event: ExecutionEventRecord): Record<string, unknown> {
  return event.data && typeof event.data === "object" ? (event.data as Record<string, unknown>) : {};
}

function record(value: unknown): Record<string, unknown> {
  return value && typeof value === "object" ? (value as Record<string, unknown>) : {};
}

function oneLine(value: unknown, max = 140): string {
  return String(value ?? "")
    .replace(/\s+/g, " ")
    .trim()
    .slice(0, max);
}

/** "opsifyhq/orbit", "#58" and friends, pulled from connector arguments. */
function target(args: Record<string, unknown>): string {
  const repo = [args.owner, args.repo ?? args.project].filter(Boolean).join("/");
  const number = args.number ?? args.iid ?? args.issue_number;
  return [number !== undefined ? `#${number}` : "", repo ? `in ${repo}` : ""].filter(Boolean).join(" ");
}

function argumentSummary(args: Record<string, unknown>): string | undefined {
  const entries = Object.entries(args)
    .filter(([key]) => !["owner", "repo", "project", "number", "iid"].includes(key))
    .slice(0, 3)
    .map(([key, value]) => `${key}: ${oneLine(value, 90)}`);
  return entries.length ? entries.join("   ") : undefined;
}

const TOOL_VERBS: Record<string, string> = {
  write: "Wrote",
  edit: "Edited",
  patch: "Edited",
  read: "Read",
  shell: "Ran",
  bash: "Ran",
  grep: "Searched for",
  glob: "Looked for files matching",
  list: "Listed",
  webfetch: "Fetched",
  websearch: "Searched the web for",
  todowrite: "Updated its task list",
  task: "Started a sub-task",
};

/** What a harness-internal tool call did, in a few words. */
function describeTool(tool: string, input: Record<string, unknown>): { title: string; detail?: string } {
  const name = tool.toLowerCase();

  if (name === "question") {
    const first = record((Array.isArray(input.questions) ? input.questions : [])[0]);
    const text = oneLine(first.question ?? first.header ?? "", 200);
    return {
      title: text ? `Asked: ${text}` : "Asked a question",
      detail: "Nobody can answer inside a run. The agent has to carry on by itself.",
    };
  }

  if (name === "execute") {
    // Code Mode: the agent writes a small script against its tools.
    const code = String(input.code ?? "");
    const calls = [...code.matchAll(/tools\.\w+\.(\w+)\s*\(/g)].map((match) => match[1]);
    if (calls.length) {
      return { title: `Called ${[...new Set(calls)].map((call) => call.replace(/_/g, " ")).join(", ")}` };
    }
    if (/\bsearch\s*\(/.test(code)) return { title: "Looked up which tools it has" };
    return { title: "Ran a script", detail: oneLine(code, 160) || undefined };
  }

  const subject = input.path ?? input.filePath ?? input.command ?? input.pattern ?? input.query ?? input.url;
  const verb = TOOL_VERBS[name];
  if (verb) {
    return { title: subject ? `${verb} ${oneLine(subject)}` : verb };
  }
  return {
    title: humanizeIdentifier(tool) || "Used a tool",
    detail: subject ? oneLine(subject) : undefined,
  };
}

/** Friendlier wording for the worker's bookkeeping lines. */
function describeStatus(message: string): { title: string; tone?: EventTone } {
  let match = message.match(/^execution queued; waiting for an? (\S+) worker(?: \(placement: (\w+)\))?/);
  if (match) {
    const where = match[2] && match[2] !== "auto" ? ` on ${match[2]}` : "";
    return { title: `Queued, waiting for ${article(brandName(match[1]))} worker${where}`, tone: "muted" };
  }
  if (/^execution dispatched to harness/.test(message)) return { title: "Picked up by a worker", tone: "muted" };
  match = message.match(/^harness session (\S+) started/);
  if (match) return { title: "Harness session started", tone: "muted" };
  match = message.match(/^policy-enforcing tool broker ready \((\d+) (.+?) tools?\)/);
  if (match) return { title: `${match[1]} ${match[2]} tools made available to the agent`, tone: "muted" };
  if (/^repository checkout skipped/.test(message)) {
    return { title: "No repository to check out; the agent starts in an empty workspace", tone: "muted" };
  }
  match = message.match(/^execution (succeeded|cancelled)/);
  if (match) return { title: match[1] === "succeeded" ? "Finished successfully" : "Cancelled", tone: match[1] === "succeeded" ? "success" : "muted" };
  if (/^a worker took this execution but never started it/.test(message)) {
    return { title: "Ended: the worker that took it never started it", tone: "danger" };
  }
  if (/^cancelled; the worker that took this execution never started it/.test(message)) {
    return { title: "Cancelled before the worker started it", tone: "muted" };
  }
  if (/^worker heartbeat expired/.test(message)) {
    return { title: "Ended: the worker stopped responding", tone: "danger" };
  }
  if (/^worker restarted; execution outcome is unknown/.test(message)) {
    return { title: "Ended: the worker restarted mid-run", tone: "danger" };
  }
  if (/^worker stopped reporting this execution/.test(message)) {
    return { title: "Ended: the worker stopped reporting this run", tone: "danger" };
  }
  if (/^worker container was OOMKilled/.test(message)) {
    return { title: "Ended: the worker was OOMKilled", tone: "danger" };
  }
  if (/^worker (pod|container) was evicted/.test(message)) {
    return { title: "Ended: the worker pod was evicted", tone: "danger" };
  }
  if (/^worker container exited/.test(message)) {
    return { title: "Ended: the worker container exited", tone: "danger" };
  }
  if (/^worker pod was lost/.test(message)) {
    return { title: "Ended: the worker was lost", tone: "danger" };
  }
  match = message.match(/^execution finished with status (\w+)/);
  if (match) return { title: `Ended: ${humanizeIdentifier(match[1]).toLowerCase()}`, tone: "danger" };
  return { title: message };
}

function article(word: string) {
  return `${/^[aeiou]/i.test(word) ? "an" : "a"} ${word}`;
}

export function describeEvent(event: ExecutionEventRecord): EventView {
  const d = data(event);
  const capability = typeof d.capability === "string" ? d.capability : null;
  const tool = typeof d.tool === "string" ? d.tool : null;
  const args = record(d.arguments ?? d.input);
  const failed = event.level === "error" || event.kind === "error";

  // What the agent said along the way, or its final answer.
  if (event.kind === "output") {
    return { label: "Agent", tone: "agent", title: event.message, prose: true };
  }

  if (event.kind === "tool_call") {
    // Approval lifecycle, reported by the platform about a connector action.
    const approval = event.message.match(/^approval (requested|approved|rejected|expired) for (\S+)/);
    if (approval) {
      const action = `${capabilityLabel(approval[2])} ${target(args)}`.trim();
      const state = approval[1];
      if (state === "requested") {
        return {
          label: "Approval",
          tone: "warning",
          title: `Asked a person to approve: ${action}`,
          detail: argumentSummary(args),
          badge: { text: "Waiting", tone: "warning" },
        };
      }
      return {
        label: "Approval",
        tone: state === "approved" ? "success" : "danger",
        title:
          state === "approved"
            ? `A person approved: ${capabilityLabel(approval[2])}`
            : state === "rejected"
              ? `A person rejected: ${capabilityLabel(approval[2])}`
              : `Nobody decided in time: ${capabilityLabel(approval[2])}`,
        badge: {
          text: humanizeIdentifier(state),
          tone: state === "approved" ? "success" : "danger",
        },
      };
    }

    // A connector capability, executed (or refused) by the platform.
    if (capability) {
      const action = `${capabilityLabel(capability)} ${target(args)}`.trim();
      if (failed) {
        const reason = String(d.error ?? event.message);
        const refused = /requires approval/.test(reason);
        return {
          label: "Action",
          tone: "danger",
          title: `Not done: ${action}`,
          detail: refused
            ? /expired/.test(reason)
              ? "It needed approval and nobody decided in time."
              : "It needed approval and was rejected."
            : oneLine(reason, 200),
          badge: { text: refused ? "Blocked" : "Failed", tone: "danger" },
        };
      }
      const decision = typeof d.decision === "string" ? d.decision : null;
      if (decision === "deny") {
        return {
          label: "Action",
          tone: "danger",
          title: `Refused: ${action}`,
          detail: typeof d.reason === "string" ? `${d.reason.charAt(0).toUpperCase()}${d.reason.slice(1)}.` : undefined,
          badge: { text: "Blocked", tone: "danger" },
        };
      }
      return {
        label: "Action",
        tone: "action",
        title: action,
        detail: argumentSummary(args),
        badge:
          decision === "allow"
            ? { text: d.effect === "read" ? "Allowed (read only)" : "Allowed", tone: "success" }
            : decision
              ? { text: humanizeIdentifier(decision), tone: "neutral" }
              : undefined,
      };
    }

    // Something the agent did inside its own workspace.
    if (tool) {
      const described = describeTool(tool, record(d.input));
      const isQuestion = tool.toLowerCase() === "question";
      return {
        label: isQuestion ? "Question" : "Tool",
        tone: isQuestion ? "warning" : failed ? "danger" : "plain",
        title: described.title,
        detail: described.detail ?? (failed && !isQuestion ? oneLine(d.output ?? d.error ?? "", 200) || undefined : undefined),
        badge: failed && !isQuestion ? { text: "Failed", tone: "danger" } : undefined,
      };
    }

    return { label: "Tool", tone: failed ? "danger" : "plain", title: event.message };
  }

  if (event.kind === "status") {
    const status = describeStatus(event.message);
    return { label: "Run", tone: status.tone ?? "plain", title: status.title };
  }

  if (event.kind === "error") {
    const status = describeStatus(event.message);
    if (status.title !== event.message) {
      return { label: "Run", tone: "danger", title: status.title };
    }
    const code = typeof d.code === "string" ? d.code : null;
    const codeTitle =
      code === "worker_oom"
        ? "Ended: the worker was OOMKilled"
        : code === "worker_evicted"
          ? "Ended: the worker pod was evicted"
          : code === "worker_exited"
            ? "Ended: the worker container exited"
            : code === "worker_lost"
              ? "Ended: the worker was lost"
              : null;
    return { label: "Run", tone: "danger", title: codeTitle ?? event.message };
  }

  return {
    label: event.kind === "log" ? "Log" : humanizeIdentifier(event.kind) || "Event",
    tone: failed ? "danger" : event.level === "warn" ? "warning" : "muted",
    title: event.message,
  };
}

/**
 * The default view: the run's story. Everything a person would retell is in:
 * what the agent said and did, what was allowed or refused, how it ended.
 * Left for "Everything": debug logs, metrics and bookkeeping about tool discovery.
 */
export function isKeyEvent(event: ExecutionEventRecord): boolean {
  if (event.kind === "status" || event.kind === "error" || event.kind === "output") return true;
  if (event.kind === "tool_call") {
    const d = data(event);
    if (d.capability || /^approval /.test(event.message)) return true;
    const tool = typeof d.tool === "string" ? d.tool.toLowerCase() : "";
    if (!tool) return event.level !== "debug";
    if (tool === "todowrite") return false;
    if (tool === "execute") {
      // A script that only wraps connector calls duplicates the Action lines.
      const code = String(record(d.input).code ?? "");
      return !/tools\.\w+\.\w+\s*\(/.test(code) && !/\bsearch\s*\(/.test(code);
    }
    return true;
  }
  if (event.kind === "log") return event.level === "warn" || event.level === "error";
  return false;
}

/** The last thing the agent said, for runs that end without a result. */
export function lastAgentMessage(events: ExecutionEventRecord[]): string | null {
  for (let index = events.length - 1; index >= 0; index -= 1) {
    if (events[index].kind === "output" && events[index].message.trim()) return events[index].message;
  }
  return null;
}

/** Provider and model the run was dispatched with, when the worker recorded them. */
export function dispatchedModel(events: ExecutionEventRecord[]): string | null {
  for (const event of events) {
    if (event.kind !== "status" || !/^execution dispatched/.test(event.message)) continue;
    const d = data(event);
    const parts = [d.provider, d.model].filter((part): part is string => typeof part === "string" && part.length > 0);
    return parts.length ? parts.join(" / ") : null;
  }
  return null;
}
