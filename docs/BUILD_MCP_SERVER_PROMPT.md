# Build a Custom MCP Server for FreeCycle

## Section 1: Role and Context

You are a senior full-stack engineer tasked with building a Model Context Protocol (MCP) server that integrates with FreeCycle. You have deep expertise in MCP server architecture, TypeScript/Node.js (or the user's chosen runtime), HTTP API integration, and agentic tooling patterns.

### What is FreeCycle?

FreeCycle is a Windows 11 system tray application (written in Rust) that monitors NVIDIA GPU usage and manages the Ollama process lifecycle. It watches for GPU intensive games and programs. When one is detected (or when non whitelisted VRAM usage exceeds a threshold), Ollama is shut down. When the GPU is free (after a configurable cooldown), Ollama is restarted and exposed to the local network for LLM inference.

FreeCycle exposes two network interfaces:

1. **Agent Signal API** (default port 7443, configurable): A lightweight HTTP server for external agents to query GPU/system status and signal task start/stop events. This controls the tray icon state and coordinates GPU access.

2. **Ollama API** (default port 11434, configurable): The standard Ollama REST API, managed by FreeCycle. Ollama is started and stopped automatically based on GPU availability. When running, it serves LLM inference and embedding requests over HTTP.

### What is MCP?

The Model Context Protocol (MCP) is a standardized protocol that lets LLM applications (Claude Desktop, Claude Code, OpenAI Codex, custom agents) discover and invoke tools exposed by external servers. An MCP server registers tools with typed input schemas. The MCP client calls those tools on behalf of the user or agent. Communication happens over stdio (for local servers) or HTTP with Server Sent Events (for remote servers).

### What You Are Building

An MCP server that wraps both the FreeCycle Agent Signal API and the Ollama API into a set of MCP tools. This lets any MCP compatible client (Claude, Codex, etc.) check GPU availability, manage tasks, run inference, generate embeddings, and manage models, all through a single MCP connection.

---

## Section 2: Mandatory Questions (Phase 0)

**STOP. Do not write any code yet.**

Before generating any implementation, you MUST ask the user the following questions and wait for answers. Do not assume defaults unless the user explicitly confirms them.

### Required Questions

**Q1: Runtime and Language**
What runtime and language should the MCP server use?
- Node.js with TypeScript (recommended, best MCP SDK support)
- Python (official MCP Python SDK available)
- Go (community MCP libraries available)
- Other (specify)

**Q2: MCP Client Target**
Which MCP client(s) will consume this server?
- Claude Code (stdio transport, registered in .claude/settings.json or .mcp.json)
- Claude Desktop (stdio transport, registered in claude_desktop_config.json)
- OpenAI Codex (stdio transport)
- Custom client (describe transport requirements)
- Multiple clients (list all)

This affects transport selection (stdio vs SSE), configuration file format, and how the server is registered.

**Q3: FreeCycle Host and Ports**
What are the FreeCycle host IP and port numbers?
- Agent Signal API: default is `localhost:7443`
- Ollama API: default is `localhost:11434`
- Is the MCP server running on the same machine as FreeCycle, or on a remote machine?
- If remote, what is the FreeCycle machine's LAN IP?
- If remote, should the MCP server support wake-on-LAN before local tools run?
- If wake-on-LAN is required, what are the server MAC address, broadcast address, UDP port, packet count, poll interval, and maximum wait time before falling back to cloud?

**Q4: Additional Tools**
Beyond the standard tool set (listed in Section 4), do you need any of these?
- Custom inference pipelines (multi step prompting, chain of thought with tool use)
- RAG integration (document chunking, embedding storage, vector search)
- Batch processing (queue multiple inference jobs, parallel embedding)
- Model lifecycle management (pull new models, delete models, check for updates)
- Conversation management (multi turn chat with context windowing)
- Prompt template management (store and invoke named prompt templates)
- Other (describe)

**Q5: Authentication and Security**
What authentication or security requirements exist?
- None (trusted LAN, default FreeCycle v1 posture)
- API key for the MCP server itself (bearer token validation)
- TLS between MCP server and FreeCycle (not supported in FreeCycle v1, but can be planned)
- IP allowlisting
- Other (describe)

### Additional Questions (Ask If Ambiguous)

If anything is unclear after the five required questions, ask follow up questions. Common areas that need clarification:
- Should the server support hot reloading of configuration?
- What error reporting format does the client expect?
- Are there rate limiting requirements for Ollama API calls?
- Should the server implement request queuing when GPU is blocked?
- What logging level and format is desired?
- Should the server expose MCP resources (read only data) in addition to tools?

**Only proceed to implementation after all questions are answered.**

---

## Section 3: Complete API Reference

### 3.1 FreeCycle Agent Signal API

Base URL: `http://{FREECYCLE_HOST}:{AGENT_PORT}` (default: `http://localhost:7443`)

#### GET /health

Simple uptime check.

**Response (200 OK):**
```json
{
  "ok": true,
  "message": "FreeCycle is running"
}
```

#### GET /status

Returns the full system state as JSON.

**Response (200 OK):**
```json
{
  "status": "Available",
  "ollama_running": true,
  "vram_used_mb": 1024,
  "vram_total_mb": 8192,
  "vram_percent": 12,
  "active_task_id": null,
  "active_task_description": null,
  "local_ip": "192.168.1.10",
  "ollama_port": 11434,
  "blocking_processes": [],
  "model_status": [
    "ready: llama3.1:8b-instruct-q4_K_M",
    "ready: nomic-embed-text"
  ]
}
```

**Field Reference:**

| Field | Type | Description |
|-------|------|-------------|
| status | string | Current display label from FreeCycle, for example `"Available"`, `"Blocked (Game Running)"`, `"Cooldown"`, `"Wake Delay"`, `"Agent Task Active"`, `"Downloading Models"`, `"Initializing"`, or `"Error"` |
| ollama_running | boolean | Whether Ollama process is alive and healthy |
| vram_used_mb | number | Total VRAM in use across all processes (MB) |
| vram_total_mb | number | Total GPU VRAM capacity (MB) |
| vram_percent | number | VRAM usage percentage as an integer |
| active_task_id | string or null | Task ID if an agent task is active |
| active_task_description | string or null | Human readable task description if active |
| local_ip | string | Machine's local LAN IP address |
| ollama_port | number | Port Ollama is listening on |
| blocking_processes | string[] | Process names currently triggering a block |
| model_status | string[] | Human readable model status messages from FreeCycle |

**Model status values:** FreeCycle currently exposes plain status strings rather than a keyed object. Preserve the array shape returned by the API.

**GPU status values explained:**
- `"Available"`: GPU is free, Ollama is running, ready for work.
- `"Blocked (Game Running)"`: A blacklisted process is running. Ollama is stopped.
- `"Cooldown"`: A post-game cooldown is active. Ollama stays stopped.
- `"Wake Delay"`: The system recently resumed from sleep. Ollama stays stopped until the post-wake hold expires.
- `"Agent Task Active"`: A tracked task is active and the tray is blue.
- `"Downloading Models"`: Model work is active.
- `"Error"`: GPU monitoring or Ollama lifecycle handling failed. Ollama state is uncertain.
- `"Initializing"`: FreeCycle is still starting.

#### POST /task/start

Signal that an agent is beginning a task that uses Ollama. Turns the tray icon blue.

**Request Body:**
```json
{
  "task_id": "abc-123",
  "description": "Indexing documentation repository"
}
```

| Field | Type | Required | Description |
|-------|------|----------|-------------|
| task_id | string | yes | Unique identifier for the task |
| description | string | yes | Human readable description shown in tray tooltip |

**Response (200 OK):**
```json
{
  "ok": true,
  "message": "Task 'abc-123' registered"
}
```

**Response (409 Conflict, GPU is blocked):**
```json
{
  "ok": false,
  "message": "GPU is currently Blocked (Game Running)"
}
```

**Behavior Notes:**
- If a task is already active with a different task_id, the new task replaces it (last writer wins).
- If a task is already active with the same task_id, the description is updated.
- FreeCycle auto clears tasks after 1 hour of no VRAM activity (configurable).
- FreeCycle infers task idle state if VRAM drops below 300MB for 3+ consecutive minutes.

#### POST /task/stop

Signal that an agent has completed its task. Reverts tray icon to green (or red if blocked).

**Request Body:**
```json
{
  "task_id": "abc-123"
}
```

| Field | Type | Required | Description |
|-------|------|----------|-------------|
| task_id | string | yes | Must match the active task ID |

**Response (200 OK):**
```json
{
  "ok": true,
  "message": "Task 'abc-123' stopped"
}
```

**Response (404 Not Found, task ID mismatch or no active task):**
```json
{
  "ok": false,
  "message": "Task 'abc-123' not found"
}
```

### 3.2 Ollama REST API

Base URL: `http://{FREECYCLE_HOST}:{OLLAMA_PORT}` (default: `http://localhost:11434`)

These are standard Ollama endpoints. The MCP server wraps them into tools.

#### GET /

Health check. Returns `"Ollama is running"` when healthy.

#### GET /api/tags

List all locally available models.

**Response (200 OK):**
```json
{
  "models": [
    {
      "name": "llama3.1:8b-instruct-q4_K_M",
      "model": "llama3.1:8b-instruct-q4_K_M",
      "modified_at": "2024-12-01T10:30:00Z",
      "size": 4920000000,
      "digest": "sha256:abc123...",
      "details": {
        "parent_model": "",
        "format": "gguf",
        "family": "llama",
        "families": ["llama"],
        "parameter_size": "8B",
        "quantization_level": "Q4_K_M"
      }
    }
  ]
}
```

#### POST /api/generate

Generate a text completion (non chat).

**Request Body:**
```json
{
  "model": "llama3.1:8b-instruct-q4_K_M",
  "prompt": "Explain quicksort in one paragraph.",
  "stream": false,
  "options": {
    "temperature": 0.7,
    "num_predict": 512
  }
}
```

**Response (200 OK, stream: false):**
```json
{
  "model": "llama3.1:8b-instruct-q4_K_M",
  "created_at": "2024-12-01T10:30:00Z",
  "response": "Quicksort is a divide-and-conquer...",
  "done": true,
  "done_reason": "stop",
  "context": [1, 2, 3],
  "total_duration": 5000000000,
  "load_duration": 1000000000,
  "prompt_eval_count": 12,
  "prompt_eval_duration": 500000000,
  "eval_count": 87,
  "eval_duration": 3500000000
}
```

**Important:** Always set `"stream": false` in the MCP server. Streaming responses require special handling and most MCP clients expect a single complete response.

#### POST /api/chat

Generate a chat completion with message history.

**Request Body:**
```json
{
  "model": "llama3.1:8b-instruct-q4_K_M",
  "messages": [
    {"role": "system", "content": "You are a helpful assistant."},
    {"role": "user", "content": "What is 2+2?"}
  ],
  "stream": false,
  "options": {
    "temperature": 0.7,
    "num_predict": 512
  }
}
```

**Response (200 OK, stream: false):**
```json
{
  "model": "llama3.1:8b-instruct-q4_K_M",
  "created_at": "2024-12-01T10:30:00Z",
  "message": {
    "role": "assistant",
    "content": "2 + 2 = 4."
  },
  "done": true,
  "done_reason": "stop",
  "total_duration": 3000000000,
  "load_duration": 800000000,
  "prompt_eval_count": 18,
  "prompt_eval_duration": 400000000,
  "eval_count": 12,
  "eval_duration": 1800000000
}
```

#### POST /api/embed

Generate embeddings for one or more texts.

**Request Body:**
```json
{
  "model": "nomic-embed-text",
  "input": ["First document text", "Second document text"]
}
```

`input` can also be a single string instead of an array.

**Response (200 OK):**
```json
{
  "model": "nomic-embed-text",
  "embeddings": [
    [0.123, -0.456, 0.789, ...],
    [0.321, -0.654, 0.987, ...]
  ],
  "total_duration": 1000000000,
  "load_duration": 500000000,
  "prompt_eval_count": 24
}
```

#### POST /api/pull

Pull (download) a model.

**Request Body:**
```json
{
  "name": "llama3.1:8b-instruct-q4_K_M",
  "stream": false
}
```

**Response (200 OK, stream: false):**
```json
{
  "status": "success"
}
```

#### DELETE /api/delete

Delete a model.

**Request Body:**
```json
{
  "name": "model-name:tag"
}
```

**Response (200 OK):** Empty body on success.

#### POST /api/show

Show details about a model.

**Request Body:**
```json
{
  "name": "llama3.1:8b-instruct-q4_K_M"
}
```

**Response (200 OK):**
```json
{
  "modelfile": "...",
  "parameters": "...",
  "template": "...",
  "details": {
    "parent_model": "",
    "format": "gguf",
    "family": "llama",
    "families": ["llama"],
    "parameter_size": "8B",
    "quantization_level": "Q4_K_M"
  },
  "model_info": { ... }
}
```

#### GET /api/ps

List models currently loaded in memory.

**Response (200 OK):**
```json
{
  "models": [
    {
      "name": "llama3.1:8b-instruct-q4_K_M",
      "model": "llama3.1:8b-instruct-q4_K_M",
      "size": 4920000000,
      "digest": "sha256:abc123...",
      "details": { ... },
      "expires_at": "2024-12-01T10:35:00Z",
      "size_vram": 4920000000
    }
  ]
}
```

---

## Section 4: Required MCP Tools

The MCP server must implement the same 13 tools that the shipped `mcp-server/src/tools.ts` currently exposes. Keep the names, primary inputs, and behavior aligned with that implementation.

### 4.1 FreeCycle Status and Task Tools

#### Tool: `freecycle_status`

Get the complete FreeCycle status, including GPU state, VRAM usage, active task metadata, and model status messages.

**Input Schema:**
```json
{
  "type": "object",
  "properties": {},
  "required": []
}
```

**Behavior:** Use the shared local-readiness helper first. If local inference is reachable, return the full `GET /status` payload. If local inference is unavailable, return the same structured cloud-fallback payload used by the shipped server, and include the latest FreeCycle status object when the FreeCycle host itself is reachable.

#### Tool: `freecycle_health`

Check whether FreeCycle and Ollama are both reachable.

**Input Schema:**
```json
{
  "type": "object",
  "properties": {},
  "required": []
}
```

**Behavior:** Run the same local-readiness helper. On success, return the JSON bodies from `GET /health` and the Ollama health check. On failure, return a structured local-unavailable result instead of throwing.

#### Tool: `freecycle_start_task`

Manually signal that a custom workflow is beginning GPU work.

**Input Schema:**
```json
{
  "type": "object",
  "properties": {
    "task_id": {
      "type": "string",
      "description": "Unique identifier for this task."
    },
    "description": {
      "type": "string",
      "description": "Human readable description shown in the FreeCycle tray tooltip."
    }
  },
  "required": ["task_id", "description"]
}
```

**Behavior:** Run the shared local-readiness helper first. Then call `POST /task/start`. On `200`, return the API response. On `409`, return the conflict body exactly as FreeCycle sends it. On transport or `5xx` errors, return a structured error.

#### Tool: `freecycle_stop_task`

Manually signal that a custom workflow has finished GPU work.

**Input Schema:**
```json
{
  "type": "object",
  "properties": {
    "task_id": {
      "type": "string",
      "description": "The task identifier used when starting the task."
    }
  },
  "required": ["task_id"]
}
```

**Behavior:** Run the shared local-readiness helper first. Then call `POST /task/stop`. On `200`, return the API response. On `404`, return the error body exactly as FreeCycle sends it. On transport or `5xx` errors, return a structured error.

#### Tool: `freecycle_check_availability`

Return a simple readiness decision for local inference.

**Input Schema:**
```json
{
  "type": "object",
  "properties": {},
  "required": []
}
```

**Behavior:** Use the shared local-readiness helper. If local inference is reachable, return a compact JSON object such as:
```json
{
  "available": true,
  "status": "Available",
  "ollama_running": true,
  "vram_percent": 12,
  "blocking_processes": []
}
```
If local inference is unavailable, return the same structured cloud-fallback payload used elsewhere.

These two manual task tools are the escape hatch for custom workflows. The local execution tools below must still auto-signal FreeCycle around their own Ollama work so users do not have to call the manual task tools themselves.

### 4.2 Model Management Tools

#### Tool: `freecycle_list_models`

List models currently available from the local Ollama instance.

**Input Schema:**
```json
{
  "type": "object",
  "properties": {},
  "required": []
}
```

**Behavior:** Run the shared local-readiness helper first, then call `GET /api/tags`. Return a friendly summary with model name, size in MB, modified time, and a short digest preview.

#### Tool: `freecycle_show_model`

Inspect a specific local model.

**Input Schema:**
```json
{
  "type": "object",
  "properties": {
    "model_name": {
      "type": "string",
      "description": "Full model name including tag."
    }
  },
  "required": ["model_name"]
}
```

**Behavior:** Run the shared local-readiness helper first, then call `POST /api/show`. Return the response body.

#### Tool: `freecycle_pull_model`

Download a model to the local Ollama instance.

**Input Schema:**
```json
{
  "type": "object",
  "properties": {
    "model_name": {
      "type": "string",
      "description": "Model name to pull."
    }
  },
  "required": ["model_name"]
}
```

**Behavior:** Run the shared local-readiness helper first. Then wrap the pull with automatic `POST /task/start` and `POST /task/stop` using a short description such as `MCP pull: <model>`. Use a generous timeout. If task start returns `409`, return a structured local-unavailable result instead of starting the pull.

### 4.3 Inference and Embedding Tools

#### Tool: `freecycle_generate`

Generate text with a local Ollama model.

**Input Schema:**
```json
{
  "type": "object",
  "properties": {
    "model": {
      "type": "string",
      "default": "llama3.1:8b-instruct-q4_K_M"
    },
    "prompt": {
      "type": "string"
    },
    "system_prompt": {
      "type": "string"
    },
    "temperature": {
      "type": "number"
    },
    "max_tokens": {
      "type": "integer"
    }
  },
  "required": ["prompt"]
}
```

**Behavior:** Run the shared local-readiness helper first. Then call `POST /api/generate` with `stream: false`. Map `max_tokens` to `num_predict`. Automatically wrap the full request with `POST /task/start` and `POST /task/stop` using a short task description such as `MCP generate: <model>`. Never include prompt text in the task description.

#### Tool: `freecycle_chat`

Run a chat completion against a local Ollama model.

**Input Schema:**
```json
{
  "type": "object",
  "properties": {
    "model": {
      "type": "string",
      "default": "llama3.1:8b-instruct-q4_K_M"
    },
    "messages": {
      "type": "array",
      "items": {
        "type": "object",
        "properties": {
          "role": {
            "type": "string",
            "enum": ["system", "user", "assistant"]
          },
          "content": {
            "type": "string"
          }
        },
        "required": ["role", "content"]
      }
    },
    "system_prompt": {
      "type": "string"
    },
    "temperature": {
      "type": "number"
    }
  },
  "required": ["messages"]
}
```

**Behavior:** Run the shared local-readiness helper first. Then call `POST /api/chat` with `stream: false`. Automatically wrap the whole request with start and stop task signaling and ensure the stop signal runs in `finally`.

#### Tool: `freecycle_embed`

Generate embeddings with the local Ollama embedding model.

**Input Schema:**
```json
{
  "type": "object",
  "properties": {
    "model": {
      "type": "string",
      "default": "nomic-embed-text"
    },
    "input": {
      "oneOf": [
        { "type": "string" },
        { "type": "array", "items": { "type": "string" } }
      ]
    }
  },
  "required": ["input"]
}
```

**Behavior:** Run the shared local-readiness helper first. Then call `POST /api/embed`. Automatically wrap the full operation with task start and stop signaling. Return the embeddings plus model metadata.

### 4.4 Evaluation and Benchmarking Tools

#### Tool: `freecycle_evaluate_task`

Recommend whether a task should run locally, in the cloud, or as a hybrid workflow.

**Input Schema:**
```json
{
  "type": "object",
  "properties": {
    "task_description": {
      "type": "string"
    },
    "requirements": {
      "type": "object",
      "properties": {
        "latency": { "type": "string", "enum": ["low", "normal"] },
        "quality": { "type": "string", "enum": ["high", "normal"] },
        "cost": { "type": "string", "enum": ["minimize", "normal"] },
        "privacy": { "type": "string", "enum": ["critical", "normal"] }
      }
    }
  },
  "required": ["task_description"]
}
```

**Behavior:** Use the same readiness logic as the executable tools. Combine availability with keyword-based workload classification and the optional `requirements` object. Return a JSON object containing `recommendation`, `reasoning`, `freecycle_status`, `local_available`, and `wake_on_lan_attempted`.

#### Tool: `freecycle_benchmark`

Benchmark a local model by running the same prompt several times.

**Input Schema:**
```json
{
  "type": "object",
  "properties": {
    "model": {
      "type": "string"
    },
    "prompt": {
      "type": "string"
    },
    "iterations": {
      "type": "integer",
      "default": 3
    }
  },
  "required": ["model", "prompt"]
}
```

**Behavior:** Run the shared local-readiness helper first. Then automatically signal one benchmark task for the full run, not once per iteration. For each iteration, call `POST /api/generate` with a small `num_predict` limit and return per-iteration latency plus average latency and average tokens per second.

---

## Section 5: Implementation Instructions

### 5.1 Project Structure (TypeScript/Node.js)

```
freecycle-mcp-server/
  src/
    index.ts              # Entry point, MCP server setup, stdio transport
    config.ts             # Config loading from file plus env overrides
    availability.ts       # Shared wake and readiness logic
    wake-on-lan.ts        # Magic-packet sender
    freecycle-client.ts   # HTTP client for the FreeCycle Agent API
    ollama-client.ts      # HTTP client for the Ollama API
    task-signaling.ts     # Automatic /task/start and /task/stop wrapper
    tools.ts              # Registers all 13 MCP tools
  package.json
  tsconfig.json
  freecycle-mcp.config.json
  README.md
  test/
    test-server.ts        # End to end test script
```

### 5.2 Dependencies (TypeScript/Node.js)

```json
{
  "dependencies": {
    "@modelcontextprotocol/sdk": "latest",
    "zod": "^3.22.0"
  },
  "devDependencies": {
    "@types/node": "^20.0.0",
    "typescript": "^5.3.0",
    "tsx": "^4.0.0"
  }
}
```

Use the official MCP SDK (`@modelcontextprotocol/sdk`). Use `zod` for input validation (the MCP SDK uses zod schemas natively). Do not add unnecessary dependencies.

### 5.3 Server Initialization Pattern

```typescript
import { McpServer } from "@modelcontextprotocol/sdk/server/mcp.js";
import { StdioServerTransport } from "@modelcontextprotocol/sdk/server/stdio.js";

const server = new McpServer({
  name: "freecycle-mcp",
  version: "1.0.0",
});

// Register all tools here (see Section 5.5 for example)

const transport = new StdioServerTransport();
await server.connect(transport);
```

### 5.4 Configuration

The server must read configuration from a checked-in JSON file such as `freecycle-mcp.config.json`, with optional environment variable overrides:

| Config Key | Default | Description |
|----------|---------|-------------|
| `freecycle.host` | `localhost` | FreeCycle machine hostname or IP |
| `freecycle.port` | `7443` | FreeCycle Agent Signal API port |
| `ollama.host` | `localhost` | Ollama host. Usually the same machine as FreeCycle |
| `ollama.port` | `11434` | Ollama API port |
| `timeouts.requestMs` | `10000` | Default HTTP request timeout in milliseconds |
| `timeouts.pullMs` | `600000` | Timeout for model pull operations |
| `wakeOnLan.enabled` | `false` | Enables the wake-and-wait flow |
| `wakeOnLan.macAddress` | `""` | Target MAC address for wake-on-LAN |
| `wakeOnLan.broadcastAddress` | `255.255.255.255` | UDP broadcast address for magic packets |
| `wakeOnLan.port` | `9` | Wake-on-LAN UDP port |
| `wakeOnLan.packetCount` | `5` | Number of magic packets sent per wake attempt |
| `wakeOnLan.pollIntervalMs` | `30000` | Delay between FreeCycle readiness checks |
| `wakeOnLan.maxWaitMs` | `900000` | Maximum wait before cloud fallback |

Optional override variables can still be supported (`FREECYCLE_MCP_CONFIG`, `FREECYCLE_HOST`, `FREECYCLE_PORT`, `OLLAMA_HOST`, `OLLAMA_PORT`, and `FREECYCLE_WOL_*`), but the JSON config file is the main control surface.

### 5.5 Tool Registration Example

Here is the complete implementation pattern for the `freecycle_status` tool:

```typescript
import { McpServer } from "@modelcontextprotocol/sdk/server/mcp.js";
import { z } from "zod";

// Assume `server` is the McpServer instance from index.ts
// Assume `freecycleClient` is an HTTP client configured with base URL and timeout

server.tool(
  "freecycle_status",
  "Get the full FreeCycle system status including GPU state, VRAM usage, Ollama health, active tasks, and model readiness.",
  {
    // Empty input schema: no parameters needed
  },
  async () => {
    try {
      const response = await freecycleClient.get("/status");

      if (response.status !== 200) {
        return {
          content: [
            {
              type: "text",
              text: JSON.stringify({
                error: true,
                message: `FreeCycle returned HTTP ${response.status}`,
                body: response.data,
              }),
            },
          ],
        };
      }

      return {
        content: [
          {
            type: "text",
            text: JSON.stringify(response.data, null, 2),
          },
        ],
      };
    } catch (error: unknown) {
      const message =
        error instanceof Error ? error.message : "Unknown error";
      return {
        content: [
          {
            type: "text",
            text: JSON.stringify({
              error: true,
              message: `Failed to reach FreeCycle: ${message}`,
              hint: "Verify the FreeCycle host, port, and optional FREECYCLE_MCP_CONFIG path are correct.",
            }),
          },
        ],
        isError: true,
      };
    }
  }
);
```

### 5.6 Tool Registration Example (With Input Parameters)

Here is the pattern for a tool with validated input parameters:

```typescript
server.tool(
  "freecycle_start_task",
  "Signal to FreeCycle that an agent task is beginning. Turns the tray icon blue.",
  {
    task_id: z.string().describe(
      "Unique identifier for this task. Use a descriptive slug or UUID."
    ),
    description: z.string().describe(
      "Human readable description shown in the FreeCycle tray tooltip."
    ),
  },
  async ({ task_id, description }) => {
    try {
      const response = await freecycleClient.post("/task/start", {
        task_id,
        description,
      });

      if (response.status === 409) {
        return {
          content: [
            {
              type: "text",
              text: JSON.stringify({
                ...response.data,
                hint: "GPU is currently in use. Wait for FreeCycle to become available or use freecycle_check_availability to drive a cloud fallback decision.",
              }),
            },
          ],
        };
      }

      return {
        content: [
          {
            type: "text",
            text: JSON.stringify(response.data, null, 2),
          },
        ],
      };
    } catch (error: unknown) {
      const message =
        error instanceof Error ? error.message : "Unknown error";
      return {
        content: [
          {
            type: "text",
            text: JSON.stringify({
              error: true,
              message: `Failed to start task: ${message}`,
            }),
          },
        ],
        isError: true,
      };
    }
  }
);
```

### 5.7 HTTP Client Pattern

Create a thin HTTP client wrapper for each API. The config loader should read a JSON file such as `freecycle-mcp.config.json`, then optionally allow environment variable overrides:

```typescript
// lib/freecycle-client.ts
import { getConfig } from "./config.js";

interface HttpResponse<T = unknown> {
  status: number;
  data: T;
}

export class FreeCycleClient {
  private baseUrl: string;
  private timeoutMs: number;

  constructor() {
    const config = getConfig();
    this.baseUrl = `http://${config.freecycle.host}:${config.freecycle.port}`;
    this.timeoutMs = config.timeouts.requestMs;
  }

  async get<T = unknown>(path: string): Promise<HttpResponse<T>> {
    const controller = new AbortController();
    const timeout = setTimeout(
      () => controller.abort(),
      this.timeoutMs
    );

    try {
      const res = await fetch(`${this.baseUrl}${path}`, {
        method: "GET",
        signal: controller.signal,
      });

      const data = res.headers
        .get("content-type")
        ?.includes("application/json")
        ? await res.json()
        : await res.text();

      return { status: res.status, data: data as T };
    } finally {
      clearTimeout(timeout);
    }
  }

  async post<T = unknown>(
    path: string,
    body: unknown
  ): Promise<HttpResponse<T>> {
    const controller = new AbortController();
    const timeout = setTimeout(
      () => controller.abort(),
      this.timeoutMs
    );

    try {
      const res = await fetch(`${this.baseUrl}${path}`, {
        method: "POST",
        headers: { "Content-Type": "application/json" },
        body: JSON.stringify(body),
        signal: controller.signal,
      });

      const data = res.headers
        .get("content-type")
        ?.includes("application/json")
        ? await res.json()
        : await res.text();

      return { status: res.status, data: data as T };
    } finally {
      clearTimeout(timeout);
    }
  }
}
```

Use Node.js native `fetch` (available in Node 18+). Do not add `axios` or `node-fetch` unless the user specifically requests it.

### 5.8 Error Handling Patterns

Every tool must follow these error handling rules:

1. **Network errors** (connection refused, timeout, DNS failure): Return `isError: true` with a message suggesting the user check that FreeCycle/Ollama is running and that host/port configuration is correct.

2. **HTTP 4xx errors**: Return the error response from the API with context. Do not set `isError: true` for expected error codes like 404 (task not found) or 409 (GPU blocked). These are informational.

3. **HTTP 5xx errors**: Return `isError: true` with the status code and any body content.

4. **Unexpected response format**: If the API returns non JSON when JSON is expected, return `isError: true` with the raw text (truncated to 500 chars).

5. **Validation errors**: The MCP SDK handles input validation via zod schemas. Do not add redundant validation.

### 5.9 Pre flight GPU Check Pattern

Before sending any inference, embedding, or model-management request to Ollama, the MCP server should follow this local-readiness flow:

1. Check whether Ollama is already responding.
2. If Ollama is down, check whether FreeCycle is reachable.
3. If FreeCycle is unreachable and wake-on-LAN is enabled in the MCP config, silently send multiple magic packets to the configured FreeCycle host.
4. Poll FreeCycle every 30 seconds by default, for up to 15 minutes by default, until the local server becomes usable or the configured maximum is hit.
5. If wake-on-LAN is disabled, or the server never becomes ready, return a structured "route to cloud" result instead of hanging.

```typescript
async function ensureLocalAvailability(
  freecycleClient: FreeCycleClient,
  ollamaClient: OllamaClient,
  config: McpConfig
): Promise<
  | { available: true; wakeOnLanAttempted: boolean }
  | { available: false; reason: string; suggestedRoute: "cloud"; wakeOnLanAttempted: boolean }
