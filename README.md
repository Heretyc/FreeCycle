# FreeCycle

GPU-aware Ollama lifecycle manager for Windows 11. Runs as a system tray application that monitors NVIDIA GPU usage and game processes, automatically enabling and disabling networked Ollama access when the GPU is available for LLM compute workloads.

## What It Does

FreeCycle sits in your Windows system tray and watches for GPU-intensive games and applications. When a game + GPU-intensive application is detected (or heavy non-whitelisted VRAM usage), Ollama is shut down instantly. When the GPU is free (including a 30-minute cooldown after the game exits), Ollama is started and exposed to the local network so other machines and agentic workflows can use your GPU for LLM inference.

**Key features:**

- Monitors for 10 preconfigured game executables (configurable)
- Tracks VRAM usage from non-whitelisted processes (50% threshold, configurable)
- Starts/stops Ollama automatically with network exposure (`OLLAMA_HOST=0.0.0.0:11434`)
- Auto-downloads and updates required models (`llama3.1:8b-instruct-q4_K_M`, `nomic-embed-text`)
- Streams model pull progress into the tray tooltip with live percentage updates when Ollama reports byte totals
- Lets the local user unlock remote model installs from the tray for one hour at a time, then auto-locks again
- Waits 60 seconds after Windows resume before re-enabling Ollama (configurable)
- Provides an HTTP API (port 7443) for external agents to signal GPU task start/stop
- Color-coded tray icon: green (available), red (blocked), blue (agent task), yellow (downloading), grey (error)
- Automatic Windows notifications on state transitions (blocked, available, etc.)
- Self-registers for Windows auto-start; disables Ollama's own auto-start
- Single-instance enforcement via lockfile

## Requirements

