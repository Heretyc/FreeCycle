# FreeCycle MCP RAG Index

Purpose: token-conscious entrypoint for MCP server questions. Read this file first, then open only one targeted reference file unless the task clearly spans multiple areas.

## Retrieval Rules

1. Start here.
2. Read one reference file based on the question map below.
3. Read `mcp-server/src/*.ts` only when the reference file is not enough or the task requires implementation detail.
4. Read the full `mcp-server/README.md` only when a human wants narrative setup docs.

## Question Map

| Question | Read This File |
|---|---|
| How do I install or register the MCP server? | `references/setup-and-registration.md` |
| Which config key or environment variable controls this behavior? | `references/config-and-timeouts.md` |
| Which tool should I call and what does it do? | `references/tools-and-routing.md` |
| Why did the server fail, route to cloud, reject a pull, or report model errors? | `references/failure-recovery.md` |
| I need implementation detail for a specific behavior | `src/config.ts`, `src/availability.ts`, `src/freecycle-client.ts`, `src/ollama-client.ts`, `src/task-signaling.ts`, or `src/tools.ts` |

## Document Index

| File | Scope | Typical Use |
|---|---|---|
| `README.md` | Short human-facing entrypoint | Basic install and registration |
| `references/setup-and-registration.md` | Install, build, Claude/Codex registration, first-run checks | New deployment or workstation setup |
| `references/config-and-timeouts.md` | Config file location, key settings, env overrides, precedence rules | Host, port, timeout, or wake-on-LAN tuning |
| `references/tools-and-routing.md` | Tool groups, shared readiness flow, auto-tracked tools, tray-gated pull behavior | Tool selection or workflow design |
| `references/failure-recovery.md` | Symptom-to-action recovery table | Fast diagnosis with minimal cloud-token burn |

## Quick Facts

| Topic | Value |
|---|---|
| FreeCycle default port | `7443` |
| Ollama default port | `11434` |
| Default config path | `mcp-server/freecycle-mcp.config.json` |
| External config override | `FREECYCLE_MCP_CONFIG` |
| Auto-tracked tools | `freecycle_pull_model`, `freecycle_generate`, `freecycle_chat`, `freecycle_embed`, `freecycle_benchmark` |
| Tray-gated remote install endpoint | FreeCycle `POST /models/install` |
| Coarse routing helper | `freecycle_evaluate_task` |

## Source Map

| Behavior | Primary File |
|---|---|
| Config loading and env overrides | `src/config.ts` |
| Wake-and-wait readiness logic | `src/availability.ts` |
| FreeCycle HTTP requests | `src/freecycle-client.ts` |
| Ollama HTTP requests | `src/ollama-client.ts` |
| Automatic task start and stop wrapping | `src/task-signaling.ts` |
| Tool registration and return payloads | `src/tools.ts` |
