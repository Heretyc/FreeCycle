# MCP Config And Timeouts

Use this file for config-path questions, host and port overrides, timeout tuning, and wake-on-LAN settings.

## Config Resolution

The MCP server resolves config in this order:

1. Environment overrides.
2. `FREECYCLE_MCP_CONFIG`, if set, otherwise `mcp-server/freecycle-mcp.config.json`.
3. Built-in defaults inside `src/config.ts`.

`ollama.host` defaults to the first server's host when omitted.

## Backward Compatibility: Old vs New Config

### Old Single-Server Format (still supported)

```json
{
  "freecycle": {
    "host": "localhost",
    "port": 7443
  },
  "ollama": { ... },
  "timeouts": { ... },
  "wakeOnLan": { ... }
}
```

This is automatically treated as a single-entry `servers` array at load time. No migration is needed.

### New Multi-Server Format

```json
{
  "servers": [
    {
      "host": "localhost",
      "port": 7443,
      "name": "Local",
      "approved": true,
      "tls_fingerprint": "a1b2c3d4...",
      "identity_uuid": "550e8400-...",
      "wakeOnLan": { "enabled": true, "macAddress": "..." },
      "timeouts": { "pullSecs": 1200 }
    },
    {
      "host": "192.168.1.100",
      "port": 7443,
      "name": "Remote",
      "approved": false
    }
  ],
  "ollama": { ... },
  "timeouts": { ... },
  "wakeOnLan": { ... }
}
```

Each server in the `servers` array can override global `wakeOnLan` and `timeouts`. Global settings apply to servers that do not override them.

## Default Config Shape (Multi-Server)

```json
{
  "servers": [
    {
      "host": "localhost",
      "port": 7443,
      "name": "Local",
      "approved": true
    }
  ],
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
| `servers[0].host` | `localhost` | Primary FreeCycle agent API target |
| `servers[0].port` | `7443` | FreeCycle agent API port (TLS-capable) |
| `servers[0].approved` | `true` | Must be true to allow connections |
| `servers[0].tls_fingerprint` | (auto-populated) | SHA-256 cert fingerprint for TOFU verification |
| `ollama.host` | (server[0].host) | FreeCycle Inference API host (via proxy) |
| `ollama.port` | `11434` | FreeCycle Inference API port (routes through FreeCycle proxy on 7443) |
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

- To configure multiple FreeCycle servers, use the `servers` array. Each entry can have its own `approved`, `tls_fingerprint`, and `identity_uuid`.
- To add a new server: either edit the config manually with `approved: false` and instructions, or use the `freecycle_add_server` MCP tool.
- For TLS connections: the MCP client automatically tries HTTPS first, falls back to plaintext if TLS fails (compatibility mode). Fingerprints are extracted and stored on first connect.
- If a server connection fails with "approved=false": edit `freecycle-mcp.config.json` and set `approved: true` for that server entry.
- If the MCP server keeps targeting `localhost`, check `FREECYCLE_MCP_CONFIG`, `servers[0].host`, and `OLLAMA_HOST`.
- If probes take too long during testing, temporarily lower `FREECYCLE_REQUEST_TIMEOUT_SECS` and `FREECYCLE_WOL_MAX_WAIT_SECS`.
- If wake-on-LAN is enabled but never works, check `wakeOnLan.macAddress`, broadcast address, and network routing before changing inference code.

## Related Files

- `../RAG_INDEX.md`
- `setup-and-registration.md`
- `failure-recovery.md`
