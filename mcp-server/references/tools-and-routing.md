# MCP Tools And Routing

Use this file for tool selection, shared readiness behavior, and the MCP server's local versus cloud routing semantics.

## Shared Local Readiness Flow

These local-only tools all use the same readiness helper before they touch Ollama:

`freecycle_status`, `freecycle_health`, `freecycle_start_task`, `freecycle_stop_task`, `freecycle_check_availability`, `freecycle_list_models`, `freecycle_show_model`, `freecycle_pull_model`, `freecycle_generate`, `freecycle_chat`, `freecycle_embed`, `freecycle_evaluate_task`, and `freecycle_benchmark`.

The readiness helper:

1. Checks FreeCycle Inference API health.
2. Falls back to FreeCycle status checks if local inference is down.
3. Sends wake-on-LAN packets when configured and needed.
4. Returns a structured cloud-fallback payload when local inference is still unavailable.

## Tool Groups

| Group | Tools | Notes |
|---|---|---|
| Status and health | `freecycle_status`, `freecycle_health`, `freecycle_check_availability` | Use these before inference when local inference might be asleep or blocked |
| Manual task signaling | `freecycle_start_task`, `freecycle_stop_task` | For custom workflows outside built-in tracked tools |
| Model inventory | `freecycle_list_models`, `freecycle_show_model` | Use before generation if model fit is uncertain |
| Model install | `freecycle_pull_model` | Calls FreeCycle `/models/install` endpoint |
| Inference | `freecycle_generate`, `freecycle_chat`, `freecycle_embed` | Local execution via FreeCycle Inference API |
| Routing and evaluation | `freecycle_evaluate_task` | Coarse recommendation only |
| Benchmarking | `freecycle_benchmark` | Benchmarks a local model over repeated generate calls |

## Auto-Tracked Tools

These tools automatically signal `POST /task/start` and `POST /task/stop` so the FreeCycle tray reflects active MCP work:

- `freecycle_pull_model`
- `freecycle_generate`
- `freecycle_chat`
- `freecycle_embed`
- `freecycle_benchmark`

**Task Description Constraints:**

Automatically-generated task descriptions must be exactly 30–40 characters. The `task-signaling.ts` module:

1. Builds a base description from operation label and model name
2. Truncates to 40 chars if over
3. Pads with meaningful text (e.g., " (local)", " via API") to reach 30 chars if under
4. Validates against the server-side padding rules before sending
5. Falls back to `"MCP task via FreeCycle local API"` (32 chars) if construction fails

## Important Behavior

- `freecycle_pull_model` is tray-gated. The local user must enable remote model installs from the FreeCycle tray before the pull starts.
- `freecycle_evaluate_task` is a coarse routing helper, not a model-fit oracle.
- `freecycle_check_availability` is the cheapest first probe when a workflow wants to avoid wasted retries.
- `freecycle_list_models` should precede generation when the requested model name is uncertain.

## Fast Selection Guide

| Goal | Start With |
|---|---|
| Confirm the local stack is reachable | `freecycle_check_availability` |
| Inspect current FreeCycle status and blocking processes | `freecycle_status` |
| Find installed local models | `freecycle_list_models` |
| Pull a new model | `freecycle_pull_model` |
| Run local text generation | `freecycle_generate` |
| Decide whether to route locally or to cloud | `freecycle_evaluate_task` |

## Related Files

- `../RAG_INDEX.md`
- `config-and-timeouts.md`
- `failure-recovery.md`