- Windows 11 x86_64
- NVIDIA GPU with drivers installed (NVML)
- [Ollama](https://ollama.ai) installed

## Installation

### From source

```
cargo install --path .
```

### From crates.io

```
cargo install freecycle
```

### Manual build

```
cargo build --release
```

The binary is at `target\release\freecycle.exe`.

## Usage

```
freecycle              # Start normally (warnings/errors to stderr)
freecycle -v           # Start with verbose debug logging to ~/freecycle-verbose.log
freecycle --help       # Show help
```

FreeCycle registers itself to start automatically when you log into Windows. The tray icon appears in the system tray notification area.

### Tray Icon States

| Color  | Meaning                                    |
|--------|--------------------------------------------|
| Green  | GPU available, Ollama running              |
| Red    | Game detected, cooldown active, or wake delay active |
| Blue   | External agent task in progress            |
| Yellow | Downloading/updating models                |
| Grey   | Error or initializing                      |

While a model pull or update is active, the tray tooltip now prefers concise progress lines such as `Downloading llama3.1:8b-instruct-q4_K_M: 42%` so the current percentage stays visible within the Windows tooltip length limit.

### Right-Click Menu

- **Status**: Shows current state (read-only label)
- **Force Enable Ollama**: Override and start Ollama immediately until a newly detected blocked state clears it. A post-resume wake delay clears this override.
- **Force Disable Ollama**: Override and stop Ollama
- **Enable Remote Model Installs (1 Hour)**: Allows remote agents to request `POST /models/install` for the next hour. The menu text flips to a disable action while the window is open, and FreeCycle auto-locks it again after one hour.
- **Open Logs**: Opens the verbose log in Notepad
- **Open Config**: Opens config.toml in Notepad
- **Exit FreeCycle**: Shuts down FreeCycle and Ollama

## Configuration

Configuration file: `%APPDATA%\FreeCycle\config.toml` (created on first run with defaults).

```toml
[general]
gpu_check_interval_ms = 5000
tray_update_interval_ms = 2000
cooldown_seconds = 1800
vram_threshold_percent = 50
vram_idle_mb = 300
vram_idle_timeout_minutes = 3
task_timeout_hours = 1
wake_delay_seconds = 60

[ollama]
host = "0.0.0.0"
port = 11434
graceful_shutdown_timeout_seconds = 10

[models]
required = ["llama3.1:8b-instruct-q4_K_M", "nomic-embed-text"]
retry_interval_minutes = 5
update_check_interval_hours = 24

[blacklisted_processes]
list = [
    "VRChat.exe", "vrchat.exe", "Cyberpunk2077.exe", "HELLDIVERS2.exe",
    "GenshinImpact.exe", "ZenlessZoneZero.exe", "Overwatch.exe",
    "VALORANT.exe", "eldenring.exe", "MonsterHunterWilds.exe"
]

[whitelisted_processes]
list = [
    "ollama_llama_server", "ollama_llama_server.exe",
    "ollama.exe", "ollama", "dwm.exe", "csrss.exe"
]

[agent_server]
port = 7443
bind_address = "0.0.0.0"
```

## Agent Signal API

External agentic workflows can signal task start/stop to FreeCycle via HTTP. This is still the right path for direct integrations and custom jobs that do not go through the shipped MCP tools.

### Endpoints

**GET /status**
Returns current FreeCycle status as JSON. Response body includes:

- `status`: Current status label (`Available`, `Blocked`, `Cooldown`, `WakeDelay`, `AgentTaskActive`, `Downloading`, `Error`, or `Initializing`)
- `ollama_running`: Whether Ollama is currently running
- `vram_used_mb`: VRAM used in megabytes
- `vram_total_mb`: Total VRAM in megabytes
- `vram_percent`: VRAM usage as a percentage (0-100)
- `active_task_id`: Current agent task ID, if any
- `active_task_description`: Current agent task description, if any
- `local_ip`: FreeCycle's local IP address
- `ollama_port`: Ollama listening port (default 11434)
- `blocking_processes`: List of process names currently blocking the GPU
- `model_status`: List of model download/update status messages
- `remote_model_installs_unlocked`: `true` while the tray-controlled install window is open
- `remote_model_installs_expires_in_seconds`: Seconds remaining before the install window auto-locks, or `null` when locked

**POST /task/start**
```json
{"task_id": "my-task-1", "description": "Running batch inference"}
```
Turns the tray icon blue and shows the task in the tooltip.

**POST /task/stop**
```json
{"task_id": "my-task-1"}
```
Clears the task and reverts the icon to green.

**GET /health**
Returns JSON health data such as `{"ok": true, "message": "FreeCycle is running"}`.

**POST /models/install**
```json
{"model_name": "qwen2.5-coder:7b"}
```
Installs any Ollama model that the local server can resolve, but only while the tray menu unlock is active. If the user has not enabled the tray toggle, FreeCycle returns `403 Forbidden` with guidance to unlock it locally first. To browse installable model names, check the [Ollama Library](https://ollama.com/library).

### Example

```bash
curl http://192.168.1.10:7443/status
curl -X POST http://192.168.1.10:7443/task/start -H "Content-Type: application/json" -d '{"task_id":"job-1","description":"Training run"}'
curl -X POST http://192.168.1.10:7443/task/stop -H "Content-Type: application/json" -d '{"task_id":"job-1"}'
curl -X POST http://192.168.1.10:7443/models/install -H "Content-Type: application/json" -d '{"model_name":"qwen2.5-coder:7b"}'
```

## MCP Server

The repository includes a Node.js MCP server in `mcp-server/` that exposes FreeCycle and Ollama as tools for Claude Code, OpenAI Codex, and other MCP compatible clients. The server loads configuration from `mcp-server/freecycle-mcp.config.json`.

When a local MCP tool is invoked, the server checks Ollama first. If Ollama is not responding and wake-on-LAN is enabled in the config, it sends magic packets to the configured FreeCycle machine and polls FreeCycle every 30 seconds for up to 15 minutes. If local inference becomes unavailable, the MCP tool reports local unavailability so workflows can fall back to cloud models.

The long-running local MCP tools (`freecycle_pull_model`, `freecycle_generate`, `freecycle_chat`, `freecycle_embed`, `freecycle_benchmark`) automatically call `/task/start` and `/task/stop` so the FreeCycle tray reflects active MCP work. `freecycle_pull_model` routes through FreeCycle's `/models/install` endpoint instead of calling Ollama directly, so the local user must unlock remote model installs from the tray first. The unlock expires after one hour.

### Prerequisites

- Node.js version 18 or later

### Install and Build

1. Install dependencies and build the MCP server:

```bash
cd mcp-server
npm install
npm run build
```

The compiled MCP server is now in `mcp-server/dist/index.js`.

### Configure

Edit `mcp-server/freecycle-mcp.config.json` to set your FreeCycle and Ollama host/port addresses:

```json
{
  "freecycle": {
    "host": "192.168.1.10",
    "port": 7443
  },
  "ollama": {
    "host": "192.168.1.10",
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
    "broadcastAddress": "255.255.255.255"
  }
}
```

Key fields (see `mcp-server/references/config-and-timeouts.md` for full schema):
- `freecycle.host`, `freecycle.port`: Network address of the FreeCycle server
- `ollama.host`, `ollama.port`: Network address of the Ollama API
- `wakeOnLan.enabled`: Set to `true` if you want the MCP server to wake the FreeCycle machine on demand
- `wakeOnLan.macAddress`: MAC address of the FreeCycle machine (required if WoL is enabled)

### Register with Claude Code

If you have built the MCP server and configured it, register it with Claude Code:

```bash
claude mcp add freecycle node C:\path\to\freecycle\mcp-server\dist\index.js -e FREECYCLE_MCP_CONFIG=C:\path\to\freecycle\mcp-server\freecycle-mcp.config.json
```

Adjust the paths to match your FreeCycle repository location.

### Register with OpenAI Codex

Add the server to your Codex `cline_mcp_settings.json` or equivalent config file:

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

### First-Run Verification

After registering the MCP server, ask Claude Code or your MCP client to call the `freecycle_status` tool. A successful response means the MCP server can reach FreeCycle and is ready to use.

### More Information

For detailed configuration, environment variable overrides, timeouts, tool descriptions, and troubleshooting, see:
- `mcp-server/RAG_INDEX.md`: Token-conscious index of all MCP reference files
- `mcp-server/references/setup-and-registration.md`: Detailed setup and registration steps
- `mcp-server/references/config-and-timeouts.md`: Complete configuration schema
- `mcp-server/references/tools-and-routing.md`: Tool list and readiness behavior
- `mcp-server/references/failure-recovery.md`: Troubleshooting guide

## Development

```
cargo check              # Type check
cargo build              # Debug build
cargo build --release    # Release build
cargo test               # Run tests
cargo clippy             # Lint
cargo fmt                # Format
cargo run -- -v          # Run with verbose logging
```

## License

Apache 2.0
