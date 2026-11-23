# ORBIT Helm chart

Deploys the ORBIT control plane (API + web) and the worker launcher, with optional
bundled PostgreSQL and NATS.

Platform workers run through the launcher: the API mints a scoped worker token per
launch and revokes it on stop, so a normal install needs **no worker token**. Fixed
worker Deployments are opt-in and are only needed when you run a worker outside that
path.

## Prerequisites

- Kubernetes 1.27+ and `kubectl`
- Helm 3.12+
- A reachable image registry containing the ORBIT images (`imageRegistry`), plus
  `imagePullSecrets` if it is private

## Install

```bash
helm upgrade --install orbit deploy/helm/orbit \
  --namespace orbit --create-namespace \
  --set ingress.host=orbit.example.com \
  --set auth.acknowledgeInsecureDev=true
```

This installs the API, web, launcher and (by default) bundled PostgreSQL and NATS with
dev auth. On first start the API seeds one launchable mock profile
(`Platform worker (mock)`); start it from **Workers → Launch** and the demo agents can
run. Point `ingress.host` at your ingress address (or add an `/etc/hosts` entry).

`auth.acknowledgeInsecureDev=true` acknowledges the anonymous Owner UI exposed by dev
auth plus ingress; use it only on throwaway local/test clusters. For anything reachable
beyond your LAN, switch to `auth.mode=oidc` as in the Production section below.

Verify:

```bash
kubectl -n orbit get pods,svc,ingress
kubectl -n orbit logs deploy/orbit-api --tail=50
curl http://orbit.example.com/api/v1/healthz
```

Custom/external workers (your own cluster, laptop or Docker host) still need a token from
`POST /api/v1/worker-tokens` or **Workers → tokens**. That flow is documented in
`docs/development/deployment.md`.

## Production

Start from the example and adjust it to your environment:

```bash
helm upgrade --install orbit deploy/helm/orbit -n orbit \
  -f deploy/helm/orbit/values-prod.example.yaml
```

Checklist:

- `platform.environment=production` **and** `auth.mode=oidc` (the API refuses dev auth in
  production).
- `secrets.create=false` with `secrets.existingSecret` containing `database-url`,
  `master-key`, `launcher-token` (when the launcher is enabled), the OIDC client secret,
  and any fixed-worker tokens.
- `externalDatabase.enabled=true` / `externalNats.enabled=true` for managed services.
  The database Secret can be an operator-managed one (see below), so the password does
  not have to appear in values.
- `ingress.tls.enabled=true` with a certificate Secret.
- `nodeSelector` / `tolerations` / `affinity` for your node pools.

## Upgrade and uninstall

```bash
helm upgrade orbit deploy/helm/orbit -n orbit -f my-values.yaml
helm history orbit -n orbit
helm rollback orbit 1 -n orbit

helm uninstall orbit -n orbit
# bundled data lives in PVCs; delete them explicitly for a clean slate
kubectl -n orbit delete pvc -l app.kubernetes.io/instance=orbit
```

Migrations run automatically when the API starts (`api.autoMigrate`, which sets
`ORBIT_DB_AUTO_MIGRATE`) and are append-only. They take an advisory lock, so several API
replicas are safe.

## Values reference

### Images

`imageRegistry` must include the repository segment (`ghcr.io/opsifyhq/orbit` renders
`ghcr.io/opsifyhq/orbit/orbit-api:<tag>`). Component tags default to the chart
`appVersion`.

| Key | Default | Notes |
|---|---|---|
| `imageRegistry` | `ghcr.io/opsifyhq/orbit` | registry + repository prefix |
| `imagePullPolicy` | `IfNotPresent` | |
| `imagePullSecrets` | `[]` | registry credentials; the first entry is also used for launcher-created pods |
| `api.image` / `api.tag` | `orbit-api` / appVersion | |
| `web.image` / `web.tag` | `orbit-web` / appVersion | |
| `worker.image` / `worker.tag` | `orbit-worker` / appVersion | |
| `opencodeWorker.image` / `.tag` | `orbit-worker-opencode` / appVersion | |
| `piWorker.image` / `.tag` | `orbit-worker-pi` / appVersion | |
| `launcher.image` / `.tag` | `orbit-launcher` / appVersion | |