> {
  try {
    await ollamaClient.healthCheck();
    return { available: true, wakeOnLanAttempted: false };
  } catch (ollamaError) {
    try {
      const { data } = await freecycleClient.get<StatusResponse>("/status");

      if (
        data.status === "Blocked (Game Running)" ||
        data.status === "Cooldown" ||
        data.status === "Wake Delay" ||
        data.status === "Error"
      ) {
        return {
          available: false,
          reason: `FreeCycle is reachable but local inference is ${data.status}.`,
          suggestedRoute: "cloud",
          wakeOnLanAttempted: false,
        };
      }
    } catch {
      if (!config.wakeOnLan.enabled) {
        return {
          available: false,
          reason: `Ollama is down and wake-on-LAN is disabled. Last Ollama error: ${String(ollamaError)}`,
          suggestedRoute: "cloud",
          wakeOnLanAttempted: false,
        };
      }

      await sendWakeOnLanPackets(config.wakeOnLan);
    }

    const deadline = Date.now() + config.wakeOnLan.maxWaitMs;
    while (Date.now() <= deadline) {
      try {
        const { data } = await freecycleClient.get<StatusResponse>("/status");
        if (
          (data.status === "Available" || data.status === "Agent Task Active") &&
          data.ollama_running
        ) {
          await ollamaClient.healthCheck();
          return { available: true, wakeOnLanAttempted: true };
        }

        if (
          data.status === "Blocked (Game Running)" ||
          data.status === "Cooldown" ||
          data.status === "Wake Delay" ||
          data.status === "Error"
        ) {
          return {
            available: false,
            reason: `FreeCycle woke up but local inference is ${data.status}.`,
            suggestedRoute: "cloud",
            wakeOnLanAttempted: true,
          };
        }
      } catch {
        // Keep polling until the deadline.
      }

      await new Promise((resolve) =>
        setTimeout(resolve, config.wakeOnLan.pollIntervalMs)
      );
    }

    return {
      available: false,
      reason: "Local inference never became reachable before the configured timeout.",
      suggestedRoute: "cloud",
      wakeOnLanAttempted: true,
    };
  }
}
```

Use this check inside every local-only MCP tool. If the local stack is not available, return the cloud fallback payload instead of attempting the Ollama request.

After this readiness check passes, automatically wrap every local execution tool (`freecycle_generate`, `freecycle_chat`, `freecycle_embed`, `freecycle_pull_model`, and any benchmark or other long-running local job you add) with `POST /task/start` and `POST /task/stop`. Use one task signal pair per MCP invocation, not per nested Ollama call. This means a benchmark tool should signal once for the full benchmark run, not once per iteration.

### 5.10 Client Registration

#### Claude Code (.mcp.json in project root)

```json
{
  "mcpServers": {
    "freecycle": {
      "command": "npx",
      "args": ["tsx", "/absolute/path/to/freecycle-mcp-server/src/index.ts"],
      "env": {
        "FREECYCLE_MCP_CONFIG": "/absolute/path/to/freecycle-mcp-server/freecycle-mcp.config.json"
      }
    }
  }
}
```

#### Claude Desktop (claude_desktop_config.json)

```json
{
  "mcpServers": {
    "freecycle": {
      "command": "npx",
      "args": ["tsx", "/absolute/path/to/freecycle-mcp-server/src/index.ts"],
      "env": {
        "FREECYCLE_MCP_CONFIG": "/absolute/path/to/freecycle-mcp-server/freecycle-mcp.config.json"
      }
    }
  }
}
```

### 5.11 Testing

Create a test script that validates the server works end to end:

```typescript
// test/test-server.ts
// Run with: npx tsx test/test-server.ts

