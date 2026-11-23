# Connectors

Connectors is where you authorise external services for your agents. A **connector** is a
kind of service ORBIT knows about (GitHub, GitLab, Kubernetes and so on) and the
capabilities it offers. A **connection** is one account you have authorised for a
connector: a name, a credential and the capabilities you allow. Agents use a connection
only for the capabilities you allow, and ORBIT enforces that in the platform, not in the
prompt. Open **Connectors** in the Connect group of the sidebar (`/connections`).

## Which connectors work today

| Connector | State |
|---|---|
| GitHub | Works through ORBIT's tool broker: issues, pull requests, checks, checkout, commit and push |
| GitLab | Works through the tool broker: projects, group issues and boards, issues, merge requests, pipelines, checkout, commit and push |
| Kubernetes | Works through the tool broker; read tools plus restart/delete recovery actions; see [Kubernetes connector](kubernetes-connector.md) |
| X | `posts.search` hydrates recent posts as untrusted context before a run; `posts.create` publishes posts and replies through the tool broker with a user-context token. See [X connector](x-connector.md) |
| Mock | A stand-in for development and tests that talks to nothing |
| Telegram | Listed with `messages.list` and `messages.send`, but the worker has no implementation for it yet, so an agent cannot use it |
| Generic webhook | Listed with `request.send`, but the worker has no implementation for it yet. A connection can be created and tested, not used by an agent |

Connections for GitHub, GitLab and Kubernetes also feed the CLI tokens described in
[CLI tools](cli-tools.md).

## Capabilities and effects

Every capability has an effect, shown on the connection form:

- **Read only** (`read`): looks things up; nothing changes.
- **Change data** (`write`): creates or updates things in the service.
- **Act outside ORBIT** (`external_side_effect`): sends messages, pushes branches, opens or
  merges requests and other effects people notice.

The effect, together with the agent's autonomy level and approval mode, decides whether a
call is allowed, needs [approval](approvals.md) or is denied. A capability that is not
allowed on the agent's binding is refused whatever the agent is told.

## Create a connection

To create one, choose **New connection**. Connection Studio shows one section at a time:

1. **Service**: pick the connector. It cannot be changed after creation.
2. **Account**: a **Name** (shown when choosing a connection for an agent or team), a
   "GitLab URL" or "Webhook URL" where the connector needs one, the **Credential**, and
   the connector's own fields (for GitHub, for example, `allowedRepositories` and
   `defaultRepository`; for GitLab `allowedProjects`, `groupId` and `allowedGroups`; for
   Kubernetes the `namespaces` and `allowedResources` allowlists, where an empty list
   denies, see [Kubernetes connector](kubernetes-connector.md)). Group tools on GitLab may
   only use `groupId` and `allowedGroups`.
3. **Capabilities**: tick what agents may do, grouped as Read only, Change data and Act
   outside ORBIT, with **Allow all** or **Clear** per group. With nothing ticked the form
   warns "Nothing allowed yet": agents cannot use the connection.
4. **Advanced**: the raw configuration as JSON. Never put secrets there; use the credential.
5. **Access control**: owner team and visibility ([access teams](access-teams.md)).

Credentials are write-only. The credential is stored encrypted and is never shown again.
When you edit a connection, a stored credential shows as stored; leave the field empty to
keep it or type a new one to replace it. Connections can also be imported and exported
without credentials ([Export and import](portable-export-import.md)).

## Use a connection in an agent

To give an agent a connection, open Agent Studio, go to **Connectors and skills**, tick the
connector, choose the **Connection**, and tick the **Allowed capabilities**. A binding
without a connection is refused when you save. Capabilities that are not ticked are denied.
Agent teams can also attach connections for all their steps
([Agent teams](agent-teams.md)).

## Test a connection

A connection page (`/connections/<id>`) has **Test**, **Edit**, **Export as YAML** and
**Remove connection**. **Test** contacts the service with the stored credential:

- GitHub and GitLab: a request to `<baseUrl>/user`.
- Generic webhook: a `HEAD` request to the URL.
- Kubernetes: lists the namespaces the credential can see.
- Mock: always passes.
- Any other connector: "ORBIT cannot test <connector> connections yet". The credential is
  checked the first time an agent uses it, and the status does not change.

The result is advisory. A failed test marks the connection "Check failed" and shows "The
last check failed", but agents can still use it; fix the URL or credential, then test
again. A passing test shows "Connected". A new connection starts as "Not checked". Only
a connection whose status is `expired` or `revoked` is refused when an agent tries to use
it, and a test never changes those two statuses. Test
requests go through the outbound network policy
([Providers](providers.md#outbound-network-policy)).

Testing needs the `connections:write` permission and use of the connection, because it
sends the stored credential. When you edit a connection's `baseUrl`, `cloneBaseUrl`, `url`
or Kubernetes `apiServer`, enter the credential again in the same save: ORBIT does not
send a stored credential to a new endpoint.

## X search vs posting auth

X recent search (`posts.search`) works with an app-only bearer token. Publishing
(`posts.create`) does not: posting and replies require a **user-context** token with the
`tweet.write` scope (OAuth 2.0 user context or OAuth 1.0a user context). Store the user
access token as the connection credential; if X returns 401/403, ORBIT reports that a
user-context token with `tweet.write` is required and never echoes the credential. See
[X connector](x-connector.md) for setup and the opt-in live check.

## Find connections

The list has a search field, a filter (All, Connected, Check failed, Not checked), a status
column, bulk access changes, **Import** and **New connection**.

## API

- `GET /api/v1/connectors`: the connector catalog with capabilities and effects.
- `GET/POST /api/v1/connections`, `PATCH/DELETE /api/v1/connections/{id}`: listing needs
  `connections:read`; creating and changing need `connections:write` plus manage access.
- `POST /api/v1/connections/{id}/test`: returns `supported`, `ok`, `latencyMs` and a
  `message`. `supported: false` means the connector has no probe.
