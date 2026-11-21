# CLI tools: glab and gh

The OpenCode and Pi worker images include `glab` and `gh`. ORBIT still recommends the
connector/broker tools; the CLIs are an opt-in escape hatch.

## Enable per worker profile

- `allowGlab`: inject `GITLAB_TOKEN` / `GITLAB_HOST`
- `allowGh`: inject `GH_TOKEN` / `GH_HOST`

Tokens are injected only when the flag is enabled **and** a GitLab/GitHub connection that
has a stored credential reaches the run: either a connection attached to the Agent Team,
or one bound on the agent itself (so single-agent runs get tokens too). The token is then visible to any shell/tool in that execution
and bypasses broker capability allowlists, approvals and audit. Use a dedicated
least-privilege token and a separate worker profile.

## Usage

```bash
glab auth status
glab mr list
gh auth status
gh pr list
```

Self-hosted GitLab uses `GITLAB_HOST` (hostname only); GitHub Enterprise uses `GH_HOST`.
The worker image must contain the binaries — the standard `orbit-worker-opencode` and
`orbit-worker-pi` images do.
