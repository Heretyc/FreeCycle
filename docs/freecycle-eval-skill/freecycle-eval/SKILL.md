---
name: freecycle-eval
description: |
  Evaluates whether to use the local FreeCycle Inference API,
  cloud LLMs (Claude/OpenAI), or a hybrid approach for agentic workflows.
  Guides through discovery, scoring, benchmarking, and routing recommendations.
type: tool
frequency: on-demand
---

# FreeCycle Evaluation Skill

## Behavior Instructions

When this skill is invoked, follow the workflow below. This skill automatically runs all phases in a single response unless it must first gather missing context.

### Automatic Full Evaluation

1. **Analyze context first.** Before asking any questions, examine the user's message and invocation context for signals answering Q1-Q5:
   - Q1 (Integration Type): local-only, cloud-only, or hybrid signals
   - Q2 (Latency): real-time, interactive, or batch/async signals
   - Q3 (Privacy): all-local, mixed, or unrestricted signals
   - Q4 (Quality): maximum quality, balanced, or speed-over-quality signals
   - Q5 (Wake-on-LAN Timeout): presence of WoL config in the MCP client or statements about machine sleep/network setup

2. **If context is complete**, infer all five answers with high confidence and proceed directly to Phase 2 (skip the question block entirely).

3. **If context is incomplete**, present all five required questions in a single numbered block (do not ask them one at a time). Clearly label this as the single required input phase. After the user replies, immediately proceed through all remaining phases without further interruption.

4. **Never pause between phases** to ask for permission or confirmation. Phases 2 through 8 run without interruption. This produces a complete evaluation in one response.

Load these reference files only when needed:
- [benchmarking.md](references/benchmarking.md): detailed model-fit gates, candidate-model exploration, and workload-specific benchmark plans
- [persistent-code.md](references/persistent-code.md): guidance for replacing repeated tool calls and cloud reasoning with static code, config, and benchmark harnesses
- [integration-templates.md](references/integration-templates.md): Python, YAML, and MCP configuration examples
- [verification-checklist.md](references/verification-checklist.md): the full 8-personality self-check

---

## Phase 1: Discovery (Mandatory)

### Workload Detection (Do Not Ask, Infer First)

Before presenting questions, analyze the integration context provided by the user to determine the primary workload. Look for these signals:

- "embeddings", "vectors", "similarity", "search" -> **Embeddings**
- "RAG", "retrieval", "indexed", "knowledge base" -> **RAG**
- "chat", "assistant", "conversation", "Q&A" -> **Inference**
- "classify", "label", "tag", "categorize" -> **Classification**
- "summarize", "extract", "condense", "digest" -> **Summarization**
- "code", "generate code", "complete", "refactor" -> **Code Generation**
- "voice", "tone", "persona", "brand voice", "style guide", "rewrite in this style" -> **Style Constrained Rewriting**
- Multiple workloads mentioned: identify the highest-volume and most latency-sensitive one as primary
- No signals detected: note this and ask a targeted follow-up **after** the five required questions

Record the inferred primary workload. Use it in Phases 2, 3, and 4 wherever a workload answer would be referenced.

If the workflow has multiple stages, such as embed -> retrieve -> draft -> rewrite -> verify, decompose it before scoring. Do not assume one model must handle the entire workflow. Evaluate the best fit for each stage separately, then decide whether the operational complexity of multiple models is worth it.

### Required Questions

If the context from the user's message does not confidently answer all five questions below, present all five at once in a numbered list. After the user replies with their answers, proceed immediately to Phase 2 without pausing for confirmation.

**Q1. Integration Type**
What type of integration are you building?
- (a) Local/networked only (all compute stays on your LAN)
- (b) Cloud only (all compute goes to Claude, OpenAI, or similar)
- (c) Hybrid (some tasks local, some tasks cloud)

**Q2. Latency Requirements**
What are your latency requirements?
- (a) Real time: under 1 second response
- (b) Interactive: under 10 seconds response
- (c) Batch/async: over 10 seconds is acceptable

**Q3. Privacy and Data Sensitivity**
What are the privacy/data sensitivity requirements?
- (a) All data must stay local (no cloud egress)
- (b) Some data can go to cloud, some must stay local
- (c) No restrictions on data location

