# Static Persistent Code Patterns

Use this reference when the workflow is recurring enough that writing stable code, config, or test fixtures will save tool calls and cloud tokens.

## Principle

Static persistent code should be written to minimize tool calls and reduce cloud token usage whenever possible.

If a decision is stable, deterministic, or cheap to encode, capture it in code or config instead of asking an agent or cloud model to rediscover it every time.

## Good Fits for Persistent Code

- stage routing rules that only change after benchmarking
- prompt-complexity heuristics
- model allowlists and denylists
- benchmark datasets and scoring harnesses
- output validators and retry rules
- cached availability checks and cooldown-aware routing wrappers

## Poor Fits for Repeated Tool Calls

- asking a cloud model which model to use for a known `embed` stage on every request
- re-running long exploratory reasoning to choose between the same two benchmarked models
- calling tools to rediscover fixed MCP host, port, or stage routing rules that already live in the application

## Example: Static Stage Router

```python
STAGE_ROUTE = {
    "embed": ("local", "nomic-embed-text"),
    "retrieve": ("local", None),
    "draft": ("local", "llama3.1:8b-instruct-q4_K_M"),
    "final_rewrite": ("cloud", "claude-sonnet-4-20250514"),
}

def choose_route(stage: str):
    return STAGE_ROUTE[stage]
```

Use this when benchmarks have already shown the right split and the workflow is stable.

## Example: Cheap Complexity Gate in Code

```python
def classify_prompt(prompt: str) -> str:
    lower = prompt.lower()
    if "```" in prompt or "stack trace" in lower:
        return "code"
    if any(token in lower for token in ("proof", "theorem", "formal", "derive")):
        return "reasoning"
    if len(prompt) < 1200:
        return "simple"
    return "long_context"
```

This kind of heuristic can be good enough to avoid a cloud classifier call on every request.

## Example: Persistent Benchmark Harness

```yaml
stages:
  draft:
    candidates:
      - llama3.1:8b-instruct-q4_K_M
      - qwen2.5:7b-instruct
    score:
      latency_weight: 0.2
      quality_weight: 0.8
  final_rewrite:
    candidates:
      - claude-sonnet-4-20250514
    score:
      fidelity_weight: 0.7
      factuality_weight: 0.3
```

Store this in the repo so the next evaluation loads a known harness instead of starting from scratch.

## Example: Cache Tool Results Before Escalating

```python
from time import time

_status_cache = {"value": None, "expires_at": 0.0}

def get_cached_status(fetch_status):
    now = time()
    if _status_cache["value"] is not None and now < _status_cache["expires_at"]:
        return _status_cache["value"]
    value = fetch_status()
    _status_cache["value"] = value
    _status_cache["expires_at"] = now + 5
    return value
```

This is useful when many requests arrive in a short burst and the system state does not need to be re-fetched for every single one.

## Decision Rule

Prefer persistent code when all of the following are true:

1. The logic is deterministic or low-volatility.
2. Re-discovering it would cost extra tool calls or cloud tokens.
3. The rule can be tested in code.
4. The rule only needs to change when benchmarks, hardware, or policy changes.

Prefer a fresh evaluation when:

1. the workload changed materially
2. the model inventory changed
3. latency, privacy, or quality targets changed
4. benchmark evidence is stale

## What to Keep in Code Versus in the Skill

Keep in code or config:

- stable stage routing
- benchmark fixtures
- validation rules
- routing fallbacks

Keep in the skill:

- how to discover workload requirements
- how to compare local, cloud, and hybrid options
- how to re-evaluate when conditions change
- how to decide whether a multi-model design is justified