### Control plane

| Key | Default | Notes |
|---|---|---|
| `api.replicas` | `1` | stateless; scale freely |
| `api.port` | `8080` | |
| `web.replicas` | `1` | |
| `web.port` | `3000` | |
| `platform.environment` | `development` | `production` forbids dev auth |
| `platform.publicBaseUrl` | `http://orbit.example.com` | used in OIDC redirects |
| `platform.corsOrigins` | `http://orbit.example.com` | |
| `platform.seedDemo` | `true` | demo agents on first start |
| `platform.seedWorkerProfile` | `true` | one launchable mock profile when a launcher is configured |
| `platform.logFormat` | `json` | `text` for local |
| `platform.bodyLimitBytes` | `2097152` | |
| `api.trustedProxies` | private ranges | Proxies whose `X-Forwarded-For` the anonymous rate limit believes; narrow to the pod CIDR when clients connect from private ranges |
| `platform.archiveAfterDays` | `30` | archive terminal runs older than N days; `0` disables |
| `platform.auditRetentionDays` | `0` | delete audit log entries older than N days; `0` (default) keeps forever |

### Database and bus

| Key | Default | Notes |
|---|---|---|
| `postgresql.enabled` | `true` | bundled single-replica PostgreSQL (test clusters) |
| `postgresql.database` / `.user` / `.password` | `orbit` / `orbit` / `orbit` | |
| `postgresql.storage` / `.storageClassName` | `5Gi` / `""` | |
| `externalDatabase.enabled` | `false` | use a managed PostgreSQL instead of the bundled one |
| `externalDatabase.url` | `""` | complete connection URL (ignored when `existingSecret` is set) |
| `externalDatabase.existingSecret` | `""` | operator-managed Secret with the connection fields |
| `externalDatabase.urlKey` | `""` | optional key holding a full URL (CNPG app Secrets include `uri`) |
| `externalDatabase.hostKey` / `.portKey` / `.databaseKey` / `.userKey` / `.passwordKey` | `host` / `port` / `dbname` / `user` / `password` | key mapping for the connection Secret |
| `externalDatabase.sslMode` | `require` | `sslmode` for the composed URL |
| `nats.enabled` | `true` | bundled NATS JetStream |
| `nats.storage` / `.storageClassName` | `2Gi` / `""` | |
| `externalNats.enabled` / `.url` | `false` / `""` | required when enabled |
| `secrets.databaseUrl` | `""` | overrides the computed database URL |

### External PostgreSQL (CNPG and other operators)

With `externalDatabase.enabled=true` you can either pass a full URL or point the chart at
a Secret that already holds the connection fields. CloudNativePG application Secrets work
directly:

```yaml
externalDatabase:
  enabled: true
  existingSecret: orbit-pg-app     # CNPG application Secret in the release namespace
  # Use the pre-composed URL if you prefer:
  # urlKey: uri

# If this Secret only holds the database fields, keep secrets.create=true or provide
# `master-key` (+ `launcher-token`) via secrets.existingSecret.
```

When `urlKey` is empty the API composes the URL from `host`, `port`, `dbname`, `user` and
`password` (percent-encoded), with `externalDatabase.sslMode`. Key names are configurable
via `hostKey`, `portKey`, `databaseKey`, `userKey` and `passwordKey`. The chart then omits
`database-url` from its own Secret.

### Workers

Platform workers are normally launched from profiles (Deployments for live profiles,
Jobs for
single executions). The settings below only apply to fixed Deployments.

| Key | Default | Notes |
|---|---|---|
| `worker.enabled` | `false` | fixed worker Deployment |
| `worker.token` | `""` | required when `worker.enabled=true` |
| `worker.replicas` | `1` | |
| `worker.concurrency` | `4` | concurrent executions per worker |
| `worker.harnesses` | `mock` | comma-separated harness kinds |
| `worker.pollIntervalMs` | `500` | |
| `worker.schedulerEnabled` | `true` | not read by current workers; schedules run in the API, which claims each fire atomically |
| `worker.workspace.sizeLimit` | `1Gi` | |
| `opencodeWorker.enabled` | `false` | fixed OpenCode CLI worker |
| `opencodeWorker.token` | `""` | required when enabled |
| `opencodeWorker.auth.existingSecret` | `""` | Secret with `opencode auth export` output |
| `opencodeWorker.config.existingConfigMap` | `""` | optional `opencode.json` ConfigMap |
| `piWorker.enabled` | `false` | fixed Pi CLI worker |
| `piWorker.token` | `""` | required when enabled |
| `piWorker.auth.existingSecret` | `""` | Secret with the Pi agent directory |