import { FreeCycleClient } from "../src/lib/freecycle-client.js";
import { OllamaClient } from "../src/lib/ollama-client.js";

async function runTests() {
  const fc = new FreeCycleClient();
  const ollama = new OllamaClient();

  console.log("=== FreeCycle MCP Server Tests ===\n");

  // Test 1: FreeCycle health
  console.log("Test 1: FreeCycle health check");
  try {
    const health = await fc.get("/health");
    console.log(`  Status: ${health.status}, Body: ${health.data}`);
    console.log("  PASS\n");
  } catch (e) {
    console.log(`  FAIL: ${e}\n`);
  }

  // Test 2: FreeCycle status
  console.log("Test 2: FreeCycle status");
  try {
    const status = await fc.get("/status");
    console.log(`  Status: ${status.status}`);
    console.log(`  GPU: ${JSON.stringify(status.data).slice(0, 200)}`);
    console.log("  PASS\n");
  } catch (e) {
    console.log(`  FAIL: ${e}\n`);
  }

  // Test 3: Ollama models
  console.log("Test 3: List Ollama models");
  try {
    const models = await ollama.get("/api/tags");
    console.log(`  Status: ${models.status}`);
    console.log(`  Models: ${JSON.stringify(models.data).slice(0, 200)}`);
    console.log("  PASS\n");
  } catch (e) {
    console.log(`  FAIL: ${e}\n`);
  }

  // Test 4: Task start/stop cycle
  console.log("Test 4: Task start/stop cycle");
  try {
    const startRes = await fc.post("/task/start", {
      task_id: "mcp-test-001",
      description: "MCP server integration test",
    });
    console.log(`  Start: ${startRes.status} ${JSON.stringify(startRes.data)}`);

    const stopRes = await fc.post("/task/stop", {
      task_id: "mcp-test-001",
    });
    console.log(`  Stop:  ${stopRes.status} ${JSON.stringify(stopRes.data)}`);
    console.log("  PASS\n");
  } catch (e) {
    console.log(`  FAIL: ${e}\n`);
  }

  // Test 5: Ollama generate (only if GPU available)
  console.log("Test 5: Ollama generate");
  try {
    const status = await fc.get<{ status: string }>("/status");
    if (status.data.status !== "Available") {
      console.log("  SKIP: GPU not available\n");
    } else {
      const gen = await ollama.post("/api/generate", {
        model: "llama3.1:8b-instruct-q4_K_M",
        prompt: "Say hello in exactly 5 words.",
        stream: false,
        options: { num_predict: 32 },
      });
      console.log(`  Status: ${gen.status}`);
      console.log(`  Response: ${JSON.stringify(gen.data).slice(0, 200)}`);
      console.log("  PASS\n");
    }
  } catch (e) {
    console.log(`  FAIL: ${e}\n`);
  }

  console.log("=== Tests Complete ===");
}

