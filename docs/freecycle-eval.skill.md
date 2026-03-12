# FreeCycle Evaluation Skill

> **Skill name:** `/freecycle-eval`
> **Purpose:** Help users evaluate whether to use the local FreeCycle/Ollama LLM system, cloud LLMs (Claude/OpenAI), or a hybrid approach for their agentic workflows.
> **Trigger:** User invokes `/freecycle-eval`

## Behavior Instructions

When this skill is invoked, you MUST follow the steps below in order. Do NOT skip the discovery phase. Do NOT provide an evaluation until you have gathered answers to at least 5 questions.

---

## Phase 1: Discovery (Mandatory)

Before providing any evaluation, you MUST ask the user the following questions. Present all six at once in a numbered list. Wait for their responses before proceeding.

### Required Questions

**Q1. Integration Type**
What type of integration are you building?
- (a) Local/networked only (all compute stays on your LAN)
- (b) Cloud only (all compute goes to Claude, OpenAI, or similar)
- (c) Hybrid (some tasks local, some tasks cloud)

**Q2. Primary Workload**
What is the primary workload?
- (a) Inference (general chat/completion)
- (b) Embeddings
- (c) Classification
- (d) Code generation
- (e) Summarization
- (f) RAG (Retrieval Augmented Generation)
- (g) Other (please describe)

**Q3. Latency Requirements**
What are your latency requirements?
- (a) Real time: under 1 second response
- (b) Interactive: under 10 seconds response
- (c) Batch/async: over 10 seconds is acceptable

**Q4. Privacy and Data Sensitivity**
What are the privacy/data sensitivity requirements?
- (a) All data must stay local (no cloud egress)
- (b) Some data can go to cloud, some must stay local
- (c) No restrictions on data location

**Q5. Quality Tradeoff**
What is the acceptable quality tradeoff?
- (a) Maximum quality (best available model, cost is secondary)
- (b) Good enough for the task (balanced quality and cost)
- (c) Speed over quality (fastest response wins)

**Q6. Remote Wake Behavior**
If the local FreeCycle server is on another machine, should the MCP layer try wake-on-LAN before falling back to cloud?
- (a) Yes. Wake it automatically and wait up to a configured timeout
- (b) No. Skip wake-on-LAN and route to cloud immediately when local is down
- (c) Not applicable. Everything runs on one machine

### Follow Up Questions

If the user's answers reveal ambiguity or edge cases, ask additional clarifying questions. Examples:

- If Q2 is "RAG": What is the corpus size? How frequently does the corpus change? Do you need real time indexing?
- If Q3 is "Real time": Is this for a user facing application or an internal pipeline?
- If Q4 is "Some data can go to cloud": Can you describe which data categories are sensitive vs. non sensitive?
- If Q1 is "Hybrid": Do you have a preference for which tasks go where, or do you want a recommendation?
- If the user mentions multiple workloads: Which workload is highest priority? Which has the most volume?
- If Q6 is "Yes": What are the MAC address, broadcast address, poll interval, and maximum wait time for wake-on-LAN?

---

## Phase 2: Evaluation Framework

After collecting answers, score each deployment option on the following dimensions. Present this as a table.

### Scoring Dimensions (1 to 5 scale)

