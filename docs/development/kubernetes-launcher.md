# Kubernetes worker launch authentication

## Overview

The `orbit-launcher` service is the only component with cluster privileges. It shells
out to `kubectl` to create and manage worker pods. The API never receives kubectl
or Docker socket access — it stores launch intent and calls the launcher over a
separate authenticated HTTP channel.

The launcher is the default way platform workers run (`launcher.enabled=true` in the Helm
chart). The API mints a scoped worker token per launch, hands it to the launcher, and
revokes it when the launch stops. Operators do not issue worker tokens for platform
workers; tokens exist for custom/external workers only.

## How authentication works

### 1. In-cluster service account (Helm chart, recommended)

When the launcher runs inside the cluster, `kubectl` uses the pod's mounted service
account token (`/var/run/secrets/kubernetes.io/serviceaccount/token`). The Helm chart
works this way: it creates a launcher ServiceAccount and the Role below, and mounts no
kubeconfig. No further setup is needed.

### 2. Kubeconfig (launcher outside the cluster)

When you run the launcher yourself outside the cluster, point `kubectl` at a kubeconfig:
mount it at `~/.kube/config` (for example `/home/orbit/.kube` in the launcher image) or
set `KUBECONFIG`. The chart does not do this.

## RBAC requirements

The launcher needs these permissions in the target namespace:

```yaml
rules:
- apiGroups: [""]
  resources: ["pods"]
  verbs: ["get", "list", "watch", "create", "delete"]
- apiGroups: [""]
  resources: ["pods/log"]
  verbs: ["get"]
- apiGroups: ["apps"]
  resources: ["deployments"]
  verbs: ["get", "list", "watch", "create", "delete"]
- apiGroups: ["batch"]
  resources: ["jobs"]
  verbs: ["get", "list", "watch", "create", "delete"]
- apiGroups: [""]
  resources: ["secrets"]
  verbs: ["create", "patch", "delete"]   # per-launch env Secret; no read access
```

The Helm chart creates a dedicated `ServiceAccount` and `Role`/`RoleBinding` for this.

## Image pull secrets

If your registry requires authentication:

```yaml
ORBIT_LAUNCHER_IMAGE_PULL_SECRET=registry-credentials
```

The launcher adds `imagePullSecrets` to every pod it creates.

## Key environment variables

| Variable | Default | Purpose |
|---|---|---|
| `ORBIT_LAUNCHER_TOKEN` | required | Bearer token the API uses to authenticate to the launcher. The Helm chart generates it and reuses it from the release Secret when `launcher.token` is empty; set the value explicitly for GitOps. |
| `ORBIT_LAUNCHER_ADDR` | `0.0.0.0:8090` | Listen address. |
| `ORBIT_LAUNCHER_NAMESPACE` | `orbit` | Target namespace for worker workloads. |
| `ORBIT_LAUNCHER_KUBECTL` | `kubectl` | Path to kubectl. |
| `ORBIT_LAUNCHER_DOCKER` | `docker` | Path to the Docker CLI (Docker launches). |
| `ORBIT_LAUNCHER_IMAGE_PULL_SECRET` | unset | Secret name for registry auth. |
| `ORBIT_LAUNCHER_IMAGE_ALIASES` | unset | Rewrite registry hosts, as comma-separated `from=to` pairs (for example `192.168.1.201:5000=registry.mdk.local:5000`). |
| `ORBIT_LAUNCHER_JOB_DEADLINE_SECONDS` | `7200` | Job deadline when the API sends no timeout. |
| `ORBIT_LAUNCHER_DOCKER_RETENTION` | `1h` | How long exited single-execution Docker containers are kept. |
| `ORBIT_MAX_APPROVAL_WAIT_SECONDS` | `3600` | Approval budget added to the Job deadline; set it identically on workers. |
| `ORBIT_WORKER_AUTH_SECRETS` | `orbit-opencode-auth*,orbit-pi-auth*` | Allowlist for `authSecretName`: comma-separated exact names or `prefix*` patterns (empty allows none). Read by both the API (rejects on save) and the launcher (refuses at launch). Helm: `launcher.authSecrets`. |
| `ORBIT_OPENCODE_AUTH_EXPORT` | unset | Path of the OpenCode auth export inside the launcher container (Docker launches). |
| `ORBIT_OPENCODE_AUTH_HOST_PATH` | unset | The same file as the Docker daemon sees it on the **host**; the daemon mounts this path into worker containers. |
| `ORBIT_PI_AGENT_DIR` | unset | Pi agent directory inside the launcher container (Docker launches). |
| `ORBIT_PI_AGENT_HOST_DIR` | unset | The same directory as the Docker daemon sees it on the host. |

## Security model

- The API stores launch intent and calls `POST /launches` on the launcher with a
  bearer token (`ORBIT_LAUNCHER_TOKEN`).
- The launcher validates the token, then runs `kubectl create -f -` with the workload
  manifest (Deployment for long-lived profiles, Job for single executions).
