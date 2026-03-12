# FreeCycle

GPU-aware Ollama lifecycle manager for Windows 11. Runs as a system tray application that monitors NVIDIA GPU usage and game processes, automatically enabling and disabling networked Ollama access when the GPU is available for LLM compute workloads.

## What It Does

FreeCycle sits in your Windows system tray and watches for GPU-intensive games and applications. When a game + GPU-intensive application is detected (or heavy non-whitelisted VRAM usage), Ollama is shut down instantly. When the GPU is free (including a 30-minute cooldown after the game exits), Ollama is started and exposed to the local network so other machines and agentic workflows can use your GPU for LLM inference.

**Key features:**

- Monitors for 10 preconfigured game executables (configurable)
- Tracks VRAM usage from non-whitelisted processes (50% threshold, configurable)
- Starts/stops Ollama automatically with network exposure (`OLLAMA_HOST=0.0.0.0:11434`)
- Auto-downloads and updates required models (`llama3.1:8b-instruct-q4_K_M`, `nomic-embed-text`)
- Provides an HTTP API (port 7443) for external agents to signal GPU task start/stop
- Color-coded tray icon: green (available), red (blocked), blue (agent task), yellow (downloading), grey (error)
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
| Red    | Game detected or cooldown active           |
| Blue   | External agent task in progress            |
| Yellow | Downloading/updating models                |
| Grey   | Error or initializing                      |

### Right-Click Menu

- **Status**: Shows current state (read-only label)
- **Force Enable Ollama**: Override and start Ollama
- **Force Disable Ollama**: Override and stop Ollama
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

External agentic workflows can signal task start/stop to FreeCycle via HTTP.

### Endpoints

**GET /status**
Returns current FreeCycle status as JSON.

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
Returns 200 OK if FreeCycle is running.

### Example

```bash
curl http://192.168.1.10:7443/status
curl -X POST http://192.168.1.10:7443/task/start -H "Content-Type: application/json" -d '{"task_id":"job-1","description":"Training run"}'
curl -X POST http://192.168.1.10:7443/task/stop -H "Content-Type: application/json" -d '{"task_id":"job-1"}'
```

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

