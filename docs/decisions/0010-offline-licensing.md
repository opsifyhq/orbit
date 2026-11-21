# 0010 — Offline licensing and entitlements

Status: **Accepted**. Scope: the licensing part of #15 / #25.

## Constraints

- Must work fully **airgapped**: no activation call, no phone-home, no JWKS/OCSP fetch.
- Self-hosted installation must keep working when the license expires (read/export/backup)
  and must never terminate running executions because of licensing.
- Licensing is commercial gating, not a security boundary. Sandboxing still comes from
  worker isolation.

## Decisions

1. **Installation-wide license.** One signed license per ORBIT installation. Limits are
   evaluated installation-wide. An optional `orgSlug` claim may pin a license to one
   organization, but multi-org licensing is not required for v1.
2. **No installation ID by default.** The LiteLLM model: the customer receives a signed
   license file with the purchase and references it at install time. No exchange, no
   activation, no outbound request. (An optional `installationId` claim can be added later
   for a specific customer; it is not part of the default flow.)
3. **Offline signed file.** Compact JWS/JWT, `alg = EdDSA` (Ed25519), `kid` in the header.
   Public verification keys are compiled into `orbit-api`; the private signing key stays
   vendor-side (offline/HSM) and never ships.
4. **Premium features:** custom roles, resource-level permissions, shared connections,
   quotas, marketplace. **Free:** authentication (OIDC/SSO, group mapping), core agents,
   executions, workers, audit and exports. Authentication is never paywalled.
5. **Limits:** `seats`, `agents`, `workers`, `executions_per_day`. Limit checks happen on
   create/provision; warnings at 80%, hard reject at 100%. Limits are local counters, not
   reported anywhere.
6. **Grace:** 30 days after `expiresAt`. After grace, premium **writes/configuration** are
   blocked; reads, exports, backups, logins and running executions continue.
7. **Renewal:** replace the license Secret/file; the API hot-reloads the mounted file
   (~60s) without a restart. Helm/env-only delivery requires a Pod restart.
8. **API-only enforcement.** Workers, launcher and adapters never verify the license, so
   the execution plane keeps running during renewal or expiry.
9. **A license never grants permissions.** Entitlements unlock features; RBAC still
   decides who may use them.

## License payload

```json
{
  "v": 1,
  "id": "lic_01J...",
  "customer": "Acme",
  "plan": "enterprise",
  "orgSlug": "acme",
  "features": [
    "custom_roles",
    "teams",
    "resource_permissions",
    "shared_connections",
    "quotas",
    "marketplace"
  ],
  "limits": {
    "seats": 50,
    "agents": 500,
    "workers": 25,
    "executions_per_day": 10000
  },
  "issued_at": "2026-11-01T00:00:00Z",
  "expires_at": "2027-11-01T00:00:00Z",
  "grace_days": 30,
  "trial": false
}
```

Header: `{"alg":"EdDSA","kid":"orbit-2026-01"}`.

Do not include secrets, credentials, telemetry, or per-pod values. `orgSlug` is optional.
`installationId` is explicitly **not** in the default format.

## Airgapped workflow

1. Customer deploys ORBIT; no license means the community tier.
2. Customer stores the signed license in a Secret:

```yaml
license:
  existingSecret: orbit-license
  key: license.jwt
```

or mounts `/etc/orbit/license.jwt`, or sets `ORBIT_LICENSE` for Docker/Compose.

3. API verifies locally with embedded public keys and exposes status via `/api/v1/meta`
   and Settings (customer, plan, features, expiry, days remaining, grace).
4. Renewal: vendor signs a new file offline, customer replaces the Secret; the API
   hot-reloads it. No network access is required at any point.

`installationId` exchange is only needed if a customer explicitly requests binding; it is
a separate, optional workflow.

## Enforcement

- `LicenseStatus { tier, features, limits, customer, expires_at, days_remaining, grace }`
  is loaded at startup and on file change.
- Handler checks are explicit, e.g. `entitlements.require(Feature::CustomRoles)`.
- Quotas are checked where the resource is created and use local counters.
- Invalid or missing license → community tier; startup never fails.
- After grace, premium mutations return `402` with a renewal hint; reads/exports/backups
  and running executions remain available. Existing premium resources stay readable but
  immutable.
- License changes and grace transitions are audited. The license is not logged.

## Key management

- Ed25519 public keys compiled in by `kid`; support overlapping keys for rotation.
- Reject unknown `kid`, unknown `alg`, malformed payloads and invalid signatures.
- Vendor signing tool is an offline CLI, not part of the runtime image.
- Offline revocation is not practical; use annual/quarterly expiry. A signed revocation
  file can be added later for customers who can receive files.

## Testing

- Verifier unit tests with a disposable keypair: valid, expired, in-grace, tampered,
  unknown kid, wrong alg, missing claims.
- Entitlement tests: community default, feature gating, seat/agent/worker/execution
  limits, grace read-only behaviour.
- Airgap test: no network calls during startup or enforcement.
- Hot-reload test: replace the file and observe the new status.

## Implementation phases

1. `orbit-security::license` verifier + fixtures (Ed25519, `kid`, expiry/grace).
2. API startup/file reload, `/api/v1/meta` status, Settings license panel.
3. Gate the #15 features and enforce limits with explicit checks.
4. Helm `license.*`, docs for airgapped install/renew, `ORBIT_LICENSE`.
5. Offline vendor signing CLI and test license fixtures.

## Explicit non-goals

- No online activation or heartbeat.
- No hardware/MAC fingerprint binding; no DR-hostile checks.
- No obfuscation of the license check; signature plus honest licensing is the mechanism.
- No license data in prompts, worker events or telemetry by default.
