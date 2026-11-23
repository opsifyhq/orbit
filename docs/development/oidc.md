# OIDC login (Dex / Keycloak)

ORBIT uses standard OIDC discovery, Authorization Code + PKCE and JWKS validation. No
provider-specific code is required beyond the issuer URL and client registration.

## Required configuration

| Setting | Env | Example |
|---|---|---|
| Auth mode | `ORBIT_AUTH_MODE` | `oidc` |
| Issuer | `ORBIT_OIDC_ISSUER` | `https://dex.example.com` |
| Audience | `ORBIT_OIDC_AUDIENCE` | `orbit` |
| Client ID | `ORBIT_OIDC_CLIENT_ID` | `orbit` |
| Client secret | `ORBIT_OIDC_CLIENT_SECRET` | optional for public PKCE clients |
| Redirect URI | `ORBIT_OIDC_REDIRECT_URI` | `http://orbit.example.com/auth/callback` |
| Scopes | `ORBIT_OIDC_SCOPES` | `openid profile email groups` |
| JWKS | `ORBIT_OIDC_JWKS_URL` | issuer keys URL (required) |
| Auto-provision | `ORBIT_OIDC_AUTO_PROVISION` | `true` (create users on first login) |
| Default role | `ORBIT_OIDC_DEFAULT_ROLE` | `viewer` |
| Role claim | `ORBIT_OIDC_ROLE_CLAIM` | `groups` |
| Identity/email claim | `ORBIT_OIDC_EMAIL_CLAIM` | `email` (or `name`) |
| Group mappings | `ORBIT_OIDC_GROUP_ROLES` | `orbit-admins=admin,orbit-operators=operator` (or JSON) |
| Sync roles | `ORBIT_OIDC_SYNC_ROLES` | `false` |
| Team mappings | `ORBIT_OIDC_TEAM_GROUPS` | see [oidc-team-sync.md](oidc-team-sync.md) |
| Auto-create teams | `ORBIT_OIDC_AUTO_CREATE_TEAMS` | `false` |
| Require a mapped group | `ORBIT_OIDC_REQUIRE_MAPPED_GROUP` | `false` |
| Require verified email | `ORBIT_OIDC_REQUIRE_VERIFIED_EMAIL` | `true` (see [Linking by email](#linking-by-email)) |
| Clock skew | `ORBIT_OIDC_CLOCK_SKEW_SECONDS` | `60`; seconds of tolerance when checking token times |

`ORBIT_OIDC_ISSUER`, `ORBIT_OIDC_AUDIENCE` and `ORBIT_OIDC_JWKS_URL` are required in oidc
mode; the API refuses to start without them. `ORBIT_OIDC_CLIENT_ID` defaults to the
audience, and `ORBIT_OIDC_REDIRECT_URI` defaults to `http://localhost:3000/auth/callback`.
The redirect URI must be registered exactly at the provider.

The API fetches:

```text
<issuer>/.well-known/openid-configuration
```

for `authorization_endpoint`, `token_endpoint` and the optional `end_session_endpoint`.
The browser never receives the client secret.

The discovery document is cached for 10 minutes and refreshed at most once every 15
seconds; if a refresh fails, the cached document is still served. The JWKS is cached for
10 minutes as well. An unknown `kid` or an expired cache triggers at most one refresh per
30 seconds, and a failed refresh keeps the cached keys. Unauthenticated requests therefore
cannot be turned into a stream of calls to your identity provider.

## Dex example

Dex static client:

```yaml
staticClients:
  - id: orbit
    name: ORBIT
    secret: optional-for-confidential-client
    redirectURIs:
      - http://orbit.example.com/auth/callback
```

Set:

```text
ORBIT_OIDC_ISSUER=https://dex.example.com
ORBIT_OIDC_AUDIENCE=orbit
ORBIT_OIDC_CLIENT_ID=orbit
ORBIT_OIDC_REDIRECT_URI=http://orbit.example.com/auth/callback
ORBIT_OIDC_JWKS_URL=https://dex.example.com/keys
```

Dex access tokens may be opaque. ORBIT uses the `id_token` from the token response as the
browser bearer token and validates it against JWKS.

## Keycloak example

Create an OpenID Connect client:

- Client ID: `orbit`
- Client authentication: off for public PKCE client, or on with a client secret
- Standard flow: enabled
- Valid redirect URI: `http://orbit.example.com/auth/callback`
- Web origins: `http://orbit.example.com`

Issuer:

```text
https://keycloak.example.com/realms/<realm>
```

Set the same variables, with `ORBIT_OIDC_JWKS_URL` set to the realm's certificate
endpoint (`<issuer>/protocol/openid-connect/certs`). ORBIT does not derive the JWKS URL
from discovery. ORBIT’s API validates the JWT audience against `ORBIT_OIDC_AUDIENCE` and
auto-provisions users when enabled.

## Group-based roles

ORBIT can map IdP groups to built-in roles. Add the claim and mappings:

```yaml
auth:
  oidc:
    scopes: "openid profile email groups"
    defaultRole: viewer
    roleClaim: groups
    groupRoles:
      orbit-admins: admin
      orbit-operators: operator
    syncRoles: true
```

- The IdP must include the claim in the ID token (Dex connector group support or a
  Keycloak group-membership mapper).
- The most privileged matching role wins (`owner > admin > operator > viewer`).
- `syncRoles=false` applies the mapped role only when a user is first auto-provisioned.
- `syncRoles=true` updates an existing user's stored role on login only when the token
  has groups **and** at least one matches `groupRoles`; the role then becomes the highest
  mapped one. A token without the claim, or whose groups match no mapping, leaves the
  stored role (including manually assigned and custom roles) unchanged. A matching mapping
  does override manual role changes in ORBIT.
- Only groups listed in `groupRoles` grant anything. `defaultRole` is used only when a new
  user is auto-provisioned without a matching group; it is never applied by sync.

Equivalent environment variables: `ORBIT_OIDC_ROLE_CLAIM`,
`ORBIT_OIDC_GROUP_ROLES` (`group=role,group=role` or JSON), `ORBIT_OIDC_SYNC_ROLES`.

Group mappings target built-in roles. Custom roles (`docs/development/rbac.md`) are
assigned manually for now; group-to-custom-role mapping is a follow-up.

OIDC groups can also add users to access teams; see
[oidc-team-sync.md](oidc-team-sync.md).

## Linking identities to existing users

A user is bound to one OIDC identity `(issuer, subject)`. On the first login of an unknown
identity, ORBIT matches the token's email (see `ORBIT_OIDC_EMAIL_CLAIM`) against existing
users:

- No user has that email: a new user is auto-provisioned (when `autoProvision` is on).
- The user has no OIDC identity yet (an admin invited them): the identity is linked to
  that user.
- The user is already bound to a different identity: the login is refused with `403`
  ("this email is already linked to another sign-in identity") and nothing changes. An
  admin must resolve it, e.g. by removing the old user.
- `email_verified: false` in the token: the identity is never linked by email (`403`).
- `email_verified` absent: the identity is not linked by email either (`403`), because
  `requireVerifiedEmail` is on by default.

### Linking by email

`requireVerifiedEmail` (`ORBIT_OIDC_REQUIRE_VERIFIED_EMAIL`, Helm
`auth.oidc.requireVerifiedEmail`) defaults to `true`: a first login is linked to an
existing user only when the token carries `email_verified: true`. Without it, an IdP that
omits the claim and lets users choose their email would let anyone sign in as an invited
user.

> **Upgrade note.** Before this default changed, a missing `email_verified` claim allowed
> linking. If your IdP never sends the claim, invited users now get `403` on their first
> login. Either make the IdP send `email_verified` (preferred), or set
> `ORBIT_OIDC_REQUIRE_VERIFIED_EMAIL=false` (Helm `auth.oidc.requireVerifiedEmail: false`)
> if the IdP does not let users choose their email. The switch under **Settings → OIDC /
> SSO** can only turn the requirement on; it cannot turn off what the environment sets.
> Users that are already linked are not affected, and new users are still
> auto-provisioned.

## Configure from the UI

Once the platform is reachable, OIDC can also be configured under **Settings → OIDC / SSO**
(`GET`/`PUT /api/v1/settings/oidc`). Values are stored in `platform_settings`, merged over
the environment configuration in OIDC mode, and applied when the API starts — restart the
API after saving. `ORBIT_OIDC_CLIENT_SECRET` is never stored by the UI; keep
confidential-client secrets in the deployment Secret.

`PUT /api/v1/settings/oidc` is a merge: only the fields present in `settings` change, and
absent fields (role claim, `groupRoles`, `teamGroups`, `autoCreateTeams`,
`requireMappedGroup`, `syncRoles`) keep their stored value. The merged result is validated
before it is stored (422 and no change on error). To clear a mapping, send it explicitly,
for example `"groupRoles": {}`.

## Web flow

```text
/login
  → GET /api/v1/auth/oidc/config
  → redirect to authorization_endpoint with PKCE
  → /auth/callback
  → POST /api/v1/auth/oidc/token { code, codeVerifier }
  → API exchanges code at token_endpoint
  → browser stores id_token as orbit.access_token
  → API requests use Authorization: Bearer <id_token>
```

`GET /api/v1/auth/oidc/config` returns 404 when OIDC is off and 502 when the discovery
document cannot be read. It also returns `endSessionEndpoint` when the provider
advertises one, and the token response includes `refresh_token` and `expires_in`. The web
app uses neither: logout only clears the local token, and the session ends when the
`id_token` expires. Provider end-session logout and refresh-token rotation are not
implemented in the web app.

## Helm

```yaml
auth:
  mode: oidc
  oidc:
    issuer: https://dex.example.com
    audience: orbit
    clientId: orbit
    redirectUri: http://orbit.example.com/auth/callback
    jwksUrl: https://dex.example.com/keys
    scopes: "openid profile email groups"
    autoProvision: true
    defaultRole: viewer
    roleClaim: groups
    emailClaim: email
    groupRoles:
      orbit-admins: admin
    syncRoles: false
    requireVerifiedEmail: true
    # teamGroups, autoCreateTeams and requireMappedGroup: see oidc-team-sync.md
secrets:
  oidcClientSecret: ""
```

For a public PKCE client leave `oidcClientSecret` empty. For a confidential client set
it; the API adds it to the server-side token exchange.
