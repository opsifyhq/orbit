# Licensing for ORBIT owners

ORBIT runs as **community** by default. An enterprise license unlocks premium features and
raises usage limits. Licensing is **offline**: no activation call, no installation ID and
no outbound request. The license is a signed file that ORBIT verifies locally.

## Community vs enterprise

| | Community | Enterprise |
|---|---|---|
| Agents, executions, workers | Yes | Yes |
| OIDC / SSO and group mapping | Yes | Yes |
| Audit log, exports, backups | Yes | Yes |
| Custom roles | — | `custom_roles` |
| Multi-team access (teams, members, team ownership) | — | `teams` |
| Resource sharing (private / team / organization visibility) | — | requires the `resource_permissions` signed flag |
| Shared connections | — | planned |
| Quotas | — | planned |
| Marketplace | — | planned |

Only features that actually exist are gated. The verifier still accepts the signed flags
`shared_connections`, `quotas` and `marketplace` for forward
compatibility, but they are **not advertised as available**: the API returns them in the raw
`features` list and exposes only shipped flags in `availableFeatures`.

Enterprise licenses also carry limits: **seats, agents, workers, executions per day**.
Limits apply to the whole installation (all organizations), not per organization.

## What a license looks like

A small signed token (Ed25519 JWS) containing:

```json
{
  "customer": "Acme",
  "plan": "enterprise",
  "features": ["custom_roles", "teams"],
  "limits": { "seats": 50, "agents": 500, "workers": 25, "executions_per_day": 10000 },
  "issued_at": "2026-11-01T00:00:00Z",
  "expires_at": "2027-11-01T00:00:00Z",
  "grace_days": 30
}
```

It contains no secrets and no hardware/installation binding. Keeping a copy is fine; it is
not a credential.

## Install a license

Create a Kubernetes Secret and point Helm at it:

```bash
kubectl -n orbit create secret generic orbit-license \
  --from-file=license.jwt=/path/to/orbit-license.jwt

helm upgrade orbit deploy/helm/orbit -n orbit --reuse-values \
  --set license.existingSecret=orbit-license \
  --set license.key=license.jwt
```

The chart mounts the file at `/etc/orbit-license.jwt`. For Docker/Compose, mount the file
and set `ORBIT_LICENSE_FILE`, or set `ORBIT_LICENSE` to the token. If `ORBIT_LICENSE_FILE`
is unset, ORBIT reads `/etc/orbit/license.jwt` when that file exists. When both
`ORBIT_LICENSE` and a file are present, the inline token is used.

No restart is needed when the license is delivered as a mounted file: ORBIT re-reads it
about every 60 seconds. If you use `license.value` / `ORBIT_LICENSE`, restart the API or
run a Helm upgrade.

## Renew a license

1. Receive the new signed file from the vendor.
2. Replace the Secret key:

```bash
kubectl -n orbit create secret generic orbit-license \
  --from-file=license.jwt=/path/to/new-license.jwt \
  --dry-run=client -o yaml | kubectl apply -f -
```

3. Wait ~1 minute. The API picks it up automatically.

## Expiry and grace

- Before `expires_at`: everything works.
- During the **grace period** (`grace_days`, default 30): premium features still work; the
  UI shows a positive "Grace ends in X days" countdown instead of negative expiry days.
- After grace: premium **writes and configuration changes** are blocked, but reads,
  exports, backups, logins and **running executions continue**. Existing premium data is
  never deleted and remains readable.

A license problem never prevents ORBIT from starting.

## Limits

| Limit | Counted as | Checked when |
|---|---|---|
| `seats` | active users in all orgs | a new user is provisioned (e.g. first OIDC login); a first login that links a user an administrator already created uses no new seat |
| `agents` | non-archived agents in all orgs | creating an agent (including import) or re-activating an archived agent |
| `workers` | running worker launches in all orgs | launching a worker (manual, team or dispatcher) |
| `executions_per_day` | executions in the last 24h in all orgs | creating any execution: manual run, rerun, team step (including dispatcher auto-run), scheduled trigger or continuous runtime tick |

Checks and inserts share a database advisory lock, so concurrent API replicas cannot pass
the limit together. The lock and count are only taken when the license actually defines
the limit, and only for the count and the insert: a worker launch releases the lock once
its pending launch row exists (the row already counts), before it calls the launcher. Warnings appear at 80% in **Settings → Platform → License** and
`GET /api/v1/license`. At 100% the create is rejected with `402 license_limit`. Using a
feature the license does not include (for example creating an access team without `teams`)
is rejected with `402 license_required`.

A scheduled trigger or continuous runtime tick that reaches the daily execution limit is
**skipped**, not failed: the trigger stays enabled and the runtime keeps running. The skip
is written to the audit log as `execution.license_skipped` and logged by the API, then the
next tick tries again. The 24-hour count is served by the `executions_created_at_idx`
index.

## Where to see the status

- **Settings → Platform → License**: tier, customer, expiry, days/grace left, features,
  usage and warnings ([settings](settings.md)).
- `GET /api/v1/license` for scripts.

Usage counts span every organization of the installation, so only Owners and Admins see
them. Other roles with `settings:read` get the licence status without the `usage` block
and without the near-limit warnings that quote the counts.
- `GET /api/v1/meta` also includes a compact license block.

## If something is wrong

| Symptom | Check |
|---|---|
| Community despite a license | Is the Secret mounted? Does the pod have `ORBIT_LICENSE_FILE`? Is the key name correct? |
| “unknown license key” | The license `kid` is not in this build. Use a license issued for this release, or update ORBIT. |
| “license signature is invalid” | The file was edited or truncated; request a fresh license. |
| License not applied | Mounted file: wait ~60s. Inline env: restart the API. |
| Limits not enforced | A license that is fully expired behaves as community; limits only apply while valid. |

## Security model (short)

- The license is signed with the vendor’s **private** key; ORBIT only contains the matching
  **public** key. The private key is never shipped, committed or available in customer
  environments.
- Licensing is a commercial control, not a security boundary. It never replaces RBAC,
  sandboxing or approvals.
- Do not edit a license file; edits break the signature. Contact the vendor for changes.
- The full design is in [`docs/decisions/0010-offline-licensing.md`](../decisions/0010-offline-licensing.md).
  Vendor-side signing is documented in
  [`docs/development/licensing.md`](../development/licensing.md).