**Q4. Quality Tradeoff**
What is the acceptable quality tradeoff?
- (a) Maximum quality (best available model, cost is secondary)
- (b) Good enough for the task (balanced quality and cost)
- (c) Speed over quality (fastest response wins)

**Q5. Wake-on-LAN Timeout**
FreeCycle always attempts to wake a sleeping remote GPU server before routing to cloud. If the FreeCycle server is on another machine, how long should it wait for it to wake up?
- (a) Short: up to 5 minutes
- (b) Standard: up to 15 minutes (default)
- (c) Long: up to 30 minutes
- (d) Not applicable. FreeCycle runs on the same machine as my agentic client

### Follow Up Questions

If the user's answers reveal ambiguity or edge cases, ask additional clarifying questions. Examples:

- If the inferred workload is "RAG": What is the corpus size? How frequently does the corpus change? Do you need real-time indexing?
- If the inferred workload is "Style Constrained Rewriting": How exact must the voice match be? Is approximate tone acceptable? Do you have 5 to 10 gold examples?
- If Q2 is "Real time": Is this for a user-facing application or an internal pipeline?
- If Q3 is "Some data can go to cloud": Can you describe which data categories are sensitive vs. non-sensitive?
- If Q1 is "Hybrid": Do you have a preference for which tasks go where, or do you want a recommendation?
- If the user mentions multiple workloads: Which workload is highest priority? Which has the most volume?
- If Q5 is (a), (b), or (c): What are the MAC address and broadcast address of the FreeCycle machine for wake-on-LAN?

### NOTE: Time Budget Question (Placeholder for Priority 14)

A future update will add a time budget question (Q6: "How much time do you have? 15 min minimum, 30+ min ideal"). Benchmark depth will adjust based on this budget. Insert that question here when implementing Priority 14.

---

## Phase 2: Evaluation Framework

Using the five answers from Phase 1 (either inferred or provided by the user), score each deployment option on the following dimensions. Present this as a table.

### Required Tool Assisted Reality Check

Before scoring local vs. cloud, gather live evidence in this order whenever local or hybrid execution is in scope:

1. Decompose the workflow into stages if it is not a single-step task. Typical stages are embed, retrieve, classify, reason, draft, rewrite, verify, and format.
2. Call `freecycle_status` to confirm current availability, blocking state, and whether remote installs are unlocked.
3. Call `freecycle_list_models` to inspect the models already installed on the FreeCycle server. Do not ask the user to guess this if the tool is available.
4. If one or more installed models might fit, optionally call `freecycle_show_model` on the top 1 to 3 candidates to inspect their metadata before benchmarking.
5. Call `freecycle_evaluate_task` only as a coarse routing hint. It combines availability with keyword classification, but it does not understand every workload nuance and it does not validate whether the installed model inventory is a strong fit.
6. If local or hybrid still looks plausible, compare each workflow stage against the installed model inventory before making a confident recommendation.

### Using `freecycle_evaluate_task` Safely

The `requirements` object is strict. Normalize natural language answers to the exact enum values below before calling the tool:

| User signal | Tool field | Allowed value |
|---|---|---|
| Real time or strict low latency | `latency` | `low` |
| Maximum quality | `quality` | `high` |
| Minimize cost or very high volume | `cost` | `minimize` |
| All data must stay local | `privacy` | `critical` |
| Anything else | any field | omit it or use `normal` |

Do not pass free-form strings such as `batch/async`, `balanced quality/cost`, or `no privacy restrictions`. Those will fail validation. If the mapping is unclear, omit `requirements` and rely on the rest of the workflow.

### Static Persistent Code First

When routing logic or model selection becomes stable, prefer persistent code, config, or benchmark fixtures over repeated tool calls or cloud reasoning. The goal is to minimize tool calls and reduce cloud token usage for deterministic work.

Examples:
- Good: keep a static `stage -> deployment -> model` map in application code and only revisit it when benchmarks or requirements change.
- Good: store gold prompts, expected outputs, and scoring rules in a benchmark harness script so reruns are mostly local and repeatable.
- Avoid: asking a cloud model on every request whether embeddings should use `nomic-embed-text`, or whether a known rewrite stage should route local or cloud.

Load [persistent-code.md](references/persistent-code.md) when you need fuller examples.