runTests().catch(console.error);
```

---

## Section 6: Negative Constraints

These are absolute rules. Violating any of these makes the implementation incorrect.

1. **No hardcoded IPs or ports.** All host addresses and port numbers must come from environment variables or configuration. Never use literal `"localhost"`, `"127.0.0.1"`, `7443`, or `11434` in tool implementation code. Only use them as default values in the config module.

2. **No streaming responses without fallback.** Always set `"stream": false` when calling Ollama APIs. If you implement streaming support as an optional feature, always provide a non streaming fallback. MCP tool responses must return complete text, not incremental chunks.

3. **No inference requests without checking FreeCycle status first.** Before calling `freecycle_generate`, `freecycle_chat`, or `freecycle_embed`, check local readiness and confirm FreeCycle is effectively available, which in the current implementation means `status` is `"Available"` or `"Agent Task Active"` and `ollama_running === true`. If the GPU is blocked, return a descriptive message immediately instead of sending a request that will fail or hang.

4. **No swallowed errors.** Every `catch` block must return a meaningful error message to the MCP client. Never return an empty string or generic "error occurred" message. Include: what failed, the likely cause, and a suggested fix.

5. **No placeholder implementations.** Every tool listed in Section 4 must have a complete, working implementation. Do not stub tools with `// TODO` comments. Do not return hardcoded mock data.

