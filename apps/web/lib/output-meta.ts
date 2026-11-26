import type { OutputDelivery, OutputInput, OutputView } from "./outputs";

/** Pure helpers for showing outputs; no runtime imports, so `node --test` can load it. */

export type OutputKind = OutputView["kind"];

export const OUTPUT_EVENTS = ["succeeded", "failed", "cancelled"] as const;

export const OUTPUT_KINDS: Array<{ kind: OutputKind; label: string; description: string }> = [
  { kind: "slack", label: "Slack", description: "Post to a channel through a Slack incoming webhook." },
  { kind: "mattermost", label: "Mattermost", description: "Post to a channel through a Mattermost incoming webhook." },
  { kind: "telegram", label: "Telegram", description: "Send a Bot API message to a chat or channel topic." },
  { kind: "webhook", label: "Webhook", description: "POST JSON with the event, the run and its text to any URL." },
  { kind: "s3", label: "S3 object storage", description: "Upload the rendered run result to Amazon S3 or RustFS." },
  { kind: "smtp", label: "Email (SMTP)", description: "Email the rendered run result through an SMTP server." },
];

export function kindLabel(kind: OutputKind): string {
  return OUTPUT_KINDS.find((item) => item.kind === kind)?.label ?? kind;
}

/** "succeeded, failed" → "Succeeded or failed". */
export function eventsLabel(events: string[]): string {
  const ordered = OUTPUT_EVENTS.filter((event) => events.includes(event));
  if (!ordered.length) return "Never: no events chosen";
  if (ordered.length === OUTPUT_EVENTS.length) return "Every finished run";
  const text = ordered.length === 1 ? ordered[0] : `${ordered.slice(0, -1).join(", ")} or ${ordered[ordered.length - 1]}`;
  return text.charAt(0).toUpperCase() + text.slice(1);
}

export type OutputState = "sending" | "failing" | "off" | "incomplete";

type Meta = {
  label: string;
  tone: "success" | "neutral" | "warning" | "danger" | "info";
  shape: "solid" | "slash" | "alert" | "cross" | "dashed" | "orbiting" | "paused";
};

/** A delivery that ended without the message arriving. */
export function deliveryFailed(delivery?: Pick<OutputDelivery, "status"> | null): boolean {
  return delivery?.status === "failed" || delivery?.status === "dead";
}

/**
 * Whether an output will send: missing what it needs, off, on but its last delivery
 * did not arrive, or on.
 */
export function outputState(
  output: Pick<OutputView, "enabled" | "hasWebhook" | "events" | "lastDelivery">,
): OutputState {
  if (!output.hasWebhook || output.events.length === 0) return "incomplete";
  if (!output.enabled) return "off";
  return deliveryFailed(output.lastDelivery) ? "failing" : "sending";
}

export const OUTPUT_STATUS: Record<OutputState, Meta> = {
  sending: { label: "On", tone: "success", shape: "solid" },
  failing: { label: "Failing", tone: "danger", shape: "cross" },
  off: { label: "Off", tone: "neutral", shape: "slash" },
  incomplete: { label: "Cannot send", tone: "warning", shape: "alert" },
};

/** Status of one delivery, in the words of the output page. */
export function deliveryStatus(delivery: Pick<OutputDelivery, "status" | "attempts">): Meta {
  switch (delivery.status) {
    case "succeeded":
      return { label: "Delivered", tone: "success", shape: "solid" };
    case "failed":
      return { label: "Refused", tone: "danger", shape: "cross" };
    case "dead":
      return { label: "Gave up", tone: "danger", shape: "alert" };
    case "sending":
      return { label: "Sending", tone: "info", shape: "orbiting" };
    case "skipped":
      return { label: "Skipped", tone: "neutral", shape: "slash" };
    case "pending":
      return delivery.attempts > 0
        ? { label: "Retrying", tone: "warning", shape: "paused" }
        : { label: "Queued", tone: "neutral", shape: "dashed" };
    default:
      return { label: delivery.status, tone: "neutral", shape: "dashed" };
  }
}

/** Why a delivery did not arrive, short enough for a list row: "HTTP 404" or the error. */
export function deliveryReason(delivery: Pick<OutputDelivery, "responseStatus" | "error" | "status">): string | null {
  if (delivery.status === "succeeded") return null;
  if (delivery.responseStatus) return `HTTP ${delivery.responseStatus}`;
  return delivery.error?.trim() || null;
}

/** What is missing before an output can send, or null. */
export function outputProblem(output: Pick<OutputView, "kind" | "hasWebhook" | "events">): string | null {
  if (!output.hasWebhook) {
    if (output.kind === "telegram") return "No bot token";
    if (output.kind === "smtp") return "No SMTP credential";
    return "No webhook URL";
  }
  if (output.events.length === 0) return "No events chosen";
  return null;
}

/** The body of an update that changes nothing but what is given. The webhook URL is left out, which keeps it. */
export function outputInput(output: OutputView, patch: Partial<OutputInput> = {}): OutputInput {
  return {
    name: output.name,
    kind: output.kind,
    agentId: output.agentId ?? null,
    teamId: output.teamId ?? null,
    events: output.events,
    config: output.config ?? {},
    ...(output.kind === "telegram"
      ? { chatId: output.chatId ?? "", messageThreadId: output.messageThreadId ?? null }
      : {}),
    template: output.template ?? null,
    enabled: output.enabled,
    ...patch,
  };
}