- Worker pods receive `ORBIT_WORKER_TOKEN` (a scoped worker token minted for that
  launch). The token and all worker-profile env values are stored in a per-launch Secret
  (`<workload>-env`, labelled `orbit.launch-id`) and referenced with `secretKeyRef`; they
  never appear as plain `value` in the Job/Deployment spec. The launcher sets the workload
  as the Secret's owner, so the Secret is garbage-collected when the workload is deleted
  (stop, relaunch, Job TTL). It needs only `create`/`patch`/`delete` on `secrets`.
  Secrets are only as private as your cluster's etcd: enable encryption at rest.
- A worker profile's `authSecretName` is mounted into the pod, so it is restricted to the
  installer's allowlist (`ORBIT_WORKER_AUTH_SECRETS` / `launcher.authSecrets`): a valid
  Kubernetes name that matches an allowed exact name or `prefix*`. The API rejects other
  names; the launcher refuses them again at launch (it cannot read Secrets, so the check is
  by name, not by label). Profiles saved before this rule that name another Secret fail to
  launch until the name is added to the allowlist or the profile is changed.
- Docker launches pass the same values through a 0600 `--env-file` that is deleted right
  after `docker run`; nothing secret is in argv. `docker inspect` on the host still shows
  container env, so protect Docker socket access.
- The token is revoked when the launch stops or fails, and, for single-execution
  launches, when the execution reaches a terminal state (the API checks every 15 s).
- Worker pods set `automountServiceAccountToken: false`; they have no Kubernetes API access.
- Optional NetworkPolicy: set `networkPolicy.enabled=true` in the Helm chart to allow
  ingress to the launcher (port 8090) only from the ORBIT API pods. It needs a CNI that
  enforces NetworkPolicy and is off by default. Egress is not restricted.
- The launcher never exposes kubectl or the Docker socket to the API or web app.

## Custom CA certificates

When `caCertificates.enabled=true`, the chart passes the CA Secret name/key to the API
(`ORBIT_CA_SECRET_NAME`, `ORBIT_CA_SECRET_KEY`). The launcher mounts that Secret into every
worker Pod/Job at `/etc/ssl/certs/ca-bundle.crt` and sets `SSL_CERT_FILE`,
`REQUESTS_CA_BUNDLE`, `NODE_EXTRA_CA_CERTS` and `GIT_SSL_CAINFO`. This covers harnesses that
use the system store (Rust workers), Node/Bun-based harnesses (OpenCode) and git, which in
the Debian-based worker images does not read `SSL_CERT_FILE`. The worker passes the two
`GIT_SSL_CA*` variables to the git commands it runs and to the harness process.

The worker pod needs read access to the Secret; the launcher itself does not read it.

## Launch status and lifecycle

Status is mapped by pure functions in `services/launcher/src/mapping.rs` from
`kubectl get deployments,jobs,pods -l orbit.launch-id=<id> -o json` (Kubernetes) or
`docker inspect` (Docker).

| Observed | Status | Message |
|---|---|---|
| Container terminated with `reason: OOMKilled` (Docker: `State.OOMKilled`) | `failed` | OOMKilled + memory limit |
| Exit code 137 without `OOMKilled` | `failed` | SIGKILL, explicitly not OOM |
| Deployment pod running after an earlier OOM restart | `running` | hint: OOMKilled earlier, restart count |
| CrashLoopBackOff, ImagePullBackOff, ErrImagePull, InvalidImageName, CreateContainerConfigError, CreateContainerError, RunContainerError | `failed` | reason, restart count, last termination |
| ContainerCreating / pending / not ready | `starting` | none |
| Job `Complete` | `stopped` | none |
| Job `Failed` condition (e.g. DeadlineExceeded, BackoffLimitExceeded) | `failed` | condition reason + message, container cause |
| Workload not found (stopped, or Job removed by TTL) | `stopped` | none |
| `kubectl get` itself fails | `starting` | read error (never terminal) |
| Docker `exited` code 0 / non-zero | `stopped` / `failed` | exit code |

The API only refreshes launches that are `pending`, `starting` or `running`, so a terminal
status it has already stored (for example `failed`) is never overwritten by a later
"not found" `stopped` after the Job TTL (300s) removes the Job. A Job that fails and
expires before the API polls it is still reported `stopped`; the API polls on every
launch read, and the TTL leaves a five-minute window.

Lifecycle:

- Single-execution Jobs set `activeDeadlineSeconds` to the execution timeout plus the
  approval budget plus 300s (the API sends `timeoutSeconds`). The approval budget is
  `ORBIT_MAX_APPROVAL_WAIT_SECONDS` (default 3600): the most time a run may spend paused
  for human approvals, whose wait does not count against the run timeout. The launcher
  forwards the value to the single-execution pod or container so the in-run broker
  enforces the same budget; set it identically on the worker. Without a timeout the launcher uses
  `ORBIT_LAUNCHER_JOB_DEADLINE_SECONDS` (default 7200).
- Single-execution Docker containers are not started with `--rm`, because status
  (exit code, `OOMKilled`) and logs are read after exit. They are labelled
  `orbit.single-execution=true` and exited ones older than `ORBIT_LAUNCHER_DOCKER_RETENTION`
  (default `1h`) are pruned on the next single-execution launch; `stop` still removes them.
- The launcher image runs as UID 10001. Under Compose the socket needs a group: set
  `DOCKER_GID` (`stat -c %g /var/run/docker.sock`) on Linux; the default `0` suits Docker Desktop.
- Worker pods have no health endpoint, so no probes are set on them.