### Scoring Dimensions (1 to 5 scale)

| Dimension | Local (FreeCycle Inference API) | Cloud (Claude/OpenAI) | Hybrid |
|---|---|---|---|
| **Latency** | Score based on task. High (4 to 5) for simple tasks on local GPU. Lower (2 to 3) for complex reasoning. | Moderate (3). Network round trip adds 200ms to 2s. | Varies by routing. Best of both when configured well. |
| **Cost** | Free after hardware. No per token fees. Score: 5. | Per token pricing. Score depends on volume. Low volume: 4. High volume: 1 to 2. | Mixed. Local handles volume, cloud handles complexity. Score: 3 to 4. |
| **Privacy** | Perfect. All data stays on the machine. Score: 5. | Data leaves your network. Score: 1 to 2 depending on provider policies. | Depends on routing rules. Score: 3 to 5 if sensitive data stays local. |
| **Quality** | Moderate for 8B parameter models. Good for embeddings, summarization, classification. Weaker for advanced reasoning and math. Score: 2 to 3. | Highest quality available. Score: 5. | Best of both worlds when routing is correct. Score: 4 to 5. |
| **Availability** | Depends on GPU and FreeCycle status. If a game is running, local inference is stopped. Score: 2 to 3. | Always on (99.9%+ uptime from major providers). Score: 5. | Failover capable. Score: 4 to 5. |
| **Throughput** | Limited by single GPU. Good for sequential tasks. Score: 3. | Virtually unlimited with API rate limits. Score: 4 to 5. | Combined capacity. Score: 4 to 5. |

### Scoring Notes

Adjust scores based on the user's specific answers and detected workload:
- If the user has a powerful GPU (RTX 4090, etc.), increase local latency and throughput scores.
- If the user runs games frequently, decrease local availability score.
- If the detected workload is primarily embeddings, increase local quality score (nomic-embed-text is excellent for this).
- If the detected workload is advanced reasoning or code generation, decrease local quality score and increase cloud quality score.
- If the detected workload is style constrained rewriting, named voice matching, or brittle persona transfer, decrease local quality unless a benchmarked local model has already proven fidelity on gold examples.
- If Q5 answer is (a), (b), or (c) (wake-on-LAN is applicable), increase local availability by 1 point when the user's latency requirement can tolerate the wake delay.

Lower confidence by one level when any of the following is true:
- `freecycle_evaluate_task` suggests local or hybrid, but the installed models are only generic small instruct models for a code, reasoning, or style-fidelity task
- No benchmark has been run for the candidate local model
- The user needs exact voice, tone, or formatting fidelity
- The local model inventory has no clear fit and remote model installs are currently locked

### How to Present the Evaluation

1. Fill in the table with numeric scores based on the user's answers.
2. Calculate a weighted total for each option, weighting dimensions by the user's stated priorities (e.g., if privacy is critical, weight privacy 3x).
3. Provide a clear recommendation with rationale.

---

## Phase 3: Benchmarking Methodology

Provide the user with a concrete benchmarking plan tailored to their detected workload. The methodology below should be adapted based on the inferred primary workload.

### Prerequisites: FreeCycle Installation and MCP Configuration

