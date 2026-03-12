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

Simple uptime check. Returns plain text, not JSON.

**Response (200 OK):**
```
ok
```

#### GET /status

Returns the full system state as JSON.

**Response (200 OK):**
```json
{
  "status": "available",
  "ollama_running": true,
  "vram_used_mb": 1024,
  "vram_total_mb": 8192,
  "vram_percent": 12.5,
  "active_task_id": null,
  "active_task_description": null,
  "local_ip": "192.168.1.10",
  "ollama_port": 11434,
  "blocking_processes": [],
  "model_status": {
    "llama3.1:8b-instruct-q4_K_M": "ready",
    "nomic-embed-text": "ready"
  }
}
```

**Field Reference:**

| Field | Type | Description |
|-------|------|-------------|
| status | string | One of: `"available"`, `"blocked_by_process"`, `"blocked_by_cooldown"`, `"blocked_by_vram"`, `"error"` |
| ollama_running | boolean | Whether Ollama process is alive and healthy |
| vram_used_mb | number | Total VRAM in use across all processes (MB) |
| vram_total_mb | number | Total GPU VRAM capacity (MB) |
| vram_percent | number | VRAM usage percentage (one decimal place) |
| active_task_id | string or null | Task ID if an agent task is active |
| active_task_description | string or null | Human readable task description if active |
| local_ip | string | Machine's local LAN IP address |
| ollama_port | number | Port Ollama is listening on |
| blocking_processes | string[] | Process names currently triggering a block |
| model_status | object | Map of model name to status string |

**Model status values:** `"not_downloaded"`, `"downloading"`, `"ready"`, `"error"`

