# FreeCycle MCP Server

MCP (Model Context Protocol) server that exposes FreeCycle and Ollama as tools for Claude Code, OpenAI Codex, or any MCP compatible client.

This README is intentionally short. For token-conscious retrieval, start with [RAG_INDEX.md](RAG_INDEX.md) and then open only the reference file that matches the task.

## Install

```bash
cd mcp-server
npm install
npm run build
```

## Register

Claude Code:

```bash
claude mcp add freecycle node dist/index.js
```

Claude Code with external config:

```bash
claude mcp add freecycle -e FREECYCLE_MCP_CONFIG=C:\\path\\to\\freecycle-mcp.config.json node dist/index.js
```

OpenAI Codex:

```json
{
  "mcpServers": {
    "freecycle": {
      "command": "node",
      "args": ["C:/path/to/freecycle/mcp-server/dist/index.js"],
      "env": {
        "FREECYCLE_MCP_CONFIG": "C:/path/to/freecycle/mcp-server/freecycle-mcp.config.json"
      }
    }
  }
}
```

## Runtime Summary

1. Check whether Ollama is already responding.
2. If not, check whether FreeCycle is reachable.
3. If FreeCycle is unreachable and wake-on-LAN is enabled, send wake packets and wait.
4. If local inference still never becomes reachable, return a structured cloud fallback result.

After local readiness succeeds, `freecycle_pull_model`, `freecycle_generate`, `freecycle_chat`, `freecycle_embed`, and `freecycle_benchmark` automatically signal FreeCycle task start and stop so the tray reflects active MCP work.

`freecycle_pull_model` uses FreeCycle `POST /models/install`, not Ollama `POST /api/pull`, so the local tray unlock remains authoritative for remote installs.

## Reference Files

- [RAG_INDEX.md](RAG_INDEX.md): Read this first for token-conscious lookup.
- [references/setup-and-registration.md](references/setup-and-registration.md): Install, register, and first-run checks.
- [references/config-and-timeouts.md](references/config-and-timeouts.md): Config file shape, precedence, and environment overrides.
- [references/tools-and-routing.md](references/tools-and-routing.md): Tool list, readiness flow, and routing behavior.
- [references/failure-recovery.md](references/failure-recovery.md): Fast diagnosis for startup, config, transport, model, and tray-lock failures.

## License

Apache 2.0
