// What an approval request shows of a tool call's arguments. The worker keeps arguments up to
// 64 KiB whole; larger ones arrive as a string holding the head and the tail with the left-out
// middle named, plus `argumentsBytes` and `argumentsTruncated`. Approvals created by older
// workers carry a string cut at 4000 bytes and no flag: any string is treated as truncated.

export type ApprovalArguments = {
  /** The arguments as an object, for the summary rows; empty when truncated. */
  args: Record<string, unknown>;
  /** Text for the full-arguments view. */
  text: string;
  truncated: boolean;
  /** Size of the original serialised arguments, when the worker reported it. */
  bytes?: number;
};

export function approvalArguments(action: {
  arguments?: unknown;
  argumentsBytes?: unknown;
  argumentsTruncated?: unknown;
}): ApprovalArguments {
  const bytes = typeof action.argumentsBytes === "number" ? action.argumentsBytes : undefined;
  const raw = action.arguments;
  if (typeof raw === "string") {
    return { args: {}, text: raw, truncated: true, bytes };
  }
  const args =
    raw && typeof raw === "object" && !Array.isArray(raw) ? (raw as Record<string, unknown>) : {};
  const text = raw === undefined || raw === null ? "" : JSON.stringify(raw, null, 2);
  return { args, text, truncated: action.argumentsTruncated === true, bytes };
}

/** The warning shown above truncated arguments; null when they are complete. */
export function truncationNotice(value: ApprovalArguments): string | null {
  if (!value.truncated) return null;
  const size = value.bytes === undefined ? "" : `, ${value.bytes.toLocaleString("en-US")} bytes`;
  return `This call's arguments are too long to show in full${size}. Only the start and the end are shown below; approving allows the whole call, including the part left out.`;
}

/** A summary-row value, cut with a visible marker when it is longer than `limit` characters. */
export function rowValue(value: unknown, limit = 1000): string {
  const text = String(value);
  return text.length <= limit ? text : `${text.slice(0, limit)}… (truncated; see Full arguments)`;
}
