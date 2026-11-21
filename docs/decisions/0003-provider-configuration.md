# ADR-0003: Explicit, kind-tagged model provider configuration

**Status:** Accepted

## Context
"OpenAI-compatible" endpoints differ widely (tools, streaming, JSON mode, `/models`).
OpenCode Go and Claude Code manage their own model inventory and credentials.

## Decision
- Providers are first-class entities separate from runtimes, tagged by `kind`:
  `openai_compatible` or `runtime_managed`.
- Capabilities are explicit booleans; adapters must check them instead of assuming.
- The API never returns secret material; it returns `has_credential` and a `secret_ref`
  handle. Credentials are encrypted at rest and resolved only in the worker.
- OpenCode Go is a `runtime_managed` provider used by the OpenCode adapter, never a
  runtime type and never hardcoded in UI/domain logic.

## Consequences
- Adding a provider kind requires a config struct + validation, not cross-module edits.
- Misconfiguration produces clear validation errors rather than runtime surprises.

## Terminology and amendment (2026-11-21)
ADR 0008 renamed `runtime_managed` to `harness_managed` and "runtime" to "harness" in this
text. `ProviderKind` also has an `anthropic` kind today (native Messages API). The
database constraint `providers_kind_check` (migration 0007) does not allow it yet, so
saving an `anthropic` provider fails; see the provider contract page.
