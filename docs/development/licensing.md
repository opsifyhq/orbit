# Offline licensing

The design is in [`docs/decisions/0010-offline-licensing.md`](../decisions/0010-offline-licensing.md).
This page is the operator quickstart.

## Install a license (airgapped)

The license is an Ed25519-signed JWS token. No installation ID, activation call or
network access is required.

```bash
kubectl -n orbit create secret generic orbit-license \
  --from-file=license.jwt=/path/to/orbit-license.jwt

helm upgrade orbit deploy/helm/orbit -n orbit --reuse-values \
  --set license.existingSecret=orbit-license \
  --set license.key=license.jwt
```

The chart mounts it at `/etc/orbit-license.jwt` and sets `ORBIT_LICENSE_FILE`. For
Docker/Compose mount the file and set `ORBIT_LICENSE_FILE`, or set `ORBIT_LICENSE` to the
token (inline values are not recommended for production).

## Renew

1. Replace the key in the Secret
   (`kubectl -n orbit create secret ... --dry-run=client -o yaml | kubectl apply -f -`).
2. Restart the API (`kubectl -n orbit rollout restart deployment/orbit-api`). The chart
   mounts the licence file with `subPath`, and Kubernetes does not update subPath mounts
   when a Secret changes, so the running pod keeps the old file until it is replaced.

The API can reload a licence file without a restart (`LicenseStatus::reload`), but that
only helps when the file is mounted as a directory, as in a custom Compose or Kubernetes
setup. With the chart, always restart. The same applies to an inline value
(`license.value` / `ORBIT_LICENSE`): restart the API or run `helm upgrade`.

## Status

`GET /api/v1/meta` includes:

```json
"license": {
  "tier": "enterprise",
  "customer": "Acme",
  "features": ["custom_roles", "quotas"],
  "availableFeatures": ["custom_roles"],
  "limits": { "seats": 50, "agents": 500 },
  "expiresAt": "2027-11-01T00:00:00Z",
  "daysRemaining": 350,
  "grace": false,
  "graceEndsAt": "2027-12-01T00:00:00Z",
  "valid": true
}
```

Without a license the installation runs as `community`. An expired license keeps working
through `graceDays`, then premium writes are rejected while reads, exports, backups and
running executions continue.

## Vendor signing tool

The offline signer lives in `services/license-cli` and is not part of the runtime images:

```bash
cargo build -p orbit-license-cli

# 1. Generate a signing key once; keep key.json offline.
./target/debug/orbit-license-cli keygen --out key.json --kid orbit-2026-01

# 2. Write claims.json (v, customer, plan, features, limits, issued_at, expires_at, ...).
# 3. Sign.
./target/debug/orbit-license-cli sign --key key.json --claims claims.json --out license.jwt

# 4. Show the public key entry for the build/runtime.
./target/debug/orbit-license-cli public-key --key key.json
```

Add the printed public key to `crates/orbit-security/license-public-keys.json` as
`{"kid":"base64url"}` and rebuild. This file is public by design and is compiled into
release images. Release builds ignore `ORBIT_LICENSE_PUBLIC_KEYS`; the env var is only read
by debug builds for local testing. Never commit `key.json`.

## Local test license (debug builds only)

To exercise licensed features locally (access teams, sharing, custom roles) without the
vendor key, sign a throwaway license with your own key. This only works against a **debug**
API build, because release builds ignore `ORBIT_LICENSE_PUBLIC_KEYS`.

```bash
cargo build -p orbit-license-cli -p orbit-api-server --bin orbit-api --bin orbit-license-cli
mkdir -p /tmp/orbit-license && cd /tmp/orbit-license   # outside the repo
../path/to/target/debug/orbit-license-cli keygen --out key.json --kid orbit-local-test
cat > claims.json <<'JSON'
{ "v": 1, "id": "local-test", "customer": "Local test", "plan": "enterprise", "trial": true,
  "features": ["custom_roles", "teams"],
  "limits": { "seats": 50, "agents": 500, "workers": 25, "executions_per_day": 10000 },
  "issued_at": "2026-11-19T00:00:00Z", "expires_at": "2027-11-19T00:00:00Z" }
JSON
orbit-license-cli sign --key key.json --claims claims.json --out license.jwt

# Run the debug API with it (plus your usual ORBIT_* settings):
ORBIT_LICENSE_FILE=$PWD/license.jwt \
ORBIT_LICENSE_PUBLIC_KEYS="$(orbit-license-cli public-key --key key.json)" \
  target/debug/orbit-api
```

`GET /api/v1/license` should report `"tier": "enterprise"`. Delete `key.json` when done and
never commit it or add its public key to `license-public-keys.json`.

## Verification keys

```json
// crates/orbit-security/license-public-keys.json
{ "orbit-2026-01": "base64url-raw-ed25519-public-key" }
```

The object may contain multiple keys during rotation. An unknown `kid` is rejected.

Public keys are embedded in the API by `kid`. Development builds include a documented dev
key; production builds must add the vendor public key. `ORBIT_LICENSE_PUBLIC_KEYS` accepts
a JSON object of `kid -> base64` for testing/custom builds.

The private signing key is vendor-side only (offline/HSM) and must never be committed.

## What a licence enables today

The signed `features` list can name six flags. Three are advertised as available today and
gated by the API; the remaining three are accepted and verified for forward compatibility
but are not advertised as shipped product features.

| Feature | Enforced for | Advertised |
|---|---|---|
| `custom_roles` | Creating, changing and deleting custom roles, and assigning one to a user | Yes |
| `teams` | Access teams and their members (`/api/v1/teams`) and OIDC group-to-team sync | Yes |
| `resource_permissions` | Changing a resource's visibility (`/api/v1/sharing/...`) | Yes |
| `shared_connections` | Not enforced yet | No |
| `quotas` | Not enforced yet | No |
| `marketplace` | Not enforced yet | No |

`settings:write` custom role creation/update/deletion and custom-role assignment require
`custom_roles`; access-team writes require `teams`. Reserved flags stay in the raw `features`
array so existing gates keep working, but they are omitted from `availableFeatures`, which is
what the API/UI advertises. A request that needs a feature the licence lacks is refused with
`402 license_required`.

Reading shared resources, exports, running executions and everything in the community
tier stay available without a licence.

Numeric `limits` in a licence are enforced too. Counts are installation-wide (all
organizations), and the check and insert for each counted resource share a Postgres advisory
lock so concurrent API replicas cannot race past a limit. The lock is only taken when the
licence sets that limit and is held for the count and the insert only (a worker launch drops
it before the launcher call). When a limit is reached the API
answers `402` with code `license_limit`:

| Limit | Counted as | Checked when |
|---|---|---|
| `seats` | Active users in all orgs | A new user is auto-provisioned at OIDC login (linking a pre-provisioned user by email uses no seat) |
| `agents` | Non-archived agents in all orgs | An agent is created |
| `workers` | Running worker launches in all orgs | A worker is launched (manual, team or dispatcher) |
| `executions_per_day` | Executions in the last 24h in all orgs | An execution or team step is started |

A licence without a limit leaves that resource unlimited. `graceEndsAt` lets the UI show a
positive grace countdown when `daysRemaining` has gone negative.
