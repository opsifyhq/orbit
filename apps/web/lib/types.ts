import type { components } from "@orbit/api-client";

export type AgentSummary = components["schemas"]["AgentSummary"];
export type AgentDetail = components["schemas"]["AgentDetail"];
export type AgentVersionView = components["schemas"]["AgentVersionView"];
export type ExecutionRecord = components["schemas"]["ExecutionRecord"];
export type ExecutionEventRecord = components["schemas"]["ExecutionEventRecord"];
export type HarnessView = components["schemas"]["HarnessView"];
export type ProviderView = components["schemas"]["ProviderView"];
export type ConnectorView = components["schemas"]["ConnectorView"];
export type ConnectionView = components["schemas"]["ConnectionView"];
export type TriggerRecord = components["schemas"]["Trigger"];
export type DashboardResponse = components["schemas"]["DashboardResponse"];
export type MeResponse = components["schemas"]["MeResponse"];
export type MetaResponse = components["schemas"]["MetaResponse"];
export type AgentPage = components["schemas"]["AgentPage"];
export type ExecutionPage = components["schemas"]["ExecutionPage"];
export type ApiTokenView = components["schemas"]["ApiTokenView"];
export type CreatedApiToken = components["schemas"]["CreatedApiToken"];
export type Organization = components["schemas"]["Organization"];
export type User = components["schemas"]["User"];
export type AgentDefinition = components["schemas"]["AgentDefinition"];
export type AgentSpec = components["schemas"]["AgentSpec"];
export type AgentInstanceView = components["schemas"]["AgentInstanceView"];
export type ProviderProbeResponse = components["schemas"]["ProviderProbeResponse"];
export type ApprovalWithExecution = components["schemas"]["ApprovalWithExecution"];
export type WorkerView = components["schemas"]["WorkerView"];
export type Skill = components["schemas"]["Skill"];
export type WorkerProfile = components["schemas"]["WorkerProfile"];
export type WorkerBootstrap = components["schemas"]["WorkerBootstrap"];
// Generated from the API schema so the UI cannot drift from the server contract.
export type WorkSource = components["schemas"]["WorkSourceDefinition"] & {
  provider: "github" | "gitlab";
};
export type TeamRepository = components["schemas"]["RepositorySpec"];

// The API schema types these JSON columns as `unknown`; the UI reads known keys from
// them, so narrow just those fields while every other field comes from the generated schema.
export type AgentTeam = Omit<components["schemas"]["AgentTeam"], "definition"> & {
  definition: {
    mode?: "pipeline" | "native";
    instructions?: string;
    repository?: string | null;
    branch?: string | null;
    workSource?: WorkSource | null;
    repositories?: TeamRepository[];
    model?: { provider?: string | null; model?: string | null; management?: string | null } | null;
    workerProfileId?: string | null;
    autoLaunch?: boolean;
    skills?: string[];
    harness?: string | null;
    connectionIds?: string[];
    envMappingIds?: string[];
    timeoutSeconds?: number | null;
    steps?: Array<{ agentId: string; model?: { provider?: string | null; model?: string | null; management?: string | null } | null }>;
  };
};

export type TeamRun = Omit<components["schemas"]["TeamRun"], "input"> & {
  input: Record<string, unknown>;
};

export type TeamStep = Omit<components["schemas"]["TeamStep"], "input"> & {
  input: Record<string, unknown>;
};

export type TeamRunDetail = { run: TeamRun; steps: TeamStep[] };

export type WorkerLaunch = components["schemas"]["WorkerLaunch"];
