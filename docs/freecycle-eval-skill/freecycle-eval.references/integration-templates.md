# Integration Templates

Load this reference when the user asks for concrete code or configuration patterns after the evaluation is complete.

**Required:** All Python integrations must use `freecycle_client.py` (`FreeCycleClient`). It implements TOFU TLS pinning, wake-on-LAN, multi-server routing, task signaling, and caching — the same security and lifecycle behavior as the MCP tools. Do not bypass it with raw HTTP calls; doing so skips critical security features and would require reimplementing the entire client from scratch.

If `freecycle_client.py` is missing, the MCP server installation is incomplete. Direct the user to reinstall from source: https://github.com/Heretyc/FreeCycle/tree/main/mcp-server

## Template 1: Check Status and Generate

```python
from freecycle_client import FreeCycleClient

client = FreeCycleClient()  # auto-discovers freecycle-mcp.config.json

def generate(prompt, prefer_local=True):
    """Generate a response, routing to local or cloud based on availability."""
    if prefer_local:
        status = client.status_sync()
        if status.get("ollama_running"):
            # generate() handles task signaling automatically
            result = client.generate_sync(
                model="llama3.1:8b-instruct-q4_K_M",
                prompt=prompt,
            )
            return result.get("response", "")
    return call_cloud_api(prompt)

def embed_text(text):
    """Generate embeddings using the local engine."""
    result = client.embed_sync(model="nomic-embed-text", input=text)
    return result.get("embeddings", [])

def run_benchmark(model, prompt, iterations=5):
    """Benchmark a local model."""
    return client.benchmark_sync(model=model, prompt=prompt, iterations=iterations)
```

## Template 2: Claude Code MCP Configuration

```json
{
  "mcpServers": {
    "freecycle": {
      "command": "npx",
      "args": ["-y", "freecycle-mcp-server"],
      "env": {
        "FREECYCLE_MCP_CONFIG": "C:/path/to/freecycle/mcp-server/freecycle-mcp.config.json"
      }
    }
  }
}
```

Example `freecycle-mcp.config.json` for a remote FreeCycle host with wake-on-LAN:

```json
{
  "freecycle": {
    "host": "192.168.1.10",
    "port": 7443
  },
  "inference": {
    "host": "192.168.1.10",
    "port": 7443
  },
  "wakeOnLan": {
    "enabled": true,
    "macAddress": "AA:BB:CC:DD:EE:FF",
    "broadcastAddress": "192.168.1.255",
    "port": 9,
    "packetCount": 5,
    "packetIntervalSecs": 0.25,
    "pollIntervalSecs": 30,
    "maxWaitSecs": 900
  }
}
```

## Template 3: Automatic Routing Configuration

```yaml
routing_rules:
  - match:
      stage: embed
    target: local
    model: nomic-embed-text

  - match:
      stage: draft
      prompt_tokens_max: 500
    target: local
    model: "llama3.1:8b-instruct-q4_K_M"

  - match:
      stage: final_rewrite
    target: cloud
    model: claude-sonnet-4-20250514

  fallback:
    target: cloud
    model: claude-sonnet-4-20250514
    reason: "FreeCycle unavailable or task too complex for local model"
```

## Template 4: Agentic Workflow with FreeCycle Python Client

```python
import time
from freecycle_client import FreeCycleClient

class FreeCycleAgent:
    """Agent that checks FreeCycle health before each operation.

    Uses the companion freecycle_client.py for all FreeCycle interactions,
    which handles TLS/TOFU pinning, wake-on-LAN, multi-server routing,
    and task signaling automatically.
    """

    def __init__(self, config_path=None):
        self.client = FreeCycleClient(config_path=config_path)
        self.task_id = f"agent-{int(time.time())}"

    def is_local_available(self):
        """Check FreeCycle status and return availability info."""
        try:
            data = self.client.status_sync()
            return {
                "available": data.get("ollama_running", False),
                "status": data.get("status", "Unknown"),
                "vram_percent": data.get("vram_percent", 100),
                "blocking_processes": data.get("blocking_processes", [])
            }
        except Exception:
            return {"available": False, "status": "Unreachable"}

    def route_task(self, task_type, privacy_mode="normal"):
        """Decide where to route a task based on current conditions."""
        info = self.is_local_available()

        if privacy_mode == "local_only":
            if info["available"]:
                return "local"
            raise RuntimeError(
                f"Local-only task requires local processing but "
                f"FreeCycle status is: {info['status']}"
            )

        if info["available"] and task_type in ("embeddings", "classification", "summarization"):
            return "local"

        return "cloud"

    def generate(self, prompt, model="llama3.1:8b-instruct-q4_K_M"):
        """Generate text locally. Task signaling is handled automatically."""
        return self.client.generate_sync(model=model, prompt=prompt)

    def embed(self, text, model="nomic-embed-text"):
        """Generate embeddings locally. Task signaling is handled automatically."""
        return self.client.embed_sync(model=model, input=text)
```