6. **No missing input validation.** Use zod schemas for all tool inputs. Every string parameter that accepts a model name must accept arbitrary strings (users may have custom models). Do not hardcode allowed model names.

7. **No synchronous blocking in the event loop.** All HTTP calls must be async. Use `await` with proper error handling. Never use synchronous HTTP clients.

8. **No secrets in source code.** If the user configures API key authentication, the key must come from an environment variable, never from a source file or config file committed to version control.

9. **No assumption about Ollama availability.** Ollama may be stopped, starting, or crashed at any time. FreeCycle controls its lifecycle. The MCP server must handle connection refused errors gracefully on every Ollama API call.

10. **No ignoring HTTP status codes.** Check response status codes on every API call. A 200 from FreeCycle and a 409 from FreeCycle mean very different things. Parse and relay the appropriate information.

11. **No excessive dependencies.** Use Node.js native `fetch` (Node 18+). Use the official MCP SDK and zod. Do not add axios, node-fetch, got, or similar HTTP libraries unless there is a specific technical need.

12. **No tool names that conflict with built in MCP tools.** All tool names must be prefixed with `freecycle_` or `ollama_` to avoid collisions.

13. **No missing automatic task signaling for local execution tools.** After readiness succeeds, every MCP tool that sends real work to Ollama must wrap that work with `POST /task/start` and `POST /task/stop`. Reserve the manual task tools for custom workflows outside the built-in local execution tools.

