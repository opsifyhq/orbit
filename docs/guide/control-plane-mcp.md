# Control-plane MCP server

`orbit-mcp` is an optional, read-only MCP stdio server for external clients such as Claude
Desktop, Cursor or an IDE. It is **not** the worker tool broker.

| Interface | Purpose |
|---|---|
| `orbit-worker mcp-broker` | Tools available to a harness inside one execution |
| `orbit-mcp` | Read-only inspection of ORBIT from an external MCP client |

The control-plane server calls the normal ORBIT HTTP API with the caller's bearer token.
It does not add an execution path, bypass access checks or expose credentials.

## Run it

```bash
export ORBIT_API_URL=https://orbit.example.com
export ORBIT_API_TOKEN=...   # a scoped ORBIT user/API token, never an admin shared token
cargo run -p orbit-mcp
```

The binary speaks MCP over stdio. Example Claude Desktop configuration:

```json
{
  "mcpServers": {
    "orbit": {
      "command": "/path/to/orbit-mcp",
      "env": {
        "ORBIT_API_URL": "https://orbit.example.com",
        "ORBIT_API_TOKEN": "orbit_sk_..."
      }
    }
  }
}
```

## Read-only tools

- `agents.list`, `agents.get`
- `teams.list`, `teams.get`
- `executions.list`, `executions.get`, `executions.events`
- `team_runs.list`, `team_runs.get`

Every call uses the caller's ORBIT permissions. Lists are bounded by the API's paging
limits. Secrets, provider keys, worker tokens and connection credentials are never
returned by these endpoints.

## Not included

Starting/cancelling runs, creating resources, approvals and other writes are deliberately
absent. They need a separate operator mode with explicit scopes, autonomy checks, audit
and idempotency. A2A is a separate interface ([#424](https://github.com/opsifyhq/orbit/issues/424)).