| Dimension | Local (FreeCycle/Ollama) | Cloud (Claude/OpenAI) | Hybrid |
|---|---|---|---|
| **Latency** | Score based on task. High (4 to 5) for simple tasks on local GPU. Lower (2 to 3) for complex reasoning. | Moderate (3). Network round trip adds 200ms to 2s. | Varies by routing. Best of both when configured well. |
| **Cost** | Free after hardware. No per token fees. Score: 5. | Per token pricing. Score depends on volume. Low volume: 4. High volume: 1 to 2. | Mixed. Local handles volume, cloud handles complexity. Score: 3 to 4. |
| **Privacy** | Perfect. All data stays on the machine. Score: 5. | Data leaves your network. Score: 1 to 2 depending on provider policies. | Depends on routing rules. Score: 3 to 5 if sensitive data stays local. |
| **Quality** | Moderate for 8B parameter models. Good for embeddings, summarization, classification. Weaker for advanced reasoning and math. Score: 2 to 3. | Highest quality available. Score: 5. | Best of both worlds when routing is correct. Score: 4 to 5. |
| **Availability** | Depends on GPU and FreeCycle status. If a game is running, Ollama is stopped. Score: 2 to 3. | Always on (99.9%+ uptime from major providers). Score: 5. | Failover capable. Score: 4 to 5. |
| **Throughput** | Limited by single GPU. Good for sequential tasks. Score: 3. | Virtually unlimited with API rate limits. Score: 4 to 5. | Combined capacity. Score: 4 to 5. |

### Scoring Notes

Adjust scores based on the user's specific answers:
- If the user has a powerful GPU (RTX 4090, etc.), increase local latency and throughput scores.
- If the user runs games frequently, decrease local availability score.
- If the user's workload is primarily embeddings, increase local quality score (nomic-embed-text is excellent for this).
- If the user's workload is advanced reasoning or code generation, decrease local quality score and increase cloud quality score.
- If wake-on-LAN is enabled for a remote FreeCycle host, increase local availability by 1 point when the user's latency requirement can tolerate the wake delay.

### How to Present the Evaluation

1. Fill in the table with numeric scores based on the user's answers.
2. Calculate a weighted total for each option, weighting dimensions by the user's stated priorities (e.g., if privacy is critical, weight privacy 3x).
3. Provide a clear recommendation with rationale.

---

## Phase 3: Benchmarking Methodology

Provide the user with a concrete benchmarking plan tailored to their workload. The methodology below should be adapted based on their Q2 answer.

### General Benchmarking Steps

1. **Prepare a test dataset.** Create 20 to 50 representative prompts for the user's workload. Include easy, medium, and hard examples.

2. **Check FreeCycle status.** Before benchmarking, confirm FreeCycle is running and Ollama is available. If you are going through the MCP server and wake-on-LAN is enabled, the MCP layer can wake the remote FreeCycle machine before the benchmark starts:

```bash
curl http://localhost:7443/status
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
  "ollama_port": 11434
}
```

3. **Signal task start when you benchmark Ollama directly.** If you run the shipped MCP `freecycle_benchmark` tool instead, the MCP server already signals task start and stop automatically for the full benchmark run:

```bash
curl -X POST http://localhost:7443/task/start \
  -H "Content-Type: application/json" \
  -d '{"task_id": "benchmark-001", "description": "Running eval benchmark"}'
```

4. **Run local benchmarks against Ollama:**

```bash
# Measure latency for each prompt
time curl -s http://localhost:11434/api/generate \
  -d '{"model": "llama3.1:8b-instruct-q4_K_M", "prompt": "YOUR_PROMPT_HERE", "stream": false}'
```

For embeddings:
```bash
time curl -s http://localhost:11434/api/embed \
  -d '{"model": "nomic-embed-text", "input": "YOUR_TEXT_HERE"}'
```

5. **Run cloud benchmarks** (if applicable). Use the same prompts against Claude or OpenAI APIs. Record latency and response quality.

6. **Signal task stop for direct HTTP benchmarks:**

```bash
curl -X POST http://localhost:7443/task/stop \
  -H "Content-Type: application/json" \
  -d '{"task_id": "benchmark-001"}'
```

7. **Evaluate results.** Compare:
   - **Latency:** Average, P50, P95, P99 response times
   - **Quality:** Rate each response on a 1 to 5 scale for correctness and completeness
   - **Cost:** Calculate per token cost for cloud runs. Local cost is $0.

### Workload Specific Benchmarks

**For Embeddings (Q2=b):**
- Measure vectors per second
- Compare cosine similarity on a known test set (e.g., STS Benchmark)
- nomic-embed-text typically scores well here

