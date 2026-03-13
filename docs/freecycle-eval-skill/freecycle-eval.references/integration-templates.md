# Integration Templates

Load this reference when the user asks for concrete code or configuration patterns after the evaluation is complete.

## Template 1: Check FreeCycle Status Before Choosing Model

```python
import requests

FREECYCLE_URL = "http://localhost:7443"
OLLAMA_URL = "http://localhost:11434"

def get_freecycle_status():
    """Check if FreeCycle reports Ollama as available."""
    try:
        resp = requests.get(f"{FREECYCLE_URL}/status", timeout=2)
        data = resp.json()
        return data.get("ollama_running", False), data.get("status", "Unknown")
    except Exception:
        return False, "Unreachable"

def generate(prompt, prefer_local=True):
    """Generate a response, routing to local or cloud based on availability."""
    ollama_available, status = get_freecycle_status()

    if prefer_local and ollama_available:
        requests.post(f"{FREECYCLE_URL}/task/start", json={
            "task_id": "agent-gen-001",
            "description": "Generating response"
        })
        try:
            resp = requests.post(f"{OLLAMA_URL}/api/generate", json={
                "model": "llama3.1:8b-instruct-q4_K_M",
                "prompt": prompt,
                "stream": False
            }, timeout=60)
            return resp.json().get("response", "")
        finally:
            requests.post(f"{FREECYCLE_URL}/task/stop", json={
                "task_id": "agent-gen-001"
            })
    return call_cloud_api(prompt)
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
  "ollama": {
    "host": "192.168.1.10",
    "port": 11434
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

## Template 4: Agentic Workflow with FreeCycle Health Check

```python
import requests
import time

class FreeCycleAgent:
    """Agent that checks FreeCycle health before each operation."""

    def __init__(self, freecycle_url="http://localhost:7443",
                 ollama_url="http://localhost:11434"):
        self.freecycle_url = freecycle_url
        self.ollama_url = ollama_url
        self.task_id = f"agent-{int(time.time())}"

    def is_local_available(self):
        """Check FreeCycle status and return availability info."""
        try:
            resp = requests.get(f"{self.freecycle_url}/status", timeout=2)
            data = resp.json()
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

    def signal_start(self, description):
        try:
            requests.post(f"{self.freecycle_url}/task/start", json={
                "task_id": self.task_id,
                "description": description
            }, timeout=2)
        except Exception:
            pass

    def signal_stop(self):
        try:
            requests.post(f"{self.freecycle_url}/task/stop", json={
                "task_id": self.task_id
            }, timeout=2)
        except Exception:
            pass
```
