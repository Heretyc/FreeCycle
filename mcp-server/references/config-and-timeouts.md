# MCP Config And Timeouts

Use this file for config-path questions, host and port overrides, timeout tuning, and wake-on-LAN settings.

## Config Resolution

The MCP server resolves config in this order:

1. Environment overrides.
2. `FREECYCLE_MCP_CONFIG`, if set, otherwise `mcp-server/freecycle-mcp.config.json`.
3. Built-in defaults inside `src/config.ts`.

`ollama.host` defaults to `freecycle.host` when omitted.

## Default Config Shape

```json
{
  "freecycle": {
    "host": "localhost",
    "port": 7443
  },
  "ollama": {
    "host": "localhost",
    "port": 11434
  },
  "timeouts": {
    "requestSecs": 10,
    "inferenceSecs": 300,
    "pullSecs": 600
  },
  "wakeOnLan": {
    "enabled": false,
    "macAddress": "",
    "broadcastAddress": "255.255.255.255",
    "port": 9,
    "packetCount": 5,
    "packetIntervalSecs": 0.25,
    "pollIntervalSecs": 30,
    "maxWaitSecs": 900
  }
}
```

## High-Value Keys

| Key | Default | Why It Matters |
|---|---|---|
| `freecycle.host` | `localhost` | FreeCycle agent API target |
| `freecycle.port` | `7443` | FreeCycle agent API port |
| `ollama.host` | `localhost` | Ollama API target |
| `ollama.port` | `11434` | Ollama API port |
| `timeouts.requestSecs` | `10` | Short control-plane calls |
| `timeouts.inferenceSecs` | `300` | Generate, chat, and embed calls |
| `timeouts.pullSecs` | `600` | Model install operations |
| `wakeOnLan.enabled` | `false` | Enables wake-and-wait flow |
| `wakeOnLan.pollIntervalSecs` | `30` | Delay between readiness checks |
| `wakeOnLan.maxWaitSecs` | `900` | Maximum wait before cloud fallback |

## Environment Overrides

- `FREECYCLE_MCP_CONFIG`
- `FREECYCLE_HOST`
- `FREECYCLE_PORT`
- `OLLAMA_HOST`
- `OLLAMA_PORT`
- `FREECYCLE_REQUEST_TIMEOUT_SECS`
- `FREECYCLE_INFERENCE_TIMEOUT_SECS`
- `FREECYCLE_PULL_TIMEOUT_SECS`
- `FREECYCLE_WOL_ENABLED`
- `FREECYCLE_WOL_MAC`
- `FREECYCLE_WOL_BROADCAST`
- `FREECYCLE_WOL_PORT`
- `FREECYCLE_WOL_PACKET_COUNT`
- `FREECYCLE_WOL_PACKET_INTERVAL_SECS`
- `FREECYCLE_WOL_POLL_INTERVAL_SECS`
- `FREECYCLE_WOL_MAX_WAIT_SECS`

## Fast Guidance

- If the MCP server keeps targeting `localhost`, check `FREECYCLE_MCP_CONFIG`, `FREECYCLE_HOST`, and `OLLAMA_HOST`.
- If probes take too long during testing, temporarily lower `FREECYCLE_REQUEST_TIMEOUT_SECS` and `FREECYCLE_WOL_MAX_WAIT_SECS`.
- If wake-on-LAN is enabled but never works, check `wakeOnLan.macAddress`, broadcast address, and network routing before changing inference code.

## Related Files

- `../RAG_INDEX.md`
- `setup-and-registration.md`
- `failure-recovery.md`
