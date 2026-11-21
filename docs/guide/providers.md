# Providers and models

ORBIT supports three provider kinds:

| Kind | Auth | Use |
|---|---|---|
| `openai_compatible` | ORBIT stores an encrypted credential and injects it into the harness | vLLM, LiteLLM, gateways, OpenRouter-style endpoints |
| `anthropic` | ORBIT stores an encrypted Anthropic API key | Native Anthropic Messages API (`/v1/messages`) |
| `harness_managed` | The harness owns credentials (e.g. `opencode auth export`) | Provider inventories managed by OpenCode/Pi |

`anthropic` is resolved to the native Anthropic protocol: OpenCode uses
`@ai-sdk/anthropic` and Pi uses `anthropic-messages`. The base URL defaults to
`https://api.anthropic.com/v1`; model discovery probes `/models` with `x-api-key` and
`anthropic-version`. The key is still encrypted, write-only and sent only to that base URL.

> **Known issue:** a provider of kind `anthropic` cannot be saved yet. Creating one fails
> with a 500, because the database constraint `providers_kind_check` (migration 0007) only
> allows `openai_compatible` and `harness_managed`. Until a migration widens it, use an
> `openai_compatible` or `harness_managed` provider instead.

## Known OpenAI-compatible endpoints

These providers expose an OpenAI-compatible API and need no extra ORBIT adapter. On the
**Model providers** page, choose a preset in the provider form or enter the base URL
directly:

| Provider | Base URL | Credential |
|---|---|---|
| OpenRouter | `https://openrouter.ai/api/v1` | OpenRouter API key |
| DeepSeek | `https://api.deepseek.com` | DeepSeek API key |
| Google Gemini | `https://generativelanguage.googleapis.com/v1beta/openai` | Gemini API key |
| Azure OpenAI (v1) | `https://<resource>.openai.azure.com/openai/v1` | Azure OpenAI key |
| Anthropic (native Messages API) | `https://api.anthropic.com/v1` | Anthropic API key, kind `anthropic` (cannot be saved yet, see the known issue above) |

Model ids are not bundled: use the provider's `/models` discovery or type the id. Azure
v1 accepts the key as a bearer token; do not use the legacy deployment-specific URL unless
you know your provider endpoint expects it.

## Models

`config.settings.models` is the model catalog used by agent and worker selectors. In the
provider form, **Test and fetch** (and **Test** on a saved provider's card) probes
`/models` (when `compat.modelsEndpoint` is enabled), fills the catalog and default model,
and saved providers persist the discovered list.
Endpoints without `/models` can use a free-text model.

## Credentials and the base URL

The stored API key is only ever sent to the provider's `baseUrl`. To keep it that way,
**changing `config.baseUrl` on a provider that has a stored credential requires sending a
new `credential` in the same update**; otherwise the API answers `422` ("re-enter the API
key when changing the base URL"). The old key is never sent to the new host.

Testing a saved provider sends the stored key to the endpoint and saves the discovered models,
so it requires the right to manage the provider, not just to view it.

## Outbound network policy

ORBIT makes requests to URLs that users configure: provider endpoints, connection tests,
output webhooks and work-source (GitHub/GitLab) base URLs. These requests go through an
address policy so a manager cannot point ORBIT at internal services or the cloud
metadata endpoint:

| Target | Default | Can be allowed? |
|---|---|---|
| Loopback (`127.0.0.0/8`, `::1`), link-local (`169.254.0.0/16`, `fe80::/10`) | blocked | yes, via `ORBIT_OUTBOUND_ALLOWED_HOSTS` |
| Private ranges (RFC 1918, CGNAT `100.64.0.0/10`, IPv6 ULA) | **allowed** (self-hosted model servers and GitLab usually live there) | block with `ORBIT_OUTBOUND_ALLOW_PRIVATE=false` |
| Cloud metadata (`169.254.169.254`, `169.254.170.2`, `100.100.100.200`, `fd00:ec2::254`), unspecified, multicast, broadcast, reserved | blocked | never |

- `ORBIT_OUTBOUND_ALLOW_PRIVATE` (default `true`): set `false` for a strict, internet-only
  deployment.
- `ORBIT_OUTBOUND_ALLOWED_HOSTS`: comma-separated hostnames, IPs or CIDRs that are exempt,
  for example `localhost,ollama.internal,10.20.0.0/16`. Use it for a model server on the
  same host (`localhost`).
- Helm: `outbound.allowPrivate` and `outbound.allowedHosts`.
- Host names are resolved and every resulting address is checked, also at connection time
  (so DNS rebinding does not bypass it). Redirects are followed only within the same host.
- Failures are reported as a category (`blocked`, `dns`, `connect`, `tls`, `status N`),
  not the raw error. A blocked target shows `endpoint refused (blocked)`.

## Using OpenCode as a platform provider

Create an `openai_compatible` provider with the OpenCode gateway base URL and API key if
you want ORBIT-managed credentials instead of a harness auth Secret.

## Decoupling from the agent

The agent stores a default harness/model. Work and team runs can override the harness for
that run; model selection can be per agent, per team step, or worker-profile-level. There
is no hardcoded provider or model inventory in the control plane.