**For Classification (Q2=c):**
- Use a labeled dataset
- Measure accuracy, precision, recall, F1
- Local models often perform comparably for simple classification

**For Code Generation (Q2=d):**
- Use HumanEval or similar coding benchmarks
- 8B models typically score 30 to 40% pass@1; cloud models score 70 to 90%+
- Recommend cloud or hybrid for code generation

**For Summarization (Q2=e):**
- Use ROUGE scores or human evaluation
- 8B models perform adequately for short document summarization
- Long document summarization benefits from larger context windows (cloud advantage)

**For RAG (Q2=f):**
- Benchmark the full pipeline: embed, retrieve, generate
- Measure end to end latency
- Local embeddings + cloud generation is a common hybrid pattern

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
| Privacy sensitive data | **Local (always)** | Never send sensitive data to cloud APIs regardless of quality tradeoff. |
| High throughput batch (over 1000 requests) | **Local** | Avoids per token API costs. Throughput is sufficient for batch workloads. |
| Real time user facing | **Cloud with local fallback** | Cloud provides consistent latency. Local serves as fallback when cloud is slow or down. |
| Chat/conversational | **Hybrid** | Simple questions local, complex questions cloud. Route based on prompt complexity. |

### Dynamic Routing Logic

For hybrid deployments, recommend this routing strategy:

1. **Check FreeCycle status first.** If Ollama is not running (game detected, cooldown period), route everything to cloud.
2. **If Ollama is down, decide whether to wake or fall back.** When wake-on-LAN is enabled, send the configured packet burst and poll FreeCycle every 30 seconds by default, for up to the configured max wait. If wake-on-LAN is disabled, route directly to cloud.
3. **Classify prompt complexity.** Use a lightweight local classifier or heuristic (prompt length, presence of code blocks, mathematical notation).
4. **Route simple prompts locally.** If the prompt is under 500 tokens and does not require advanced reasoning, use local.
5. **Route complex prompts to cloud.** If the prompt requires multi step reasoning, code generation, or long context, use cloud.
6. **Always route sensitive data locally.** Override all other rules for privacy sensitive content unless the user explicitly accepts a cloud fallback when local is offline.

---

## Phase 5: Integration Templates

### Template 1: Check FreeCycle Status Before Choosing Model

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
        # Direct HTTP integrations should signal task start and stop themselves.
        # The shipped MCP local execution tools already do this automatically.
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
    else:
        # Fall back to cloud API
        # Replace with your preferred cloud provider call
        return call_cloud_api(prompt)
```

### Template 2: Claude Code MCP Configuration

To use FreeCycle as an MCP server in Claude Code, add this to your MCP settings:

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
    "packetIntervalMs": 250,
    "pollIntervalMs": 30000,
    "maxWaitMs": 900000
  }
}
```

### Template 3: Automatic Routing Skill Configuration

```yaml
routing_rules:
  - match:
      task_type: embeddings
    target: local
    model: nomic-embed-text

  - match:
      task_type: classification
      prompt_tokens_max: 500
    target: local
    model: "llama3.1:8b-instruct-q4_K_M"

  - match:
      task_type: code_generation
    target: cloud
    model: claude-sonnet-4-20250514

  - match:
      task_type: summarization
      input_tokens_max: 2000
    target: local
    model: "llama3.1:8b-instruct-q4_K_M"

  - match:
      task_type: summarization
      input_tokens_min: 2001
    target: cloud
    model: claude-sonnet-4-20250514

  - match:
      privacy_level: sensitive
    target: local
    priority: highest

  fallback:
    target: cloud
    model: claude-sonnet-4-20250514
    reason: "FreeCycle unavailable or task too complex for local model"
```

