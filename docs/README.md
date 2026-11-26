# ORBIT Documentation

ORBIT is a self-hosted control plane for persistent AI agents: it stores agent
definitions, connects them to models and tools, runs them on workers you control and
enforces who may do what. This handbook is served by your own installation, so it works
offline.

The sections follow the app's sidebar. Each one documents the screens of the same name.

## Get started

- **[How ORBIT fits together](guide/README.md)**: the main concepts and where each is documented.
- **[Run ORBIT locally](development/local.md)**: a working installation in a few minutes.

## Use ORBIT

| Section | What it covers |
|---|---|
| **Run work** | [Start work](guide/start-work.md), [executions](guide/executions.md) and [approvals](guide/approvals.md) |
| **Build** | [Agents](guide/agents.md), [agent teams](guide/agent-teams.md), [skills](guide/skills.md), [tool policies](guide/tool-policies.md) and [export and import](guide/portable-export-import.md) |
| **Connect** | [Connectors](guide/connections.md), [model providers](guide/providers.md), [outputs](guide/outputs.md), [CLI tools](guide/cli-tools.md) and the [Kubernetes connector](guide/kubernetes-connector.md), [Exchange on-premises connector](guide/exchange-connector.md) |
| **Infrastructure** | [Workers and profiles](guide/worker-profiles.md) and the [Kubernetes launcher](development/kubernetes-launcher.md) |
| **Admin** | [Settings](guide/settings.md), [access teams](guide/access-teams.md), [roles and permissions](guide/rbac.md), [single sign-on](development/oidc.md) and [licensing](guide/licensing.md) |

## Run ORBIT in production

- **[Deployment](development/deployment.md)**: Docker Compose and Kubernetes (Helm).
- **[Installing a license](development/licensing.md)**: offline license install.
- **[Releasing](development/releasing.md)**: version bump, tag, images and chart.
- **[Security](security/threat-analysis.md)**: threat model and the [access matrix](security/idor-matrix.md).

## Understand and extend ORBIT

- **[Architecture](architecture/overview.md)**: components, contracts and [diagrams](architecture/diagram.md).
- **[Contributing](development/testing.md)**: testing, the [UI test plan](development/ui-test-plan.md) and the [design system](development/design-system.md).
- **[Decision records](decisions/README.md)**: why ORBIT is built the way it is.