### Launcher

| Key | Default | Notes |
|---|---|---|
| `launcher.enabled` | `true` | Launch/Stop/Logs in the UI; default worker path |
| `launcher.token` | `""` | internal API↔launcher token; generated and reused from the release Secret when empty |
| `launcher.authSecrets` | `orbit-opencode-auth*,orbit-pi-auth*` | harness auth Secrets a worker profile may mount (`authSecretName`): exact names or `prefix*`, comma-separated; enforced by the API and the launcher |
| `launcher.imagePullSecret` | `""` | registry secret for launched pods; defaults to the first `imagePullSecrets` entry |
| `launcher.admissionPolicy.enabled` | `true` | ValidatingAdmissionPolicies limiting what the launcher ServiceAccount may create (Kubernetes 1.30+, cluster-scoped) |

The launcher gets namespaced RBAC for Pods/Jobs only; the API never receives cluster
credentials. Because RBAC cannot limit `create` by name or content, the chart adds
admission policies so the launcher can create only `orbit-worker-<launch>` workloads and
Opaque `-env` Secrets, and only pods that mount the allowed harness auth Secrets (see
`docs/development/kubernetes-launcher.md`). Set `launcher.token` explicitly (or provide `launcher-token` in
`secrets.existingSecret`) when the rendered chart is committed or diffed by GitOps,
because `helm template` cannot look up the stored token.

### License

| Key | Default | Notes |
|---|---|---|
| `license.existingSecret` | `""` | Secret with the signed license file (airgapped) |
| `license.key` | `license.jwt` | key inside that Secret |
| `license.value` | `""` | inline token for dev/test (not recommended) |

Mount at `/etc/orbit-license.jwt`; the API hot-reloads it (~60s) and exposes the status via
`/api/v1/meta`. No installation ID or outbound call is used.

### Authentication and secrets

| Key | Default | Notes |
|---|---|---|
| `auth.mode` | `dev` | `dev` or `oidc` |
| `auth.acknowledgeInsecureDev` | `false` | allow dev + anonymous + ingress; throwaway clusters only |
| `auth.dev.allowAnonymous` | `true` | omit tokens (dev only) |
| `auth.dev.token` | `orbit-dev-token` | injected server-side by the web proxy |
| `auth.dev.userEmail` / `.userName` / `.orgSlug` / `.orgName` | dev values | seeded identity |
| `auth.oidc.issuer` / `.audience` / `.jwksUrl` | `""` | required in OIDC mode |
| `auth.oidc.clientId` | audience | web login client |
| `auth.oidc.redirectUri` | `<publicBaseUrl>/auth/callback` | |
| `auth.oidc.scopes` | `openid profile email` | |
| `auth.oidc.autoProvision` / `.defaultRole` | `true` / `viewer` | |
| `auth.oidc.roleClaim` | `groups` | IdP claim with group/role values |
| `auth.oidc.emailClaim` | `email` | unique identity claim; use `name` for shared emails |
| `auth.oidc.groupRoles` | `{}` | group → role mapping, e.g. `orbit-admins: admin` |
| `auth.oidc.teamGroups` | `{}` | group → `team-slug:role`, e.g. `platform: platform:member` |
| `auth.oidc.autoCreateTeams` | `false` | create missing access teams for mapped group slugs |
| `auth.oidc.requireMappedGroup` | `false` | deny users whose groups match no role/team mapping |
| `auth.oidc.syncRoles` | `false` | re-evaluate the stored role on every login |
| `auth.oidc.requireVerifiedEmail` | `true` | link a first login to an invited user by email only when the token has `email_verified=true` |
| `secrets.create` / `secrets.existingSecret` | `true` / `""` | bring your own Secret in production |
| `secrets.masterKey` | dev key | base64 32-byte AES key; replace for real deployments |
| `secrets.oidcClientSecret` | `""` | confidential clients only when the chart creates the Secret |
| `vault.enabled` / `.addr` / `.existingSecret` | `false` / `""` / `""` | optional Vault wiring |