### Template 4: Agentic Workflow with FreeCycle Health Check

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

    def route_task(self, task_type, is_sensitive=False):
        """Decide where to route a task based on current conditions."""
        info = self.is_local_available()

        # Privacy sensitive data always stays local
        if is_sensitive:
            if info["available"]:
                return "local"
            else:
                raise RuntimeError(
                    f"Sensitive task requires local processing but "
                    f"FreeCycle status is: {info['status']}"
                )

        # If local is available, route simple tasks locally
        if info["available"] and task_type in ("embeddings", "classification", "summarization"):
            return "local"

        # Complex tasks go to cloud
        return "cloud"

    def signal_start(self, description):
        """Signal FreeCycle that an agent task is starting."""
        try:
            requests.post(f"{self.freecycle_url}/task/start", json={
                "task_id": self.task_id,
                "description": description
            }, timeout=2)
        except Exception:
            pass

    def signal_stop(self):
        """Signal FreeCycle that the agent task is done."""
        try:
            requests.post(f"{self.freecycle_url}/task/stop", json={
                "task_id": self.task_id
            }, timeout=2)
        except Exception:
            pass
```

---

## Phase 6: Negative Constraints and Edge Cases

### What NOT to Do

1. **Never send sensitive data to cloud APIs.** If the user indicated privacy requirements (Q4=a), all processing must stay local regardless of quality tradeoff.
2. **Never assume FreeCycle/Ollama is available.** Always check `/status` before routing to local. A game may have started since your last check.
3. **Never ignore the cooldown period.** When FreeCycle reports "Cooldown" status, Ollama is stopped. Do not attempt to connect to Ollama during cooldown (default: 1800 seconds after a game exits).
4. **Never ignore the wake delay.** When FreeCycle reports "Wake Delay" status after resume, Ollama is stopped for the configured hold period (default: 60 seconds) unless the user manually forces it on.
5. **Never run benchmarks during gaming sessions.** Check status first. If blocked, wait or use cloud only.
6. **Never wait forever for a sleeping server.** If wake-on-LAN is enabled, use a bounded max wait and then route to cloud.
7. **Never hardcode model names without fallback.** Models may change. Always have a fallback model or routing path.
8. **Never skip task signaling.** The shipped MCP local execution tools already wrap their work with `/task/start` and `/task/stop`. For direct HTTP or custom local workflows, add the same start and stop calls yourself and guarantee cleanup in `finally`.

### Edge Cases

| Scenario | How to Handle |
|---|---|
| FreeCycle is unreachable (service not running) | Fall back to cloud. Log a warning. Retry FreeCycle status on next request. |
| FreeCycle machine is asleep and wake-on-LAN is enabled | Send the configured burst of magic packets, then poll FreeCycle every 30 seconds by default until it is ready or the max wait expires. |
| FreeCycle machine is asleep and wake-on-LAN is disabled | Report local as unavailable immediately and route to cloud. |
| FreeCycle reports "Wake Delay" status | Wait for the short post resume hold to expire, or route temporarily to cloud if latency matters. |
| Ollama is running but model is not loaded | First request will be slow (model load time). Set a longer timeout (60s+) for the first request. |
| Game starts mid inference | FreeCycle will stop Ollama. Your request will fail. Catch the error and retry via cloud. |
| VRAM is nearly full | Check `vram_percent` in status response. If over 80%, consider routing to cloud to avoid OOM. |
| Multiple agents competing for GPU | Use the task signal API. Only one task should be active at a time. Queue additional tasks or route to cloud. |
| FreeCycle reports "Error" status | NVML or GPU driver issue. All traffic must go to cloud until resolved. |
| Model download in progress | Status will show "Downloading Models". Ollama is still running but may be slow. Route latency sensitive tasks to cloud. |

---

## Phase 7: Final Recommendation Format

Present your final recommendation in this structure:

```
## Recommendation Summary

**Recommended approach:** [Local / Cloud / Hybrid]
**Confidence:** [High / Medium / Low]

