# FreeCycle MCP Server

MCP (Model Context Protocol) server that exposes FreeCycle and Ollama as tools for Claude Code, OpenAI Codex, or any MCP compatible client.

This server can wake a remote FreeCycle host over LAN before local-only tools run. The runtime flow is:

1. Check whether Ollama is already responding.
2. If not, check whether FreeCycle is reachable.
3. If FreeCycle is unreachable and wake-on-LAN is enabled in the MCP config, send multiple magic packets to the configured FreeCycle server.
4. Poll FreeCycle every 30 seconds by default, for up to 15 minutes by default.
5. If local inference still never becomes reachable, return a structured "route to cloud" result instead of hanging forever.

After local readiness succeeds, the long-running local tools automatically signal FreeCycle task start and stop so the tray reflects active MCP work. The shipped auto-tracked tools are `freecycle_pull_model`, `freecycle_generate`, `freecycle_chat`, `freecycle_embed`, and `freecycle_benchmark`. The manual task tools remain available for custom workflows outside those built-in operations.

## Prerequisites

- Node.js 18+
- FreeCycle running on the local machine or reachable on the network
- Ollama installed and managed by FreeCycle
- A valid MAC address and broadcast route if you enable wake-on-LAN

## Installation

```bash
cd mcp-server
npm install
npm run build
```

## Configuration

The server loads `freecycle-mcp.config.json` from the `mcp-server/` directory by default.

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
    "requestMs": 10000,
    "inferenceMs": 300000,
    "pullMs": 600000
  },
  "wakeOnLan": {
    "enabled": false,
    "macAddress": "",
    "broadcastAddress": "255.255.255.255",
    "port": 9,
    "packetCount": 5,
    "packetIntervalMs": 250,
    "pollIntervalMs": 30000,
    "maxWaitMs": 900000
  }
}
```

### Key Settings

| Key | Default | Description |
|---|---|---|
| `freecycle.host` | `localhost` | FreeCycle agent API host |
| `freecycle.port` | `7443` | FreeCycle agent API port |
| `ollama.host` | `localhost` | Ollama API host. Defaults to the FreeCycle host when omitted in the config loader |
| `ollama.port` | `11434` | Ollama API port |
| `wakeOnLan.enabled` | `false` | Enables the silent wake-and-wait flow |
| `wakeOnLan.macAddress` | `""` | Target machine MAC address for magic packets |
| `wakeOnLan.broadcastAddress` | `255.255.255.255` | UDP broadcast address |
| `wakeOnLan.port` | `9` | UDP port used for wake-on-LAN |
| `wakeOnLan.packetCount` | `5` | Number of magic packets sent per wake attempt |
| `wakeOnLan.packetIntervalMs` | `250` | Delay between magic packets |
| `wakeOnLan.pollIntervalMs` | `30000` | Delay between FreeCycle readiness checks |
| `wakeOnLan.maxWaitMs` | `900000` | Maximum wake wait time. Default is 15 minutes |

### Environment Overrides

The config file is the primary source of truth. These environment variables can override it:

- `FREECYCLE_MCP_CONFIG`
- `FREECYCLE_HOST`
- `FREECYCLE_PORT`
- `OLLAMA_HOST`
- `OLLAMA_PORT`
- `FREECYCLE_REQUEST_TIMEOUT_MS`
- `FREECYCLE_INFERENCE_TIMEOUT_MS`
- `FREECYCLE_PULL_TIMEOUT_MS`
- `FREECYCLE_WOL_ENABLED`
- `FREECYCLE_WOL_MAC`
- `FREECYCLE_WOL_BROADCAST`
- `FREECYCLE_WOL_PORT`
- `FREECYCLE_WOL_PACKET_COUNT`
- `FREECYCLE_WOL_PACKET_INTERVAL_MS`
- `FREECYCLE_WOL_POLL_INTERVAL_MS`
- `FREECYCLE_WOL_MAX_WAIT_MS`

## Usage with Claude Code

```bash
claude mcp add freecycle node dist/index.js
```

If the config file is somewhere else:

```bash
claude mcp add freecycle -e FREECYCLE_MCP_CONFIG=C:\\path\\to\\freecycle-mcp.config.json node dist/index.js
```

## Usage with OpenAI Codex

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

## Tools

| Tool | Description |
|---|---|
| `freecycle_status` | Get complete system status, with wake-on-LAN preflight when enabled |
| `freecycle_health` | Quick local readiness check for FreeCycle and Ollama |
| `freecycle_start_task` | Manually signal that a custom agent workflow is beginning GPU work |
| `freecycle_stop_task` | Manually signal that a custom agent workflow has finished GPU work |
| `freecycle_check_availability` | Check whether local inference is usable right now |
| `freecycle_list_models` | List all locally available Ollama models |
| `freecycle_show_model` | Get detailed info about a specific model |
| `freecycle_pull_model` | Download a new model to the local Ollama instance and auto-signal the tray while the pull runs |
| `freecycle_generate` | Text generation via local Ollama with automatic task signaling |
| `freecycle_chat` | Multi-turn chat completion via local Ollama with automatic task signaling |
| `freecycle_embed` | Generate vector embeddings via local Ollama with automatic task signaling |
| `freecycle_evaluate_task` | Decide whether a task should run locally, in cloud, or hybrid |
| `freecycle_benchmark` | Benchmark local model latency and throughput with one task signal for the full run |

## Development

```bash
npm run dev
npm run build
npm start
```

## License

Apache 2.0
