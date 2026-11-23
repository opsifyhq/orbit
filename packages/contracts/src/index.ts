import { z } from "zod";

/**
 * Shared agent-definition validation for the web app (and a future mobile app).
 * Only schemas with a consumer are exported; building blocks stay private.
 * Keep these aligned with `crates/orbit-domain` (camelCase JSON, orbit.io/v1).
 */

const harnessKindSchema = z.enum(["mock", "opencode", "claude_code", "pi", "generic"]);

const executionModeSchema = z.enum(["on_demand", "long_running"]);
const placementTargetSchema = z.enum(["auto", "local", "docker", "kubernetes"]);
const modelManagementSchema = z.enum(["harness", "platform"]);

const cronSchema = z
  .string()
  .trim()
  .min(1, "cron expression is required")
  .refine((value) => value.split(/\s+/).length === 5, "use a 5-field cron expression");

const connectorBindingSchema = z.object({
  connector: z
    .string()
    .min(1, "connector is required")
    .regex(/^[a-z0-9][a-z0-9_-]{0,62}$/, "invalid connector slug"),
  connectionId: z.string().uuid().nullish(),
  capabilities: z.array(z.string()).default([]),
});

const triggerSchema = z.discriminatedUnion("type", [
  z.object({
    type: z.literal("schedule"),
    cron: cronSchema,
    timezone: z.string().optional(),
    /** What the agent is asked to do each time the schedule fires. */
    task: z.string().max(20000).optional(),
  }),
  // Wire name is snake_case (`secret_ref`): `TriggerSpec` only renames its variants.
  z.object({ type: z.literal("webhook"), secret_ref: z.string().nullish() }),
  z.object({ type: z.literal("event"), subject: z.string().min(1, "event subject is required") }),
  z.object({ type: z.literal("manual") }),
]);

const autonomySchema = z.object({
  level: z.number().int().min(1).max(4),
  approvalMode: z.enum(["auto", "always", "writes"]).default("auto"),
  maxExecutionsPerHour: z.number().int().min(1).max(10000).optional(),
});

const networkPolicySchema = z.object({
  mode: z.enum(["default", "allowlist", "denied"]).nullish(),
  allowedHosts: z.array(z.string()).default([]),
});

const resourceLimitsSchema = z.object({
  timeoutSeconds: z.number().int().min(1).max(86400),
  maxConcurrentExecutions: z.number().int().min(1).max(20),
  memoryMb: z.number().int().min(64).max(65536).optional(),
  cpuMillis: z.number().int().min(100).max(64000).optional(),
  maxOutputTokens: z.number().int().min(1).max(1_000_000).optional(),
  network: networkPolicySchema.nullish(),
});

export const agentSpecSchema = z.object({
  harness: z
    .object({
      type: harnessKindSchema,
      executionMode: executionModeSchema.default("on_demand"),
      placement: placementTargetSchema.default("auto"),
      config: z.record(z.string(), z.unknown()).default({}),
    })
    .optional(),
  model: z
    .object({
      management: modelManagementSchema.default("harness"),
      provider: z.string().optional(),
      model: z.string().optional(),
      params: z.record(z.string(), z.unknown()).default({}),
    })
    .default({ management: "harness", params: {} }),
  /**
   * Harness-specific options keyed by harness kind (`opencode`, `pi`, ...). Not edited by
   * the Agent Studio form, but it must round-trip: Zod strips unknown keys, so omitting it
   * here silently drops it on every UI save (#109).
   */
  harnessConfig: z.record(z.string(), z.unknown()).default({}),
  instructions: z.object({
    system: z.string().min(1, "system instructions are required").max(200_000),
    constraints: z.array(z.string()).default([]),
  }),
  connectors: z.array(connectorBindingSchema).default([]),
  skills: z.array(z.string()).default([]),
  triggers: z.array(triggerSchema).default([]),
  autonomy: autonomySchema.default({ level: 2, approvalMode: "auto" }),
  resources: resourceLimitsSchema.default({
    timeoutSeconds: 300,
    maxConcurrentExecutions: 1,
  }),
  memory: z
    .object({
      enabled: z.boolean().default(false),
      strategy: z.enum(["none", "short_term", "summary"]).default("none"),
      retentionDays: z.number().int().min(1).max(3650).optional(),
    })
    .default({ enabled: false, strategy: "none" }),
  /** Pre-filled task for manual runs (mirrors `RunDefaults` in orbit-domain). */
  runDefaults: z.object({ task: z.string().optional() }).optional(),
});

export const agentDefinitionSchema = z.object({
  apiVersion: z.literal("orbit.io/v1"),
  kind: z.literal("Agent"),
  metadata: z.object({
    name: z.string().trim().min(1, "name is required").max(120),
    description: z.string().max(2000).default(""),
    labels: z.record(z.string(), z.string()).default({}),
  }),
  spec: agentSpecSchema,
});
export type AgentDefinition = z.infer<typeof agentDefinitionSchema>;

export const AUTONOMY_LEVELS = [
  { level: 1, name: "Observe", description: "Read-only access to connected services." },
  { level: 2, name: "Recommend", description: "Analyse and propose actions; writes are blocked." },
  { level: 3, name: "Prepare", description: "Prepare writes and side effects; humans approve." },
  { level: 4, name: "Autonomous", description: "Execute allowlisted actions within policy." },
] as const;

export function emptyAgentDefinition(): AgentDefinition {
  return {
    apiVersion: "orbit.io/v1",
    kind: "Agent",
    metadata: { name: "", description: "", labels: {} },
    spec: {
      model: { management: "harness", params: {} },
      harnessConfig: {},
      instructions: { system: "", constraints: [] },
      connectors: [],
      skills: [],
      triggers: [],
      autonomy: { level: 2, approvalMode: "auto" },
      resources: { timeoutSeconds: 300, maxConcurrentExecutions: 1 },
      memory: { enabled: false, strategy: "none" },
    },
  };
}
