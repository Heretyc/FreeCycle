# MCP Setup And Registration

Use this file for install, registration, and first-run checks.

## Prerequisites

- Node.js 18 or newer.
- FreeCycle running locally or reachable on the network.
- Ollama installed on the FreeCycle machine.
- A valid MAC address and broadcast route if wake-on-LAN is enabled.

## Install

```bash
cd mcp-server
npm install
npm run build
```

## Registration

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

## First-Run Checklist

1. Confirm `dist/index.js` exists after `npm run build`.
2. Confirm the intended config path exists if `FREECYCLE_MCP_CONFIG` is set.
3. Confirm the target FreeCycle host and Ollama host are reachable.
4. Call `freecycle_check_availability` before attempting inference or model pulls.
5. If local inference is unavailable, read `references/failure-recovery.md` instead of retrying blind.

## Runtime Flow

1. Check Ollama first.
2. If Ollama is down, check FreeCycle.
3. If FreeCycle is unreachable and wake-on-LAN is enabled, send wake packets and wait.
4. If the stack never becomes reachable, return a structured cloud fallback payload.

## Related Files

- `../RAG_INDEX.md`
- `config-and-timeouts.md`
- `tools-and-routing.md`
- `failure-recovery.md`
