import type { OutputInput, OutputView } from "./outputs";

/** Pure helpers for showing outputs; no runtime imports, so `node --test` can load it. */

export type OutputKind = OutputView["kind"];

export const OUTPUT_EVENTS = ["succeeded", "failed", "cancelled"] as const;

export const OUTPUT_KINDS: Array<{ kind: OutputKind; label: string; description: string }> = [
  { kind: "slack", label: "Slack", description: "Post to a channel through a Slack incoming webhook." },
  { kind: "mattermost", label: "Mattermost", description: "Post to a channel through a Mattermost incoming webhook." },
  { kind: "webhook", label: "Webhook", description: "POST JSON with the event, the run and its text to any URL." },
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

export type OutputState = "sending" | "off" | "incomplete";

/** Whether an output will send: on, off, or missing what it needs to send. */
export function outputState(output: Pick<OutputView, "enabled" | "hasWebhook" | "events">): OutputState {
  if (!output.hasWebhook || output.events.length === 0) return "incomplete";
  return output.enabled ? "sending" : "off";
}

export const OUTPUT_STATUS: Record<OutputState, { label: string; tone: "success" | "neutral" | "warning"; shape: "solid" | "slash" | "alert" }> = {
  sending: { label: "On", tone: "success", shape: "solid" },
  off: { label: "Off", tone: "neutral", shape: "slash" },
  incomplete: { label: "Cannot send", tone: "warning", shape: "alert" },
};

/** What is missing before an output can send, or null. */
export function outputProblem(output: Pick<OutputView, "hasWebhook" | "events">): string | null {
  if (!output.hasWebhook) return "No webhook URL";
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
    template: output.template ?? null,
    enabled: output.enabled,
    ...patch,
  };
}
