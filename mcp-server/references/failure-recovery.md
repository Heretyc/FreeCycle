# MCP Failure Recovery

Use this file for fast diagnosis with minimal retry churn and minimal cloud-token burn.

## Recovery Table

| Symptom | Likely Cause | Fastest Next Step |
|---|---|---|
| MCP server starts with the wrong host or port | Missing or wrong config path. Env overrides not applied | Check `FREECYCLE_MCP_CONFIG`, then verify `freecycle.host`, `freecycle.port`, `ollama.host`, and `ollama.port` |
| MCP server uses `localhost` unexpectedly | Config file was not loaded, or host overrides were absent | Open `references/config-and-timeouts.md` and validate config precedence |
| `node dist/index.js` fails at startup | Missing build output or missing dependencies | Run `npm install`, then `npm run build`, then confirm `dist/index.js` exists |
| `freecycle_check_availability` returns a cloud fallback payload | Local inference is down, FreeCycle is unreachable, or wake-and-wait timed out | Read the returned `message`, then check wake-on-LAN settings and host reachability before retrying |
| Availability result reports `Blocked (Game Running)`, `Cooldown`, or `Wake Delay` | FreeCycle is reachable, but GPU policy is intentionally preventing local inference | Wait, or route only that stage to cloud. Do not keep retrying local inference immediately |
| `freecycle_pull_model` returns `403` | The FreeCycle tray lock is off | Ask the local user to enable "Remote Model Installs" from the tray, then retry |
| `freecycle_pull_model` returns `409` | GPU state currently blocks installs | Check `freecycle_status` for the active state and blocking processes |
| `freecycle_pull_model` returns `503` | Local inference is not running on the FreeCycle machine | Call `freecycle_check_availability`, then retry after local readiness succeeds |
| Generate or chat returns `HTTP 404 ... model not found` | Requested model is not installed, or the name is wrong | Call `freecycle_list_models` or `freecycle_show_model`, then use the exact installed name or pull the model first |
| Generate, chat, or embed times out | Inference timeout too low, model too slow, or server overloaded | Increase `FREECYCLE_INFERENCE_TIMEOUT_SECS` or reduce model and prompt size |
| Control-plane request times out | Request timeout too low, or FreeCycle host is unreachable | Increase `FREECYCLE_REQUEST_TIMEOUT_SECS` only after checking connectivity |
| Response says non-JSON | Wrong endpoint, reverse proxy issue, or unexpected service on the target port | Confirm the FreeCycle and Ollama hosts and ports point to the intended services |

## Cheap Diagnostic Order

1. `freecycle_check_availability`
2. `freecycle_status`
3. `freecycle_list_models`
4. Retry the intended tool only after the above signals are clear

## Cloud-Token Guardrails

- Prefer one availability probe over repeated generate retries.
- Prefer `freecycle_list_models` over guessing model names.
- Treat `403`, `409`, `503`, `Blocked`, `Cooldown`, and `Wake Delay` as control-plane guidance, not as prompts for more reasoning.
- If the failure is clearly environmental, stop the agent loop and fix configuration or tray state first.

## Related Files

- `../RAG_INDEX.md`
- `setup-and-registration.md`
- `config-and-timeouts.md`
- `tools-and-routing.md`
