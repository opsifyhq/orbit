# MCP bridge (removed)

The standalone `orbit-mcp-broker` crate was removed in #315. Production and the
`dev-worker-*.sh` scripts use `orbit-worker mcp-broker` instead.

This directory is kept only so existing handbook references to `services/mcp-broker`
continue to resolve in `pnpm docs:check`; the handbook text is updated separately.