---

## Section 7: Output Format

When implementation is complete, deliver all of the following files. Each file must be complete, functional, and copy-pasteable.

### Required Deliverables

1. **`src/index.ts`**: Entry point. Creates the MCP server, registers all tools, connects stdio transport.

2. **`src/tools.ts`**: Registers all 13 tools: `freecycle_status`, `freecycle_health`, `freecycle_start_task`, `freecycle_stop_task`, `freecycle_check_availability`, `freecycle_list_models`, `freecycle_show_model`, `freecycle_pull_model`, `freecycle_generate`, `freecycle_chat`, `freecycle_embed`, `freecycle_evaluate_task`, and `freecycle_benchmark`.

3. **`src/freecycle-client.ts`**: HTTP client for the FreeCycle Agent API with timeout handling, typed responses, and helpers for start/stop task responses.

4. **`src/ollama-client.ts`**: HTTP client for the Ollama API with typed generate/chat/embed/model methods and configurable timeouts.

5. **`src/config.ts`**: Configuration loader reading `freecycle-mcp.config.json` with optional environment variable overrides.

6. **`src/availability.ts`**: Shared wake-and-wait readiness helper that decides when to use local inference and when to return a cloud fallback result.

7. **`src/wake-on-lan.ts`**: Magic-packet sender for wake-on-LAN.