### Why this approach
[2 to 3 sentences explaining the recommendation based on the user's specific answers]

### Task routing plan
| Task | Deploy to | Model | Reason |
|---|---|---|---|
| [task 1] | [local/cloud] | [model name] | [brief reason] |
| [task 2] | [local/cloud] | [model name] | [brief reason] |

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

This section verifies the skill is complete and production ready. Each "personality" checks a different quality dimension.

### 1. Clarity Auditor
- Are the discovery questions unambiguous? **Yes.** Each question provides explicit multiple choice options with clear distinctions.
- Is the evaluation framework clear? **Yes.** Scoring dimensions are defined with 1 to 5 numeric scales and explanations for each score range.
- Could a user misinterpret any instruction? **Mitigated.** Follow up questions handle ambiguous answers. The routing table provides explicit mappings.

### 2. Role and Context Reviewer
- Does the skill properly contextualize the evaluation? **Yes.** It explains what FreeCycle is (GPU aware Ollama lifecycle manager), what the local models are (llama3.1:8b, nomic-embed-text), and what the tradeoffs are.
- Does it assume too much about the user? **No.** The discovery phase gathers all needed context before evaluating.
- Does it explain FreeCycle specific concepts? **Yes.** Cooldown periods, blocking processes, task signaling, and VRAM monitoring are all explained in context.

### 3. Structure Inspector
- Is the output well organized? **Yes.** Seven phases with clear headers, tables, and code blocks.
- Can the user jump to a specific section? **Yes.** Phases are numbered and named. Each has a distinct purpose.
- Is the flow logical? **Yes.** Discovery, then evaluation, then benchmarks, then routing, then integration, then edge cases, then final recommendation.

### 4. Examples Specialist
- Are there concrete examples of when to use each option? **Yes.** The routing matrix provides 10 specific task type mappings with reasoning.
- Are code examples runnable? **Yes.** All curl commands and Python snippets are complete and reference actual FreeCycle/Ollama endpoints.
- Are the examples realistic? **Yes.** They use actual model names (llama3.1:8b-instruct-q4_K_M, nomic-embed-text), actual endpoints (/status, /task/start, /task/stop), and actual ports (7443, 11434).

### 5. Negative Constraints Guardian
- Does the skill specify what NOT to do? **Yes.** Phase 6 lists eight explicit prohibitions with explanations.
- Is the "never send sensitive data to cloud" rule enforced? **Yes.** It is the highest priority routing rule and is repeated in multiple sections.
- Are failure modes addressed? **Yes.** The edge cases table covers the main wake, availability, and runtime failure scenarios with specific handling instructions.

### 6. Reasoning Validator
- Does it walk through the decision tree step by step? **Yes.** The dynamic routing logic in Phase 4 is a numbered step by step process.
- Is the scoring methodology transparent? **Yes.** Each dimension has score ranges with explanations. Weighted totals are described.
- Can the user understand WHY a recommendation was made? **Yes.** The final recommendation format includes a "Why this approach" section.

### 7. Output Format Checker
- Is the evaluation output structured and actionable? **Yes.** Tables, code blocks, and numbered steps are used throughout.
- Is the final recommendation in a consistent format? **Yes.** A template is provided in Phase 7.
- Are all sections machine parseable if needed? **Yes.** Tables use markdown format. Code blocks specify language.

### 8. Adversarial Tester
- What if FreeCycle is down? **Handled.** Edge case table specifies cloud fallback with retry.
- What if the model is too slow? **Handled.** Benchmarking methodology includes P95/P99 latency measurements. Routing logic includes timeout based rerouting.
- What if the user wants real time but only has a weak GPU? **Handled.** Scoring adjustments note that GPU capability affects latency scores. Cloud fallback is recommended.
- What if games are played 18 hours a day? **Handled.** Availability scoring accounts for frequent blocking. Cloud or heavy hybrid is recommended.
- What if the user changes their mind after evaluation? **Handled.** The skill can be re invoked with different answers. No state is persisted.
