# FreeCycle MCP Server

MCP (Model Context Protocol) server that exposes FreeCycle and Ollama as tools for Claude Code, OpenAI Codex, or any MCP compatible client.

FreeCycle is a Windows system tray app that monitors GPU usage and manages the Ollama lifecycle. This server lets agentic workflows query GPU status, manage models, run local inference, and intelligently route tasks between local and cloud.

## Prerequisites

- Node.js 18+
- FreeCycle running on the local machine (or reachable on the network)
- Ollama installed and managed by FreeCycle

## Installation

```bash
cd mcp-server
npm install
npm run build
```

## Configuration

Environment variables (all optional, shown with defaults):

| Variable | Default | Description |
|---|---|---|
| FREECYCLE_HOST | localhost | FreeCycle agent API host |
| FREECYCLE_PORT | 7443 | FreeCycle agent API port |
| OLLAMA_HOST | localhost | Ollama API host |
| OLLAMA_PORT | 11434 | Ollama API port |

## Usage with Claude Code

```bash
claude mcp add freecycle node dist/index.js
```

Or with environment overrides:

```bash
claude mcp add freecycle -e FREECYCLE_HOST=192.168.1.10 node dist/index.js
```

## Usage with OpenAI Codex

Add to your MCP configuration:

```json
{
  "mcpServers": {
    "freecycle": {
      "command": "node",
      "args": ["path/to/mcp-server/dist/index.js"],
      "env": {
        "FREECYCLE_HOST": "localhost",
        "FREECYCLE_PORT": "7443"
      }
    }
  }
}
```

## Tools

| Tool | Description |
|---|---|
| freecycle_status | Get complete system status (GPU, VRAM, Ollama, tasks, network) |
| freecycle_health | Quick connectivity check |
| freecycle_start_task | Signal that an agentic workflow is beginning GPU work |
| freecycle_stop_task | Signal that an agentic workflow has finished GPU work |
| freecycle_check_availability | Check if the GPU is available for work |
| freecycle_list_models | List all locally available Ollama models |
| freecycle_show_model | Get detailed info about a specific model |
| freecycle_pull_model | Download a new model to the local Ollama instance |
| freecycle_generate | Text generation (completion) via local Ollama |
| freecycle_chat | Multi turn chat completion via local Ollama |
| freecycle_embed | Generate vector embeddings via local Ollama |
| freecycle_evaluate_task | Evaluate whether a task should run locally, on cloud, or hybrid |
| freecycle_benchmark | Benchmark local model performance (latency, tokens per second) |

## Development

```bash
npm run dev    # Run with tsx (no build step)
npm run build  # Compile TypeScript
npm start      # Run compiled output
```

## License

Apache 2.0