**Where FreeCycle must be installed:** FreeCycle runs on the machine that has the GPU, the server or desktop where the local inference service will run. It does not need to be installed on every machine. Agentic clients such as laptops interact with FreeCycle exclusively through the MCP server. The MCP server can be obtained by:
- Building it with the provided prompt in the [FreeCycle GitHub repo](https://github.com/Heretyc/FreeCycle), or
- Using the pre-made MCP server from the same repo

**Verify MCP server configuration before benchmarking.** The MCP server must be pointed at the correct IP address where FreeCycle is actually running. To verify, call `freecycle_status`. If FreeCycle is not running locally on the client machine, the configured `FREECYCLE_HOST` must match the LAN IP of the GPU machine.

If `freecycle_status` returns an error or the host is unreachable, check the MCP configuration:
- Via config file: update `freecycle.host` in `freecycle-mcp.config.json`
- Via environment variable: set `FREECYCLE_HOST=<GPU machine IP>`

If the MCP server cannot reach FreeCycle, benchmarking cannot proceed until the connection is established. In that case, provide the user with the configuration steps above before continuing.

When the user needs a different local model, use the `freecycle_model_catalog` tool to discover available models. If you recommend a model that is not already present, remind the user that `freecycle_pull_model` and direct `POST /models/install` requests only work while the FreeCycle tray menu has "Remote Model Installs" unlocked on the GPU machine.

### General Benchmarking Steps

1. **Prepare a test dataset.** Create 20 to 50 representative prompts for the user's workload. Include easy, medium, and hard examples.

2. **Check FreeCycle status.** Before benchmarking, call `freecycle_status` to confirm FreeCycle is running and the local inference engine is available on the GPU machine:

```
freecycle_status
```

Expected response when available:
```json
{
  "status": "Available",
  "ollama_running": true,
  "vram_used_mb": 512,
  "vram_total_mb": 8192,
  "vram_percent": 6,
  "local_ip": "192.168.1.10",
  "ollama_port": 11434,
  "remote_model_installs_unlocked": false,
  "remote_model_installs_expires_in_seconds": null
}
```

If the GPU machine is sleeping and wake-on-LAN is configured, calling `freecycle_check_availability` will automatically send the wake packet and poll until the machine is ready or the configured timeout expires. No separate wake step is needed.

### NOTE: Auto-Install Logic (Placeholder for Priority 14)

When `remote_model_installs_unlocked=true` in the status response, a future update will automatically pull recommended candidate models without user intervention. For now, manually remind users to enable the one-hour unlock on the FreeCycle tray if they want to pull new models. Insert auto-pull logic here when implementing Priority 14.

3. **Signal task start when running inference tools manually.** If you use `freecycle_benchmark` directly, task signaling is handled automatically. For manual runs with `freecycle_generate` or `freecycle_embed`, signal task start first:

```
freecycle_start_task(task_id="benchmark-001", description="Running eval benchmark")
```

4. **Run local benchmarks:**

For inference:
```
freecycle_generate(model="llama3.1:8b-instruct-q4_K_M", prompt="YOUR_PROMPT_HERE")
```

For embeddings:
```
freecycle_embed(model="nomic-embed-text", input="YOUR_TEXT_HERE")
```

For automated latency and tokens/sec measurement (recommended):
```
freecycle_benchmark(model="llama3.1:8b-instruct-q4_K_M", prompt="YOUR_PROMPT_HERE", iterations=5)
```

5. **Run cloud benchmarks** (if applicable). Use the same prompts against Claude or OpenAI APIs. Record latency and response quality.

6. **Signal task stop** (only when you manually called `freecycle_start_task` in step 3):

```
freecycle_stop_task(task_id="benchmark-001")
```

7. **Evaluate results.** Compare:
   - **Latency:** Average, P50, P95, P99 response times
   - **Quality:** Rate each response on a 1 to 5 scale for correctness and completeness
   - **Cost:** Calculate per token cost for cloud runs. Local cost is $0.

### NOTE: Benchmark Results Persistence Constraint (Placeholder for Priority 14)

A future update will add an explicit instruction: benchmark results must remain in conversation context only and never be persisted to disk. This protects sensitive workload data. Insert that instruction here when implementing Priority 14.

Before step 3, review installed models with `freecycle_list_models` and do a quick per-stage fit check. If no installed model is a clear fit for a critical stage, lower confidence immediately and either run a candidate-model exploration loop or keep that stage in cloud.

For detailed model-fit gates, candidate-model exploration, workload-specific benchmarks, and local-model recommendation patterns, load [benchmarking.md](references/benchmarking.md).

---

## Phase 4: Routing Recommendations

Based on the evaluation, provide specific routing recommendations. Present as a decision table.

### Task Routing Matrix

| Task Type | Recommended Deployment | Reasoning |
|---|---|---|
| Embeddings | **Local** | nomic-embed-text is fast, free, and high quality. No reason to use cloud. |
| Simple classification | **Local** | 8B models handle binary/multi class classification well. |
| Short summarization (under 2000 tokens) | **Local** | Adequate quality, zero cost, low latency. |
| Long summarization (over 2000 tokens) | **Cloud or Hybrid** | Larger context windows and better coherence from cloud models. |
| Code generation | **Cloud** | Quality gap is significant. 8B models lack the reasoning depth for complex code. |
| Advanced reasoning and math | **Cloud** | Cloud models (Claude, GPT 4) significantly outperform 8B local models. |
| Style constrained rewriting or voice matching | **Cloud or Hybrid** | Generic local instruct models often drift on persona, tone, and brittle formatting constraints. Use local only after benchmark evidence. |
| Privacy sensitive data with explicit local-only requirement | **Local** | Applies when the user answered Q3=a or otherwise explicitly disallowed cloud egress. |
| High throughput batch (over 1000 requests) | **Local** | Avoids per token API costs. Throughput is sufficient for batch workloads. |
| Real time user facing | **Cloud with local fallback** | Cloud provides consistent latency. Local serves as fallback when cloud is slow or down. |
| Chat/conversational | **Hybrid** | Simple questions local, complex questions cloud. Route based on prompt complexity. |

### Dynamic Routing Logic

For hybrid deployments, recommend this routing strategy:

1. **Check FreeCycle availability first.** Call `freecycle_check_availability`. If local inference is not running (game detected, cooldown period), route everything to cloud.
2. **If the GPU machine is unreachable, wake it automatically.** The MCP layer always attempts wake-on-LAN when the machine is not responding, waiting up to the configured timeout before routing to cloud. No manual fallback decision is needed. The MCP handles it.
3. **Check model fit stage by stage.** If the installed local models are a weak match for any critical stage, either run the candidate-model exploration loop or send that stage's high-stakes path to cloud.
4. **Classify prompt complexity.** Use a lightweight local classifier or heuristic (prompt length, presence of code blocks, mathematical notation).
5. **Route simple prompts locally.** If the prompt is under 500 tokens, does not require advanced reasoning, and the installed local model is a clear fit for that stage, use local.
6. **Route complex prompts to cloud.** If the prompt requires multi-step reasoning, code generation, exact style matching, or long context, use cloud for that stage.
7. **Apply the user's explicit privacy answer.** If Q3=a, keep that traffic local. If Q3=b, route only the allowed subset to cloud.

If privacy or cost constraints force a task to stay local but the default models are weak for that workload, use the `freecycle_model_catalog` tool to recommend a better-fit local model, then remind the user that the FreeCycle tray unlock must be enabled before `freecycle_pull_model` can install it remotely.
If `freecycle_evaluate_task` says `local` but the installed model inventory or benchmark evidence disagrees, trust the inventory and benchmark evidence.
For multi-stage workflows, it is valid to recommend multiple local models plus a cloud-only stage if that is the best-fit design.

---

## Phase 5: Integration Patterns

Prefer persistent code or config for repeatable routing logic instead of re-deciding the same rules through tools on every request.

Small example:

```python
STAGE_ROUTE = {
    "embed": ("local", "nomic-embed-text"),
    "draft": ("local", "llama3.1:8b-instruct-q4_K_M"),
    "final_rewrite": ("cloud", "claude-sonnet-4-20250514"),
}
```

Use this pattern when benchmark results are already known and the stage boundaries are stable. Re-run the evaluation workflow when requirements, hardware, or models change, not on every request.

For full Python, YAML, and MCP examples, load [integration-templates.md](references/integration-templates.md). For more persistent-code examples, load [persistent-code.md](references/persistent-code.md).

---

## Phase 6: Negative Constraints and Edge Cases

### What NOT to Do

1. **Never send data to cloud when the user explicitly requires local-only handling.** If the user answered Q3=a, all processing must stay local regardless of quality tradeoff.
2. **Never assume FreeCycle Inference API is available.** Always call `freecycle_status` before routing to local. A game may have started since your last check.
3. **Never ignore the cooldown period.** When FreeCycle reports "Cooldown" status, local inference is stopped. Do not attempt to connect during cooldown (default: 1800 seconds after a game exits).
4. **Never ignore the wake delay.** When FreeCycle reports "Wake Delay" status after resume, local inference is stopped for the configured hold period (default: 60 seconds) unless the user manually forces it on.
5. **Never run benchmarks during gaming sessions.** Check status first. If blocked, wait or use cloud only.
6. **Never wait forever for a sleeping server.** Wake-on-LAN is always attempted, but uses a bounded max wait. After the timeout, fall back to cloud.
7. **Never rely on `freecycle_evaluate_task` alone.** It is a coarse helper based on availability, a small requirements enum, and keyword classification. Validate against installed models and benchmark evidence.
8. **Never force one model across a multi-stage workflow without checking stage fit.** It is valid to recommend different models for embeddings, drafting, rewriting, verification, or other stages.
9. **Never hardcode model names without fallback.** Models may change. Always have a fallback model or routing path.
10. **Never assume remote installs are unlocked.** `freecycle_pull_model` and direct `POST /models/install` calls only work while the FreeCycle tray menu on the GPU machine has "Remote Model Installs" enabled. That unlock auto-expires after one hour.
11. **Never skip task signaling.** The shipped MCP tools already wrap their work with `freecycle_start_task` and `freecycle_stop_task`. For direct HTTP or custom local workflows, add the same start and stop calls yourself and guarantee cleanup in `finally`.
12. **Never assume FreeCycle is installed on agentic clients.** FreeCycle must be installed on the GPU machine. Clients (laptops, workstations without a GPU) connect through the MCP server only.

### Edge Cases

| Scenario | How to Handle |
|---|---|
| FreeCycle is unreachable (service not running) | Fall back to cloud. Log a warning. Retry FreeCycle status on next request. |
| FreeCycle machine is asleep | The MCP layer always sends the configured wake-on-LAN burst and polls until FreeCycle is ready or the configured max wait expires. |
| FreeCycle reports "Wake Delay" status | Wait for the short post-resume hold to expire, or route temporarily to cloud if latency matters. |
| Inference engine is running but model is not loaded | First request will be slow (model load time). Set a longer timeout (60s+) for the first request. |
| Game starts mid inference | FreeCycle will stop local inference. Your request will fail. Catch the error and retry via cloud. |
| VRAM is nearly full | Check `vram_percent` in status response. If over 80%, consider routing to cloud to avoid OOM. |
| Multiple agents competing for GPU | Use the task signal API. Only one task should be active at a time. Queue additional tasks or route to cloud. |
| FreeCycle reports "Error" status | NVML or GPU driver issue. All traffic must go to cloud until resolved. |
| Model download in progress | Status will show "Downloading Models". Local inference is still running but may be slow. Route latency-sensitive tasks to cloud. |
| `freecycle_evaluate_task` says local, but the installed models are a poor fit | Lower confidence immediately. Inspect installed models, run the candidate-model exploration loop, or keep the hard path in cloud. |
| Different stages want different models | Recommend a staged plan instead of forcing one model. For example, local embeddings, local draft generation, and cloud final rewrite or verification. |
| A recommended local model is missing | Use the `freecycle_model_catalog` tool for the exact model name, then remind the user that the GPU machine owner must enable the tray's one-hour remote install unlock before `freecycle_pull_model` or `POST /models/install` will succeed. |
| MCP server pointing at wrong host | Call `freecycle_status` to verify connectivity. If unreachable, update `freecycle.host` in `freecycle-mcp.config.json` or set the `FREECYCLE_HOST` environment variable to the correct LAN IP of the GPU machine. |

---

## Phase 7: Final Recommendation Format

Present your final recommendation in this structure:

```
## Recommendation Summary

**Recommended approach:** [Local / Cloud / Hybrid]
**Confidence:** [High / Medium / Low]

### Why this approach
[2 to 3 sentences explaining the recommendation based on the user's specific answers]

### Stage routing plan
| Stage | Deploy to | Model | Reason |
|---|---|---|---|
| [stage 1] | [local/cloud] | [model name] | [brief reason] |
| [stage 2] | [local/cloud] | [model name] | [brief reason] |

### Model strategy
[State whether a single-model path is sufficient or whether a multi-model path is justified. Mention the operational tradeoff.]

### Estimated costs
- Local: $0 per month (hardware already owned)
- Cloud: $X per month at estimated volume of Y requests
- Hybrid: $X per month (cloud portion only)

### Next steps
1. [First action item]
2. [Second action item]
3. [Third action item]
```

---

## Phase 8: Self Verification (8 Personality Checks)

Run the full checklist in [verification-checklist.md](references/verification-checklist.md) before considering this skill complete.

That checklist covers:
- clarity
- role and context
- structure
- examples
- negative constraints
- reasoning transparency
- output format
- adversarial edge cases