8. **`src/task-signaling.ts`**: Helper that wraps local operations with `POST /task/start` and `POST /task/stop`, including cleanup on success, error, timeout, or early return.

9. **`freecycle-mcp.config.json`**: Default MCP runtime config including FreeCycle host, Ollama host, timeouts, and wake-on-LAN settings.

10. **`package.json`**: Complete with name, version, scripts (build, start, test), dependencies, and `"type": "module"`.

11. **`tsconfig.json`**: Configured for ES2022 target, ESM modules, strict mode, and Node.js module resolution.

12. **`README.md`**: Installation instructions, configuration reference, MCP client registration examples (Claude Code, Claude Desktop, Codex), and troubleshooting guide.

13. **`test/test-server.ts`**: End to end test script that validates connectivity and basic tool functionality.

### File Format Rules

- All TypeScript files must use ES module syntax (`import`/`export`, not `require`).
- All files must include appropriate type annotations (no `any` types unless absolutely necessary).
- JSON output from tools must be formatted with `JSON.stringify(data, null, 2)` for readability.
- The package.json `"bin"` field must point to the compiled entry point for npx compatibility.

---

## Section 8: Evaluation Rubric (8 Personality Framework)

Before considering the implementation complete, evaluate it against all 8 personality criteria. Every personality must agree the implementation is satisfactory. If any personality identifies a deficiency, fix it before delivering.