**GPU status values explained:**
- `"available"`: GPU is free, Ollama is running, ready for work.
- `"blocked_by_process"`: A blacklisted process (game) is running. Ollama is stopped.
- `"blocked_by_cooldown"`: Game exited but cooldown timer (default 30 min) has not expired. Ollama stays stopped.
- `"blocked_by_vram"`: Non whitelisted VRAM usage exceeds threshold (default 50%). Ollama is stopped.
- `"error"`: GPU monitoring failed. Ollama state is uncertain.

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
  "task_id": "abc-123"
}
```

**Response (409 Conflict, GPU is blocked):**
```json
{
  "ok": false,
  "error": "gpu_blocked",
  "status": "blocked_by_process",
  "blocking_processes": ["VRChat.exe"]
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
  "ok": true
}
```

**Response (404 Not Found, task ID mismatch or no active task):**
```json
{
  "ok": false,
  "error": "task_not_found"
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

The MCP server must implement at minimum the following 14 tools. Each tool listing includes: name, description, input schema, and behavioral requirements.

### 4.1 FreeCycle Status and Health Tools

#### Tool: `freecycle_status`

Get the full FreeCycle system status including GPU state, VRAM usage, Ollama health, active tasks, and model readiness.

**Input Schema:**
```json
{
  "type": "object",
  "properties": {},
  "required": []
}
```
No input parameters. Returns the full /status JSON.

**Behavior:** Call `GET /status` on the FreeCycle Agent API. Return the full JSON response. If the request fails, return an error with the HTTP status code and message.

#### Tool: `freecycle_health`

Quick health check to verify FreeCycle is reachable and running.

**Input Schema:**
```json
{
  "type": "object",
  "properties": {},
  "required": []
}
```

**Behavior:** Call `GET /health` on the FreeCycle Agent API. Return `{ "healthy": true }` on 200 OK. On failure, return `{ "healthy": false, "error": "<message>" }`.

#### Tool: `freecycle_gpu_available`

Check whether the GPU is available for inference work. This is a convenience wrapper that checks status and returns a simple boolean with context.

**Input Schema:**
```json
{
  "type": "object",
  "properties": {},
  "required": []
}
```

**Behavior:** Call `GET /status`. Return:
```json
{
  "available": true,
  "status": "available",
  "ollama_running": true,
  "vram_percent": 12.5,
  "blocking_processes": []
}
```
The `available` field is `true` only when `status === "available"` AND `ollama_running === true`.

### 4.2 Task Management Tools

#### Tool: `freecycle_task_start`

Signal to FreeCycle that an agent task is beginning. This turns the tray icon blue and registers the task.

**Input Schema:**
```json
{
  "type": "object",
  "properties": {
    "task_id": {
      "type": "string",
      "description": "Unique identifier for this task. Use a descriptive slug or UUID."
    },
    "description": {
      "type": "string",
      "description": "Human readable description of what the task is doing. Shown in the FreeCycle tray tooltip."
    }
  },
  "required": ["task_id", "description"]
}
```

**Behavior:** Call `POST /task/start`. On 200, return the success response. On 409 (GPU blocked), return the conflict response including blocking_processes. On network error, return an error.

#### Tool: `freecycle_task_stop`

Signal to FreeCycle that an agent task has completed. Reverts the tray icon.

**Input Schema:**
```json
{
  "type": "object",
  "properties": {
    "task_id": {
      "type": "string",
      "description": "The task_id that was used in freecycle_task_start."
    }
  },
  "required": ["task_id"]
}
```

**Behavior:** Call `POST /task/stop`. On 200, return success. On 404 (task not found), return the error response. On network error, return an error.

### 4.3 Ollama Inference Tools

#### Tool: `ollama_generate`

Generate a text completion using a local Ollama model.

**Input Schema:**
```json
{
  "type": "object",
  "properties": {
    "prompt": {
      "type": "string",
      "description": "The prompt text to send to the model."
    },
    "model": {
      "type": "string",
      "description": "Model name to use. Default: llama3.1:8b-instruct-q4_K_M",
      "default": "llama3.1:8b-instruct-q4_K_M"
    },
    "temperature": {
      "type": "number",
      "description": "Sampling temperature (0.0 to 2.0). Default: 0.7",
      "default": 0.7
    },
    "max_tokens": {
      "type": "number",
      "description": "Maximum tokens to generate. Default: 512",
      "default": 512
    },
    "system": {
      "type": "string",
      "description": "Optional system prompt to prepend."
    }
  },
  "required": ["prompt"]
}
```

**Behavior:** Call `POST /api/generate` with `stream: false`. Map `max_tokens` to `num_predict` in the options object. If `system` is provided, include it in the request body. Return the model's response text and usage statistics (eval_count, total_duration).

**CRITICAL:** Always set `stream: false`. Always check FreeCycle status before sending inference requests (see Section 6, Negative Constraints).

#### Tool: `ollama_chat`

Send a chat conversation to a local Ollama model with message history.

**Input Schema:**
```json
{
  "type": "object",
  "properties": {
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
      },
      "description": "Array of chat messages in chronological order."
    },
    "model": {
      "type": "string",
      "description": "Model name to use. Default: llama3.1:8b-instruct-q4_K_M",
      "default": "llama3.1:8b-instruct-q4_K_M"
    },
    "temperature": {
      "type": "number",
      "description": "Sampling temperature (0.0 to 2.0). Default: 0.7",
      "default": 0.7
    },
    "max_tokens": {
      "type": "number",
      "description": "Maximum tokens to generate. Default: 512",
      "default": 512
    }
  },
  "required": ["messages"]
}
```

**Behavior:** Call `POST /api/chat` with `stream: false`. Return the assistant message content and usage statistics.

#### Tool: `ollama_embed`

Generate vector embeddings for one or more text inputs using a local embedding model.

**Input Schema:**
```json
{
  "type": "object",
  "properties": {
    "input": {
      "oneOf": [
        { "type": "string" },
        { "type": "array", "items": { "type": "string" } }
      ],
      "description": "A single string or array of strings to embed."
    },
    "model": {
      "type": "string",
      "description": "Embedding model to use. Default: nomic-embed-text",
      "default": "nomic-embed-text"
    }
  },
  "required": ["input"]
}
```

**Behavior:** Call `POST /api/embed`. Return the embeddings array and metadata (model, total_duration).

### 4.4 Model Management Tools

#### Tool: `ollama_list_models`

List all models currently downloaded on the Ollama instance.

**Input Schema:**
```json
{
  "type": "object",
  "properties": {},
  "required": []
}
```

**Behavior:** Call `GET /api/tags`. Return the models array with name, size, parameter_size, quantization_level, and modified_at for each.

#### Tool: `ollama_model_info`

Get detailed information about a specific model.

**Input Schema:**
```json
{
  "type": "object",
  "properties": {
    "name": {
      "type": "string",
      "description": "Full model name including tag, e.g. llama3.1:8b-instruct-q4_K_M"
    }
  },
  "required": ["name"]
}
```

**Behavior:** Call `POST /api/show`. Return model details, parameters, template, and family information.

#### Tool: `ollama_pull_model`

Download a model to the Ollama instance.

**Input Schema:**
```json
{
  "type": "object",
  "properties": {
    "name": {
      "type": "string",
      "description": "Model name to pull, e.g. mistral:7b-instruct-q4_K_M"
    }
  },
  "required": ["name"]
}
```

**Behavior:** Call `POST /api/pull` with `stream: false`. This is a blocking call that may take several minutes for large models. Return the final status. Consider setting a generous HTTP timeout (10+ minutes).

#### Tool: `ollama_loaded_models`

List models currently loaded into GPU memory.

**Input Schema:**
```json
{
  "type": "object",
  "properties": {},
  "required": []
}
```

**Behavior:** Call `GET /api/ps`. Return the list of loaded models with their VRAM usage and expiry time.

### 4.5 Utility Tools

#### Tool: `ollama_delete_model`

Delete a model from the Ollama instance.

**Input Schema:**
```json
{
  "type": "object",
  "properties": {
    "name": {
      "type": "string",
      "description": "Full model name including tag to delete."
    }
  },
  "required": ["name"]
}
```

**Behavior:** Call `DELETE /api/delete`. Return success or error.

#### Tool: `freecycle_wait_for_gpu`

Poll FreeCycle status until the GPU becomes available or a timeout is reached. Useful for workflows that need to wait for a game to exit.

**Input Schema:**
```json
{
  "type": "object",
  "properties": {
    "timeout_seconds": {
      "type": "number",
      "description": "Maximum seconds to wait. Default: 300 (5 minutes).",
      "default": 300
    },
    "poll_interval_seconds": {
      "type": "number",
      "description": "Seconds between status checks. Default: 10.",
      "default": 10
    }
  },
  "required": []
}
```

**Behavior:** Poll `GET /status` at the specified interval. Return immediately if GPU is already available. If timeout is reached, return the last known status with `{ "timed_out": true }`. Cap timeout at 3600 seconds. Cap poll interval minimum at 5 seconds.

---

## Section 5: Implementation Instructions

### 5.1 Project Structure (TypeScript/Node.js)

```
freecycle-mcp-server/
  src/
    index.ts              # Entry point, MCP server setup, stdio transport
    tools/
      status.ts           # freecycle_status, freecycle_health, freecycle_gpu_available
      tasks.ts            # freecycle_task_start, freecycle_task_stop
      inference.ts        # ollama_generate, ollama_chat, ollama_embed
      models.ts           # ollama_list_models, ollama_model_info, ollama_pull_model,
                          #   ollama_loaded_models, ollama_delete_model
      wait.ts             # freecycle_wait_for_gpu
    lib/
      freecycle-client.ts # HTTP client for FreeCycle Agent API
      ollama-client.ts    # HTTP client for Ollama API
      config.ts           # Configuration loading (env vars, defaults)
      errors.ts           # Error types and formatting
  package.json
  tsconfig.json
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

The server must read configuration from environment variables with sensible defaults:

| Variable | Default | Description |
|----------|---------|-------------|
| FREECYCLE_HOST | localhost | FreeCycle machine hostname or IP |
| FREECYCLE_AGENT_PORT | 7443 | FreeCycle Agent Signal API port |
| FREECYCLE_OLLAMA_PORT | 11434 | Ollama API port |
| FREECYCLE_TIMEOUT_MS | 30000 | Default HTTP request timeout in milliseconds |
| FREECYCLE_PULL_TIMEOUT_MS | 600000 | Timeout for model pull operations (10 min) |

Never hardcode IPs or ports. Always read from config.

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
              hint: "Verify FreeCycle is running and FREECYCLE_HOST / FREECYCLE_AGENT_PORT are correct.",
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
  "freecycle_task_start",
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
                ok: false,
                error: "gpu_blocked",
                ...response.data,
                hint: "GPU is currently in use. Wait for the blocking process to exit or use freecycle_wait_for_gpu.",
              }),
            },
          ],
        };
      }

      return {
        content: [
          {
            type: "text",
            text: JSON.stringify(response.data),
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

Create a thin HTTP client wrapper for each API:

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
    this.baseUrl = `http://${config.freecycleHost}:${config.agentPort}`;
    this.timeoutMs = config.timeoutMs;
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

Before sending any inference or embedding request to Ollama, the MCP server should check FreeCycle status:

```typescript
async function ensureGpuAvailable(
  freecycleClient: FreeCycleClient
): Promise<{ available: true } | { available: false; reason: string }> {
  try {
    const { data } = await freecycleClient.get<StatusResponse>("/status");

    if (data.status !== "available") {
      return {
        available: false,
        reason: `GPU is ${data.status}. Blocking processes: ${data.blocking_processes?.join(", ") || "none"}.`,
      };
    }

    if (!data.ollama_running) {
      return {
        available: false,
        reason: "Ollama is not running. FreeCycle may be starting it.",
      };
    }

    return { available: true };
  } catch {
    // If we cannot reach FreeCycle, still try Ollama directly.
    // FreeCycle may be down but Ollama could be running independently.
    return { available: true };
  }
}
```

Use this check inside `ollama_generate`, `ollama_chat`, and `ollama_embed`. If the GPU is not available, return a descriptive message instead of attempting the request (which would fail or hang).

### 5.10 Client Registration

#### Claude Code (.mcp.json in project root)

```json
{
  "mcpServers": {
    "freecycle": {
      "command": "npx",
      "args": ["tsx", "/absolute/path/to/freecycle-mcp-server/src/index.ts"],
      "env": {
        "FREECYCLE_HOST": "localhost",
        "FREECYCLE_AGENT_PORT": "7443",
        "FREECYCLE_OLLAMA_PORT": "11434"
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
        "FREECYCLE_HOST": "localhost",
        "FREECYCLE_AGENT_PORT": "7443",
        "FREECYCLE_OLLAMA_PORT": "11434"
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
    if (status.data.status !== "available") {
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

3. **No inference requests without checking FreeCycle status first.** Before calling `ollama_generate`, `ollama_chat`, or `ollama_embed`, check `GET /status` to verify `status === "available"` and `ollama_running === true`. If the GPU is blocked, return a descriptive message immediately instead of sending a request that will fail or hang.

4. **No swallowed errors.** Every `catch` block must return a meaningful error message to the MCP client. Never return an empty string or generic "error occurred" message. Include: what failed, the likely cause, and a suggested fix.

5. **No placeholder implementations.** Every tool listed in Section 4 must have a complete, working implementation. Do not stub tools with `// TODO` comments. Do not return hardcoded mock data.

6. **No missing input validation.** Use zod schemas for all tool inputs. Every string parameter that accepts a model name must accept arbitrary strings (users may have custom models). Do not hardcode allowed model names.

7. **No synchronous blocking in the event loop.** All HTTP calls must be async. Use `await` with proper error handling. Never use synchronous HTTP clients.

8. **No secrets in source code.** If the user configures API key authentication, the key must come from an environment variable, never from a source file or config file committed to version control.

9. **No assumption about Ollama availability.** Ollama may be stopped, starting, or crashed at any time. FreeCycle controls its lifecycle. The MCP server must handle connection refused errors gracefully on every Ollama API call.

10. **No ignoring HTTP status codes.** Check response status codes on every API call. A 200 from FreeCycle and a 409 from FreeCycle mean very different things. Parse and relay the appropriate information.

11. **No excessive dependencies.** Use Node.js native `fetch` (Node 18+). Use the official MCP SDK and zod. Do not add axios, node-fetch, got, or similar HTTP libraries unless there is a specific technical need.

12. **No tool names that conflict with built in MCP tools.** All tool names must be prefixed with `freecycle_` or `ollama_` to avoid collisions.

---

## Section 7: Output Format

When implementation is complete, deliver all of the following files. Each file must be complete, functional, and copy-pasteable.

### Required Deliverables

1. **`src/index.ts`**: Entry point. Creates the MCP server, registers all tools, connects stdio transport.

2. **`src/tools/status.ts`**: Exports tool registration functions for `freecycle_status`, `freecycle_health`, `freecycle_gpu_available`.

3. **`src/tools/tasks.ts`**: Exports tool registration functions for `freecycle_task_start`, `freecycle_task_stop`.

4. **`src/tools/inference.ts`**: Exports tool registration functions for `ollama_generate`, `ollama_chat`, `ollama_embed`.

5. **`src/tools/models.ts`**: Exports tool registration functions for `ollama_list_models`, `ollama_model_info`, `ollama_pull_model`, `ollama_loaded_models`, `ollama_delete_model`.

6. **`src/tools/wait.ts`**: Exports tool registration function for `freecycle_wait_for_gpu`.

7. **`src/lib/freecycle-client.ts`**: HTTP client for FreeCycle Agent API with timeout, error handling, and typed responses.

8. **`src/lib/ollama-client.ts`**: HTTP client for Ollama API with timeout, error handling, typed responses, and configurable pull timeout.

9. **`src/lib/config.ts`**: Configuration loader reading from environment variables with defaults.

10. **`src/lib/errors.ts`**: Error formatting utilities.

11. **`package.json`**: Complete with name, version, scripts (build, start, test), dependencies, and `"type": "module"`.

12. **`tsconfig.json`**: Configured for ES2022 target, ESM modules, strict mode, and Node.js module resolution.

13. **`README.md`**: Installation instructions, configuration reference, MCP client registration examples (Claude Code, Claude Desktop, Codex), and troubleshooting guide.

14. **`test/test-server.ts`**: End to end test script that validates connectivity and basic tool functionality.

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
- Does `freecycle_wait_for_gpu` properly handle edge cases (already available, timeout, unreachable)?
- Does error handling distinguish between network errors, HTTP errors, and unexpected responses?

**Pass criteria:** The implementation handles all realistic failure modes without redundant code.

### Personality 7: Output Format

**What this personality checks:**
- Are all 14 required deliverable files present?
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
- What happens if two clients call ollama_generate simultaneously? (Expected: both succeed or one gets a clear error from Ollama.)
- What happens if VRAM is at 99%? (Expected: status correctly reports blocked_by_vram.)
- What happens if config env vars are missing? (Expected: use defaults, log a note.)

**Pass criteria:** The server does not crash, hang, or return empty responses under any of these conditions.

---

## Quick Reference: FreeCycle Status Values

| Status Value | Meaning | Ollama State | MCP Server Behavior |
|-------------|---------|-------------|---------------------|
| available | GPU free, no blocks | Running | Allow inference requests |
| blocked_by_process | Game/app detected | Stopped | Reject inference, report blocker |
| blocked_by_cooldown | Post game cooldown | Stopped | Reject inference, report wait time |
| blocked_by_vram | VRAM threshold exceeded | Stopped | Reject inference, report usage |
| error | GPU monitoring failed | Unknown | Warn, attempt Ollama directly |

## Quick Reference: Default Ports

| Service | Default Port | Env Variable |
|---------|-------------|-------------|
| FreeCycle Agent API | 7443 | FREECYCLE_AGENT_PORT |
| Ollama API | 11434 | FREECYCLE_OLLAMA_PORT |

## Quick Reference: Available Models (Default Config)

| Model | Purpose | Typical VRAM |
|-------|---------|-------------|
| llama3.1:8b-instruct-q4_K_M | General LLM inference, chat, text generation | ~5 GB |
| nomic-embed-text | Text embeddings for RAG and semantic search | ~0.3 GB |