With `secrets.existingSecret`, the API reads the optional `oidc-client-secret` key from
that Secret in OIDC mode (no inline `secrets.oidcClientSecret` needed). Fixed workers
likewise read `worker-token`, `opencode-worker-token` or `pi-worker-token` from it when
enabled; the chart only writes tokens into a Secret it creates itself.

OIDC can also be configured at runtime under **Settings → OIDC / SSO**; stored values are
applied on the next API start.

### Networking and reliability

| Key | Default | Notes |
|---|---|---|
| `ingress.enabled` | `true` | |
| `ingress.host` | `orbit.example.com` | |
| `ingress.ingressClassName` | `nginx` | |
| `ingress.tls.enabled` / `.secretName` | `false` / `orbit-tls` | |
| `ingress.annotations` | SSE-friendly nginx | keep `proxy-buffering: off` |
| `caCertificates.enabled` / `.secretName` / `.key` | `false` / `""` / `ca-bundle.crt` | private CA bundle for every workload |
| `serviceAccount.create` / `.name` / `.annotations` | `true` / `""` / `{}` | namespaced, no cluster-admin |
| `podAnnotations` | `{}` | |
| `extraEnv` | `[]` | extra env for backend workloads |
| `podSecurityContext` / `containerSecurityContext` | non-root, read-only root | |
| `nodeSelector` / `tolerations` / `affinity` | `{}` / `[]` / `{}` | |
| `terminationGracePeriodSeconds` | `30` | |
| `nameOverride` / `fullnameOverride` | `""` | |

### Monitoring (Prometheus)

The API serves Prometheus metrics at `/metrics`; the ingress never routes it. Metric
catalogue and examples: `docs/development/observability.md`.

| Key | Default | Notes |
|---|---|---|
| `monitoring.annotations` | `false` | `prometheus.io/*` annotations on the API pods and Service; cannot be combined with `monitoring.token` |
| `monitoring.port` | `0` | dedicated metrics port (e.g. `9090`); `0` serves `/metrics` on `api.port`. When set, `api.port` stops serving `/metrics` |
| `monitoring.token.existingSecret` / `.key` | `""` / `token` | Secret (release namespace) with the bearer token scrapers must send |
| `monitoring.serviceMonitor.enabled` | `false` | Prometheus Operator `ServiceMonitor`; needs the `monitoring.coreos.com/v1` CRDs |
| `monitoring.serviceMonitor.namespace` / `.labels` | `""` / `{}` | release namespace by default (required with a token); add the label your Prometheus selects on |
| `monitoring.serviceMonitor.interval` / `.scrapeTimeout` | `30s` / `10s` | |
| `monitoring.serviceMonitor.scheme` / `.tlsConfig` | `http` / `{}` | |
| `monitoring.serviceMonitor.relabelings` / `.metricRelabelings` | `[]` / `[]` | |
| `monitoring.allowFrom` | `[]` | with `networkPolicy.enabled` and `monitoring.port`: peers allowed to reach the metrics port (empty = any) |

## Troubleshooting

| Symptom | Fix |
|---|---|
| `ImagePullBackOff` | `imageRegistry` must include the repository segment; add `imagePullSecrets` for private registries |
| `worker.token is required` at render time | a fixed worker is enabled without a token: set `worker.token` or drop `worker.enabled=true` |
| API exits with `ORBIT_AUTH_MODE=dev is not allowed in production` | use `platform.environment=development`, or configure OIDC |
| `launcher_unavailable` in the UI | `launcher.enabled=false`, or `launcher-token` missing/mismatched in the Secret |
| Executions stay `queued` | no worker for that harness/placement; launch a profile or check `GET /api/v1/workers` |
| SSE stalls behind a proxy | keep the chart's nginx annotations (`proxy-buffering: off`) |
| Harness auth failures | the auth Secret is missing or stale; recreate it and restart the worker |
