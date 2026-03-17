# MCP Setup And Registration

Use this file for install, registration, and first-run checks.

## Prerequisites

- Node.js 18 or newer.
- FreeCycle running locally or reachable on the network.
- A valid MAC address and broadcast route if wake-on-LAN is enabled.

## Install

```bash
cd mcp-server
npm install
npm run build
```

## Registration

### Claude Code (from within mcp-server/)

If you are in the `mcp-server/` directory:

```bash
claude mcp add freecycle node dist/index.js
```

### Claude Code (with explicit --cwd)

Run from any directory with an absolute path to the `mcp-server/` folder. On Windows with Dropbox:

```bash
claude mcp add freecycle --cwd "C:\Users\<user>\Dropbox\freecycle\mcp-server" node dist/index.js
```

Replace `<user>` with your Windows username.

### Claude Code with external config

If your config file is at a custom path:

```bash
claude mcp add freecycle \
  --cwd "C:\Users\<user>\Dropbox\freecycle\mcp-server" \
  -e FREECYCLE_MCP_CONFIG="C:\Users\<user>\Dropbox\freecycle\mcp-server\freecycle-mcp.config.json" \
  node dist/index.js
```

### Verification

After registration, confirm the server was added:

```bash
claude mcp list
```

You should see `freecycle` in the list. If not, run the command again and check for error messages.

### OpenAI Codex

Add this to your Codex config file:

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

1. Run `npm run build` in `mcp-server/` to ensure `dist/index.js` is current.
2. Confirm the intended config path exists if `FREECYCLE_MCP_CONFIG` is set.
3. Use `claude mcp add` with `--cwd` to register (see Registration above).
4. Run `claude mcp list` to verify registration succeeded.
5. Confirm the target FreeCycle host and Ollama host are reachable.
6. Call `freecycle_check_availability` before attempting inference or model pulls.
7. If local inference is unavailable, read `references/failure-recovery.md` instead of retrying blind.

## Runtime Flow

1. Check the FreeCycle Inference API first.
2. If local inference is down, check FreeCycle status.
3. If FreeCycle is unreachable and wake-on-LAN is enabled, send wake packets and wait.
4. If the stack never becomes reachable, return a structured cloud fallback payload.

## Related Files

- `../RAG_INDEX.md`
- `config-and-timeouts.md`
- `tools-and-routing.md`
- `failure-recovery.md`