### Personality 1: Clarity

**What this personality checks:**
- Is every tool description precise enough that an LLM agent can decide when to use it without ambiguity?
- Are error messages actionable? Does each error tell the user what failed, why, and what to do next?
- Are configuration variable names self documenting?
- Is the README understandable by someone who has never seen FreeCycle before?

**Pass criteria:** A developer reading any tool description can immediately understand its purpose, inputs, and expected behavior without reading the source code.

### Personality 2: Role and Context

**What this personality checks:**
- Does the MCP server correctly represent itself as a FreeCycle integration (name, version, description)?
- Does each tool's description accurately reflect what it does in the FreeCycle/Ollama ecosystem?
- Is the relationship between FreeCycle (GPU manager) and Ollama (inference server) clear in tool descriptions?
- Do tools correctly communicate GPU availability constraints to the calling agent?

**Pass criteria:** An agent using these tools understands that GPU availability is managed by FreeCycle and that Ollama may not always be reachable.

### Personality 3: Structure

**What this personality checks:**
- Is the project organized into logical modules (tools, lib, config)?
- Are tool registrations separated by domain (status, tasks, inference, models, utility)?
- Is there a clear separation between HTTP client logic and MCP tool logic?
- Is the configuration centralized in one module?
- Are types defined once and reused?

**Pass criteria:** A new developer can find any piece of functionality within 10 seconds by following the directory structure.

### Personality 4: Examples

**What this personality checks:**
- Does the README include working examples for every MCP client registration format?
- Does the test script exercise every tool category (status, tasks, inference, models)?
- Are there inline code comments showing expected request/response shapes?
- Is there a curl equivalent shown for each FreeCycle/Ollama endpoint?

**Pass criteria:** A user can copy the README examples directly and have a working setup.

### Personality 5: Negative Constraints

**What this personality checks:**
- Are all 12 negative constraints from Section 6 satisfied?
- Is there no hardcoded IP or port outside the config module?
- Is `stream: false` always set on Ollama API calls?
- Is FreeCycle status always checked before inference?
- Are all error paths handled with meaningful messages?
- Are all tools fully implemented (no stubs)?

**Pass criteria:** A code review finds zero violations of any negative constraint.

### Personality 6: Reasoning and Decomposition

**What this personality checks:**
- Is the pre flight GPU check properly abstracted and reused?
- Is the HTTP client logic DRY (Don't Repeat Yourself)?
- Are timeout values configurable and appropriate (30s for normal calls, 10min for model pulls)?
- Does the shared availability helper properly handle edge cases (already available, timeout, unreachable, or wake-on-LAN disabled)?
- Does error handling distinguish between network errors, HTTP errors, and unexpected responses?

**Pass criteria:** The implementation handles all realistic failure modes without redundant code.

### Personality 7: Output Format

**What this personality checks:**
- Are all required deliverable files present?
- Is package.json valid and complete (scripts, deps, type: module, bin field)?
- Is tsconfig.json properly configured for ES2022 + ESM?
- Are MCP tool responses always wrapped in `{ content: [{ type: "text", text: "..." }] }` format?
- Is JSON output pretty printed for readability?

**Pass criteria:** Running `npm install && npm run build && npm start` produces a working MCP server with no additional steps.

### Personality 8: Adversarial Robustness

**What this personality checks:**
- What happens if FreeCycle is unreachable? (Expected: tools return error, not crash.)
- What happens if Ollama is unreachable? (Expected: inference tools return error with context.)
- What happens if FreeCycle returns unexpected JSON shape? (Expected: graceful degradation.)
- What happens if a model pull takes 30 minutes? (Expected: extended timeout, not abort.)
- What happens if task_start is called when GPU is blocked? (Expected: return 409 info, not crash.)
- What happens if two clients call `freecycle_generate` simultaneously? (Expected: both succeed or one gets a clear error from Ollama.)
- What happens if VRAM is at 99%? (Expected: status correctly reports a blocked state such as `"Blocked (Game Running)"` or another non-available status.)
- What happens if config env vars are missing? (Expected: use defaults, log a note.)

**Pass criteria:** The server does not crash, hang, or return empty responses under any of these conditions.

---

## Quick Reference: FreeCycle Status Values

| Status Value | Meaning | Ollama State | MCP Server Behavior |
|-------------|---------|-------------|---------------------|
| Available | GPU free, no blocks | Running | Allow inference requests |
| Agent Task Active | Another task is currently tracked | Running | Treat local inference as available, but show active-task context |
| Blocked (Game Running) | Game or blocked process detected | Stopped | Return local-unavailable result |
| Cooldown | Post-game cooldown | Stopped | Return local-unavailable result |
| Wake Delay | System just resumed from sleep | Stopped | Return local-unavailable result |
| Downloading Models | Model work is active | Usually running | Allow with caution if the current tool is compatible |
| Error | GPU monitoring failed | Unknown | Return a clear local-unavailable or degraded-readiness result |
| Initializing | FreeCycle is still starting | Unknown | Retry briefly or return local-unavailable |

## Quick Reference: Default Ports

| Service | Default Port | Config Key |
|---------|-------------|-------------|
| FreeCycle Agent API | 7443 | `freecycle.port` |
| Ollama API | 11434 | `ollama.port` |
| Wake-on-LAN UDP | 9 | `wakeOnLan.port` |

## Quick Reference: Wake-on-LAN Defaults

| Setting | Default |
|---------|---------|
| `wakeOnLan.enabled` | `false` |
| `wakeOnLan.packetCount` | `5` |
| `wakeOnLan.packetIntervalMs` | `250` |
| `wakeOnLan.pollIntervalMs` | `30000` |
| `wakeOnLan.maxWaitMs` | `900000` |

## Quick Reference: Available Models (Default Config)

| Model | Purpose | Typical VRAM |
|-------|---------|-------------|
| llama3.1:8b-instruct-q4_K_M | General LLM inference, chat, text generation | ~5 GB |
| nomic-embed-text | Text embeddings for RAG and semantic search | ~0.3 GB |
