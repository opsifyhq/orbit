import type { components } from "./schema";

type TeamRunDetail = components["schemas"]["TeamRunDetail"];
type ConnectorBinding = components["schemas"]["ConnectorBinding"];

// Compile-time contract test: these are shapes the API actually sends. Assigning them to
// the generated types catches documentation drift when `pnpm api:generate` is next run.
export const teamRunDetail: TeamRunDetail = {
  run: {
    id: "018f6c9e-6c0a-7000-8000-000000000001",
    orgId: "018f6c9e-6c0a-7000-8000-000000000002",
    teamId: "018f6c9e-6c0a-7000-8000-000000000003",
    status: "running",
    input: { task: "fix the failing test" },
    createdAt: "2026-11-26T10:00:00Z",
    updatedAt: "2026-11-26T10:00:00Z",
  },
  steps: [
    {
      id: "018f6c9e-6c0a-7000-8000-000000000005",
      runId: "018f6c9e-6c0a-7000-8000-000000000001",
      orgId: "018f6c9e-6c0a-7000-8000-000000000002",
      stepIndex: 1,
      name: "fix",
      agentId: "018f6c9e-6c0a-7000-8000-000000000006",
      agentVersionId: "018f6c9e-6c0a-7000-8000-000000000007",
      skills: [],
      status: "queued",
      input: { task: "fix the failing test" },
      createdAt: "2026-11-26T10:00:00Z",
      updatedAt: "2026-11-26T10:00:00Z",
      attempt: 1,
      activatedBy: [{ edge: "l1", step: 0 }],
    },
  ],
};

export const connectorBinding: ConnectorBinding = {
  connector: "github",
  connectionId: "018f6c9e-6c0a-7000-8000-000000000008",
  capabilities: ["issues.list"],
};
