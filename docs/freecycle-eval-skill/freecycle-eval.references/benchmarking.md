# Benchmarking and Model Selection

Use this reference when the main skill determines that local or hybrid execution is plausible and you need more than a coarse recommendation.

## Time Budget Modes

The benchmark depth depends on the time budget from Phase 1, Q6:

| Mode | Time Budget | Approach | Depth |
|---|---|---|---|
| Quick Mode | 15 minutes | Use installed models only, 5 prompts, 3 iterations per model, skip candidate exploration and combo tests | Smoke test or pass-fail gate |
| Full Mode | 30+ minutes | Candidate model exploration, 20-50 prompts, full benchmark sets, 2-model combination tests when time allows | Complete evaluation with exploration |

### Quick Mode (15 minutes)

Follow this streamlined procedure when the time budget is 15 minutes:

1. **Check installed models.** Call `freecycle_list_models` and use `freecycle_model_catalog` for a quick fit-check hint on whether a clear winner exists. If no installed model is clearly a fit, use the catalog for a fit hint, but do not pull candidates or explore further. Do not pull new candidates.
2. **Select the best-fit installed model** for each workflow stage. If no installed model matches the workload well and remote installs are locked, skip to Phase 4 and recommend cloud with Low confidence.
3. **Prepare 5 representative prompts** (instead of 20-50) covering easy, medium, and hard cases for the primary workload.
4. **Run a smoke test** with `freecycle_benchmark(iterations=3)` on the best candidate per stage. Record latency and quality.
5. **Evaluate results quickly.** Determine: Is local performance acceptable given the 15-minute constraint? If yes, recommend local or hybrid. If no, recommend cloud.
6. **Document confidence level.** Note in the recommendation that confidence is Limited due to the smoke test nature. Suggest re-evaluation with 30+ minutes for higher confidence.

Note: Skip 2-model combination tests in Quick Mode. Defer them to Full Mode if the user later invests more time.

---

## Single-Model Benchmark Methodology

Use this procedure to systematically benchmark a single local model using the FreeCycle MCP tools. This methodology ensures consistent, repeatable evaluation without ad-hoc scripts or persistent result storage.

### Overview

The single-model benchmark has three phases:
1. **Pre-flight checks** -- Verify FreeCycle availability, model presence, and resource constraints.
2. **Latency and throughput measurement** -- Use `freecycle_benchmark` to quantify performance across representative prompts.
3. **Quality sampling** -- Use `freecycle_generate` to inspect actual output on challenging prompts and score inline.

The entire evaluation happens within conversation context. Do not write results to disk or create external benchmark harnesses.

### Pre-Flight (Steps 1-2)

**Step 1: Check FreeCycle status and VRAM headroom.**

```
freecycle_status
```

Expected response includes `vram_used_mb`, `vram_total_mb`, and `vram_percent`. Note these values for later VRAM-gate validation (see "Pass/Fail Decision Rules" below). If `vram_percent` is already above 70%, warn the user that loading the benchmark model may trigger GPU saturation.

**Step 2: Confirm the target model is loaded.**

```
freecycle_list_models
```

If the target model is not installed:
- Check `freecycle_status` field `remote_model_installs_unlocked`. If `true`, use `freecycle_pull_model(model_name="...")` to install it.
- If `false`, inform the user that the FreeCycle tray's one-hour remote install unlock is not currently active, and you will proceed with already-installed models only.

Optionally, use `freecycle_show_model(model_name="...")` to inspect metadata (parameter count, quantization, description) before benchmarking.

### Latency and Throughput Measurement (Steps 3-5)

**Step 3: Select prompt difficulty tiers and prepare test prompts.**

Prepare 5 representative prompts for Quick Mode or 20 representative prompts for Full Mode. Organize them by difficulty:
- **Easy**: straightforward, common query (e.g., "What is the capital of France?").
- **Medium**: moderately complex, multi-part (e.g., "Summarize these points and reformat as a list").
- **Hard**: complex reasoning or context handling (e.g., "Explain the implications of X for Y and Z").
- **Edge**: boundary case or unusual input (e.g., malformed query, ambiguous reference, very long input).
- **Long-input**: normal difficulty but with large input context (e.g., >2000 token document).

Quick Mode: 1 prompt per tier = 5 total. Full Mode: 4 per tier = 20 total.

**Step 4: Run freecycle_benchmark for quick latency and throughput aggregation.**

For each prompt, call:

```
freecycle_benchmark(model="llama3.1:8b-instruct-q4_K_M", prompt="YOUR_PROMPT", iterations=3)
```

In Quick Mode, use `iterations=3`. In Full Mode, use `iterations=10` to reduce noise and capture warm-cache behavior.

The tool returns:
- `average_latency_ms` -- Mean latency across all iterations.
- `average_tokens_per_second` -- Mean throughput.
- `warm_average_latency_ms` -- Mean latency excluding first iteration (load penalty).
- `warm_average_tokens_per_second` -- Mean throughput excluding first iteration.
- `results[]` -- Per-iteration detail: `latency_ms`, `tokens_per_sec`, `load_duration_ms`, `generation_ms`, etc.

**Step 5: Aggregate across all prompts.**

After all `freecycle_benchmark` calls complete, aggregate the per-prompt results:

- **Overall average latency**: Mean of all `average_latency_ms` values.
- **Overall warm average latency**: Mean of all `warm_average_latency_ms` values.
- **Overall average throughput**: Mean of all `average_tokens_per_second` values.
- **P95 latency estimate**: Sort all individual result latencies; take the 95th percentile from the combined set.
- **Throughput range**: Min and max `tokens_per_sec` observed across all iterations.

Present this as a summary table:

| Metric | Quick Mode (x3) | Full Mode (x10) |
|---|---|---|
| Overall avg latency (ms) | [value] | [value] |
| Overall warm avg latency (ms) | [value] | [value] |
| Overall avg throughput (tokens/sec) | [value] | [value] |
| P95 latency estimate (ms) | [value] | [value] |
| Throughput range (tokens/sec) | [min-max] | [min-max] |

### Quality Sampling (Steps 6-7)

**Step 6: Signal task start for quality sampling.**

Before manual `freecycle_generate` calls, signal task start with a properly formatted description (30-40 characters):

```
freecycle_start_task(task_id="benchmark-quality-001", description="Benchmarking llama3.1 8B eval")
```

The canonical format is: `"Benchmarking {short_model} eval"`. For example:
- `"Benchmarking llama3.1 8B eval"` = 30 chars (minimum, acceptable).
- `"Benchmarking mixtral 8x7B eval"` = 32 chars.
- `"Benchmarking qwen 14B instruct"` = 31 chars.

This ensures the task description is within the 30-40 character constraint without triggering padding detection.

**Step 7: Run quality samples on the 5 hardest prompts.**

For each of the 5 hardest prompts from the test set, call:

```
freecycle_generate(model="llama3.1:8b-instruct-q4_K_M", prompt="YOUR_HARDEST_PROMPT")
```

Inspect the actual output text in conversation. Score each response on a 1-5 scale:
- **5**: Complete, accurate, well-structured answer. No errors or ambiguities.
- **4**: Mostly correct and complete. Minor omissions or stylistic issues, but usable.
- **3**: Adequate answer. Some errors, missing context, or structural issues, but covers the core topic.
- **2**: Weak answer. Significant errors or omissions that would require rework.
- **1**: Unusable. Incorrect, incoherent, or completely off-topic.

Record these scores inline (do not write to disk). After all 5 samples, compute the average quality score.

**Step 8: Signal task stop.**

```
freecycle_stop_task(task_id="benchmark-quality-001")
```

### Pass/Fail Decision Rules

Using the aggregated metrics and quality scores, apply these workload-specific pass/fail gates:

#### Latency Gates (applies to Q2=a "real-time" or Q2=b "interactive")

- **Real-time (Q2=a)**: Average latency must be < 1000 ms. If not, **FAIL**. This workload requires sub-second response times.
- **Interactive (Q2=b)**: Average latency must be < 5000 ms. If not, **FAIL**. This workload allows up to 5 seconds but needs predictable response times.
- **Batch/async (Q2=c)**: No latency gate. Throughput becomes the primary metric.

Use `warm_average_latency_ms` if available (excludes model load penalty), and note this in the result.

#### Throughput Gate (applies to Q2=c "batch/async" or volume-heavy workflows)

- **Batch/async**: Average throughput must be > 5 tokens/sec. If not, **FAIL**. Higher throughput (> 10 tokens/sec) is strongly preferred for high-volume tasks.

#### Quality Gate

- **Good enough workloads** (Q4=b, balanced quality): Average quality score must be >= 3/5 to **PASS**.
- **High-quality workloads** (Q4=a, maximum quality): Average quality score must be >= 4/5 to **PASS**. This tier has a stricter bar.

If quality score falls below these thresholds, **FAIL** and note which aspect(s) are weak (accuracy, completeness, formatting, etc.).

#### VRAM Gate

- Compare post-model-load `vram_percent` (from Step 1) to the `vram_percent` returned after the first `freecycle_benchmark` call.
- If `vram_percent` exceeds 90% at any point, flag as **OOM Risk**. The benchmark may be unstable or may trigger aggressive swapping.

If VRAM usage is above 90%, recommend keeping the model unloaded when not in use or routing this workload to cloud instead.

### Recommendation Output

Summarize the single-model benchmark result in a structured format:

```
## Benchmark Result: {model_name}

**Status**: [PASS] or [FAIL]

**Latency Summary**
- Average latency: {avg_ms} ms
- Warm average latency: {warm_avg_ms} ms (first load excluded)
- P95 latency estimate: {p95_ms} ms
- Gate: {gate_result} (target: {target})

**Throughput Summary**
- Average: {avg_tokens_per_sec} tokens/sec
- Range: {min_tokens_per_sec}–{max_tokens_per_sec} tokens/sec
- Gate: {gate_result} (target: {target})

**Quality Summary**
- Average score: {avg_score}/5 (5 samples)
- Gate: {gate_result} (target: >= {target}/5)

**VRAM Summary**
- Peak usage: {peak_vram_percent}% of {vram_total_mb} MB
- Gate: {gate_result or "OK"}

**Confidence**
[Full Mode: High | Quick Mode: Limited (smoke test only)]

**Recommendation**
[If PASS: "This model is suitable for {workload} on this hardware." | If FAIL: "This model does not meet {workload} requirements. Recommend: {cloud, larger model, or hybrid strategy}."]
```

### Multi-Mode Guidance

**Quick Mode (15 min budget)**: Run the procedure above with 5 prompts and 3 iterations. If the model **PASS**es all gates, recommend local or hybrid with **Limited confidence**. Suggest re-evaluation with a 30+ minute budget for higher confidence.

**Full Mode (30+ min budget)**: Run the procedure above with 20 prompts and 10 iterations. Full Mode results are **High confidence** unless additional factors (model inventory mismatch, edge case failures, etc.) lower confidence.

---

## 2-Model Combination Tests

Use this section when you are running in **Full Mode (Q6 = b or c, 30+ minutes)** and the workflow decomposes into distinct stages that could benefit from specialized models. A combination test benchmarks a two-stage local pipeline where two different local models handle complementary roles, answering the question: "Is the quality gain from model B worth the added latency and VRAM footprint when model A handles the cheap stages?"

### When to Run Combination Tests

Combination tests are only applicable in these scenarios:

- **Full Mode only**: Q6 answer is (b) or (c), 30+ minute time budget.
- **Multi-stage workflows**: The workflow decomposes into at least two distinct stages (e.g., embedding + retrieval, extraction + synthesis, classification + drafting).
- **After single-model baselines**: You have completed the single-model benchmark procedure for each stage individually and have baseline latency, throughput, and quality scores.
- **Complementary specialization**: The two models have different strengths. Classic patterns:
  - **Fast + Strong**: A lightweight 3B/7B model for low-cost extraction or classification feeds a larger 13B/32B model for generation or synthesis.
  - **Specialist split**: A code-specialist model for code-focused stages plus a general instruct model for prose/summarization stages.
  - **Embed + Generate**: An embedding model for retrieval or semantic routing plus a generative model for response drafting.

### Stage-Model Affinity Mapping

Before selecting candidate pairs, walk through this mapping exercise:

1. **List each workflow stage** (e.g., embed, retrieve, classify, draft, rewrite, verify, format).
2. **For each stage, note the primary task type** (embedding, classification, reasoning, generation, summarization, code).
3. **Identify installed or candidate models that fit each stage:**
   - Embedding models: `nomic-embed-text`, specialized embedding models.
   - Small/fast instruct: 3B, 7B parameter instruct models for filtering, extraction, or simple classification.
   - Medium instruct: 8B-13B parameter models for drafting, summarization, or multi-step reasoning.
   - Larger instruct: 20B+ parameter models for complex reasoning, code generation, or style-constrained rewriting.
4. **Assess VRAM constraints.** Check `freecycle_status` for `vram_total_mb`. Determine whether two models can load simultaneously or must load sequentially:
   - **Simultaneous**: Both models loaded at the same time. Ideal for latency but requires sufficient VRAM.
   - **Sequential**: Load model A, run stage A, unload model A, load model B, run stage B. Trades latency for VRAM efficiency.
5. **Select the top candidate pair.** Prefer pairing a fast, specialized model for an easy stage with a stronger model for a hard stage. Avoid pairing two large models unless necessary.

### Pipeline Latency Calculation

When both models run, measure the combined latency:

1. **Per-stage latency**: Use the `average_latency_ms` and `warm_average_latency_ms` values from the single-model benchmark for each stage.
2. **Pipeline total**: Sum the latencies across all stages.
3. **Inter-call overhead**: Add an estimated 50ms per stage boundary to account for model switching, I/O, and context passing overhead. For a two-stage pipeline, add 50ms (one transition).
4. **Load penalty (if sequential)**: If models load sequentially, the first request includes model A load time, then model B load time. Subsequent requests are warmer. Report both cold (first run) and warm (steady state) latencies.
5. **Compare to single-model baseline**: If using a single large model for both stages, compare the combo total against the single-model latency for the whole workflow.

Example:
```
Stage 1 (embedding): average_latency_ms = 150ms, warm = 120ms
Stage 2 (draft generation): average_latency_ms = 800ms, warm = 750ms
Inter-call overhead: 50ms
Combo total (warm): 120ms + 750ms + 50ms = 920ms

Single-model baseline (all in one): average_latency_ms = 1200ms, warm = 1100ms
Combo wins on latency: 920ms < 1100ms
```

### Quality Scoring

Benchmark the **final output** of the combined pipeline on the same 1-5 scale used in single-model quality sampling:

1. **Run the combo pipeline end-to-end** with the hardest prompts from your benchmark set.
2. **Score the final output** (not intermediate stages) on a 1-5 scale:
   - **5**: Complete, accurate, well-structured answer. No errors or ambiguities.
   - **4**: Mostly correct and complete. Minor omissions or stylistic issues, but usable.
   - **3**: Adequate answer. Some errors, missing context, or structural issues, but covers the core topic.
   - **2**: Weak answer. Significant errors or omissions that would require rework.
   - **1**: Unusable. Incorrect, incoherent, or completely off-topic.
3. **Compute average combo quality** across 5 hardest prompts.
4. **Compare against single-model baseline** for the same task. Does the combo pipeline produce equal or better quality?

### Pass/Fail Rule

A combination pipeline passes the benchmark if **both** of these conditions are met:

1. **Quality gate**: Final quality score >= single-model baseline quality score (or within 0.5 points if a small gain is acceptable).
2. **Latency gate**: Total pipeline latency (combo) stays within the Q2 gate (real-time < 1000ms, interactive < 5000ms, batch/async no limit).

If either gate fails, the combo pipeline does not offer a compelling advantage over the single-model baseline.

### Decision Heuristic

Prefer the combo pipeline plan **only when** one of these conditions is true:

- **Quality improvement**: The combo pipeline achieves an average quality score >= +0.5 points higher than the single-model baseline (e.g., 3.5 vs. 3.0).
- **Cost reduction**: The combo pipeline uses noticeably less VRAM or avoids loading a very large model, reducing system pressure even if quality is equal.
- **Both latency and quality meet gates**: The combo latency is faster than or equal to the single-model baseline while maintaining or improving quality.

Prefer the **simpler single-model path** if:

- The quality improvement is < 0.5 points and latency is similar.
- The combo requires sequential loading that adds significant latency without a quality win.
- The combo uses > 80% of total VRAM, creating OOM risk.
- Operational complexity (managing two models) outweighs the benefit.

### VRAM Interaction

Check VRAM availability before and after loading both models:

1. **Simultaneous loading**: Call `freecycle_status` before and after the combo run. If peak `vram_percent` exceeds 80%, flag as **High VRAM Pressure**. Note this in the recommendation.
2. **Sequential loading**: Model A loads, stage A runs, model A unloads, model B loads, stage B runs. This avoids peak VRAM spikes but adds latency due to unload/reload. Recommend sequential when:
   - Total VRAM (model A + model B) would exceed 80% of available VRAM.
   - The latency cost of sequential loading is acceptable (< 500ms additional overhead).
3. **Fallback to single large model**: If the combo requires both simultaneous loading with > 85% VRAM and sequential loading adds > 500ms latency, recommend a single larger model or routing one stage to cloud instead.

### Task Signaling for Combo Benchmarks

When running the combo pipeline benchmark, use a single `freecycle_start_task` / `freecycle_stop_task` pair wrapping the **entire pipeline run**, not one per stage:

```
freecycle_start_task(task_id="benchmark-combo-001", description="Benchmarking llama3+nomic pipeline")
[Run stage 1 with model A]
[Run stage 2 with model B]
freecycle_stop_task(task_id="benchmark-combo-001")
```

The task description must be 30-40 characters. Canonical format: `"Benchmarking {model_A}+{model_B} pipeline"`. Examples:
- `"Benchmarking llama3+nomic pipeline"` = 35 chars.
- `"Benchmarking mixtral+qwen combo"` = 32 chars.

### Combo Benchmark Output Format

Summarize the result in a structured format:

```
## Combo Benchmark Result: {model_A} + {model_B}

**Status**: [PASS] or [FAIL]

**Latency Summary**
- Stage 1 ({model_A}): {avg_ms} ms (warm: {warm_ms} ms)
- Stage 2 ({model_B}): {avg_ms} ms (warm: {warm_ms} ms)
- Pipeline total (warm): {total_warm_ms} ms
- Single-model baseline: {baseline_ms} ms
- Combo advantage: [Faster / Similar / Slower] by {delta_ms} ms

**Quality Summary**
- Combo average score: {avg_score}/5 (5 samples)
- Single-model baseline: {baseline_score}/5
- Gate: [PASS / FAIL] (target: >= baseline or +0.5 improvement)

**VRAM Summary**
- Peak simultaneous: {peak_percent}% of {vram_total_mb} MB
- Sequential alternative: [Yes/No, add ~{seq_overhead_ms}ms overhead]
- Status: [OK / High VRAM Pressure / OOM Risk]

**Recommendation**
[If PASS: "Combo pipeline is justified. Quality gain of +X, latency reduction of Y ms, VRAM efficiency is Z." | If FAIL: "Single-model path is simpler and comparable. Recommend: {single_model} or {hybrid_fallback}."]
```

---

## Local-Extract-Then-Cloud Pipeline Benchmark

Use this section when you are running in **Full Mode (Q6 = b or c, 30+ minutes)** and the workflow has a large raw input that could benefit from local preprocessing before being sent to a cloud model for final reasoning or synthesis. This pattern combines the privacy and cost benefits of local execution with the quality of cloud models.

### When to Run Local-Extract-Then-Cloud Tests

This pattern is applicable only in these scenarios:

- **Full Mode only**: Q6 answer is (b) or (c), 30+ minute time budget. Cloud API calls during benchmarking add real cost and latency. Quick Mode (15 minutes) is unsuitable.
- **Q3 = b or c only**: Some or no data restrictions. If Q3 = a (all data must stay local), this pattern is invalid because the extracted payload must leave the network to reach the cloud stage. Skip directly to local-only patterns.
- **Multi-stage with raw input reduction**: The workflow has a large raw input (e.g., a document, transcript, or search result set) that a fast local model can distill into a smaller, structured extraction before sending to cloud.
- **Privacy motivation when extraction is thorough**: If privacy is the concern, the extracted output must be complete enough that the cloud stage does not need to see the raw sensitive input.

### Pattern Overview

The local-extract-then-cloud pipeline has exactly two stages:

1. **Local extraction stage**: A small, fast local model extracts structured entities, key facts, or distilled summaries from raw input. The raw input stays on the LAN. Only the compact structured extraction leaves the network.
2. **Cloud final stage**: A cloud model (Claude, GPT-4, etc.) performs higher-quality reasoning, synthesis, or generation on the extracted structured payload, which is much smaller than the raw input.

This is distinct from a 2-model local combo because one leg of the pipeline is cloud, so cost estimation is mandatory. The privacy benefit is central: raw sensitive input never leaves the network; only distilled facts do.

### Stage-Model Affinity and Resource Check

1. **Identify the extraction task**: Is the local stage embedding, classification, summarization, entity extraction, or structured reformatting? Select a small, fast local model that fits this task.
2. **Identify the final task**: Is the cloud stage drafting, synthesis, reasoning, rewriting, or verification? Clarify what the cloud model will do with the extracted output.
3. **Assess local resources**: Call `freecycle_status` for `vram_total_mb`. Ensure the extraction model fits comfortably (< 60% VRAM).
4. **Estimate extraction output size**: Plan the extraction output format (JSON object, structured list, bullet points, etc.). Estimate the output token count compared to the raw input.

### Pre-Cloud Baseline (Cloud Direct)

Before benchmarking the pipeline, establish a baseline: send the full raw input to the cloud stage directly and measure:

1. **Cloud direct latency**: Time from submission to completion.
2. **Cloud token count (raw path)**: Tokens in and tokens out when the cloud model sees the full raw input.
3. **Cloud output quality**: Score on a 1-5 scale.

This baseline is the comparison point for evaluating whether the extraction pipeline offers a benefit.

### Local Extraction Latency and Compression

1. **Signal task start for the extraction phase:**

```
freecycle_start_task(task_id="benchmark-extract-001", description="Benchmarking llama3 extract pipeline")
```

The task description must be 30-40 characters. Canonical format: `"Benchmarking {short_model} extract pipeline"`. Examples:
- `"Benchmarking llama3 extract pipeline"` = 38 chars.
- `"Benchmarking qwen7b extract pipeline"` = 37 chars.

2. **Run freecycle_benchmark on the extraction model:**

Use 10 iterations (Full Mode) on representative raw inputs of varying sizes. For each input, record:
- Average latency for the extraction phase.
- Warm average latency (excluding first model load).
- Extraction output size in tokens.

Example call:
```
freecycle_benchmark(model="llama3.1:7b-instruct-q4_K_M", prompt="Extract key facts from: [RAW_INPUT]", iterations=10)
```

3. **Calculate extraction compression ratio:**

For each test input: (extracted token count) / (raw input token count). Higher ratios mean more cost savings.

Record:
- Average extraction latency across all inputs.
- Warm average extraction latency.
- Average compression ratio (e.g., 0.3 means the extraction is 30% of the raw input size).

4. **Signal task stop:**

```
freecycle_stop_task(task_id="benchmark-extract-001")
```

### Extraction Fidelity Score

The extraction must preserve sufficient context for the cloud stage to produce a correct final answer. Score the extraction quality separately from the final output quality.

1. **Run the extraction on 5 representative raw inputs** using `freecycle_generate`.
2. **For each extracted output, manually verify**: Does the extracted output contain all necessary facts, entities, and context for the cloud stage to complete the final task correctly?
3. **Score on a 1-5 scale:**
   - **5**: Extraction is complete and well-structured. Cloud stage has everything needed.
   - **4**: Extraction is mostly complete. Minor omissions that could be inferred from cloud context.
   - **3**: Extraction captures key points but misses some nuance. Cloud stage may need to make assumptions.
   - **2**: Extraction is sparse or unclear. Cloud stage may produce a weaker or hallucinated final answer.
   - **1**: Extraction is incomplete or incorrect. Cloud stage cannot produce a correct answer.

4. **Record the average extraction fidelity score** across 5 samples.

### Cloud Synthesis on Extracted Payload

1. **Prepare the 5 extracted outputs** from the fidelity scoring step above.
2. **For each extracted output, submit to your cloud model** (Claude, OpenAI, etc.) with the final task prompt (reasoning, synthesis, drafting, etc.).
3. **Measure cloud latency** from submission to completion.
4. **Record cloud token counts:** tokens in (extracted payload only) and tokens out.
5. **Score the final output on a 1-5 scale** using the same rubric as the cloud-direct baseline (Step 3 in "Pre-Cloud Baseline" above).

Example:

```
Cloud model: Claude Sonnet
Extracted payload (sample 1): "{fact_a, fact_b, fact_c}" (25 tokens)
Cloud final task: "Synthesize into a summary"
Cloud latency: 200ms
Cloud tokens in: 25 + 150 (system prompt) = 175 tokens
Cloud tokens out: 80 tokens
Final output quality: 4.5/5
```

### Cost Estimation Block

Compare the two approaches:

**Cloud Direct (no extraction):**
- Average input tokens: {raw_input_tokens}
- Average output tokens: {cloud_output_tokens}
- Per-request cost: {tokens_in} * {price_per_in_token} + {tokens_out} * {price_per_out_token} = ${per_request_cost}
- Monthly cost (at {requests_per_month} requests): ${monthly_cost_direct}

**Local-Extract-Then-Cloud:**
- Local extraction cost: $0 (free, on-premises)
- Local extraction latency: {avg_extraction_ms} ms (warm average)
- Extraction output tokens: {extracted_tokens} (average)
- Cloud input tokens: {extracted_tokens} + {system_prompt_tokens} = {cloud_input_tokens_via_extraction}
- Cloud output tokens: {cloud_output_tokens} (typically same as direct)
- Per-request cost: {cloud_input_tokens_via_extraction} * {price_per_in_token} + {cloud_output_tokens} * {price_per_out_token} = ${per_request_cost_extraction}
- Monthly cost (at {requests_per_month} requests): ${monthly_cost_extraction}
- **Savings vs. direct**: ${monthly_cost_direct} - ${monthly_cost_extraction} = ${monthly_savings} ({savings_percent}%)

**Placeholder note:** Use per-token pricing from the current `cloud-pricing.md` reference (being finalized in the next Priority 14 task). For now, instruct the evaluator to look up current pricing for their chosen cloud provider and plug in the values at evaluation time.

If cost savings are below 30%, the pattern adds operational complexity with minimal financial benefit. Recommend sending raw input to cloud directly.

### Pass/Fail Decision Rules

A local-extract-then-cloud pipeline passes if **all three** of these conditions are met:

1. **Extraction fidelity gate**: Average extraction fidelity score >= 3.5/5. If fidelity is too low, the cloud stage is starved of information and cannot compensate with higher quality.
2. **Final quality gate**: Average final output quality >= cloud-direct baseline quality minus 0.3 points. This allows a small acceptable degradation (e.g., 4.5 vs. 4.2) in exchange for cost savings. If the final output is appreciably worse, the extraction is throwing away information the cloud stage needs.
3. **Cost savings gate**: Extraction reduces cloud input token count by at least 30%. If savings are below 30%, the pattern adds complexity with minimal benefit.

If the extraction fidelity gate fails, do not continue. Recommend sending raw input to cloud directly or upgrading the local extraction model.

### Pipeline Latency Calculation

When both stages run in sequence:

1. **Local extraction latency**: Use the `warm_average_latency_ms` from `freecycle_benchmark` (excludes first model load).
2. **Cloud final latency**: Measured from submission to completion (does not include network RTT, which is typically 50-200ms depending on your cloud provider).
3. **Inter-stage overhead**: Add ~100ms to account for serialization, network round-trip, and cloud service initialization.
4. **Total pipeline latency**: {extraction_ms} + {cloud_latency_ms} + 100ms overhead.

Example:

```
Local extraction: 150ms (warm average)
Cloud synthesis: 400ms (measured)
Inter-stage overhead: 100ms
Total pipeline latency: 150 + 400 + 100 = 650ms

Cloud direct baseline: 800ms (raw input takes longer to process)
Extraction pipeline wins on latency: 650ms < 800ms
```

### Task Signaling for Extraction Benchmarks

Use a single `freecycle_start_task` / `freecycle_stop_task` pair wrapping only the **local extraction phase**. The cloud phase is outside FreeCycle's awareness.

Canonical task description format (30-40 chars): `"Benchmarking {short_model} extract pipeline"`. Examples:
- `"Benchmarking llama3 extract pipeline"` = 38 chars.
- `"Benchmarking qwen7b extract pipeline"` = 37 chars.

### Benchmark Results Persistence Constraint

Benchmark results and intermediate extraction outputs must remain in conversation context only and never be persisted to disk. This protects sensitive workload data (raw inputs and extracted facts).

### Local-Extract-Then-Cloud Benchmark Output Format

Summarize the result in a structured format:

```
## Local-Extract-Then-Cloud Benchmark Result

**Status**: [PASS] or [FAIL]

**Extraction Summary**
- Local model: {model_name}
- Average extraction latency (warm): {avg_ms} ms
- Extraction compression ratio: {ratio} (raw input reduced to {ratio}% of original size)
- Average extraction fidelity score: {score}/5 (5 samples)
- Extraction fidelity gate: [PASS / FAIL] (target: >= 3.5/5)

**Cloud Synthesis Summary**
- Cloud model: {cloud_model_name}
- Cloud latency (extracted path): {cloud_ms} ms
- Cloud input tokens (extracted): {extracted_tokens} tokens (vs. {raw_tokens} tokens raw)
- Cloud output tokens: {output_tokens} tokens
- Final output quality (extracted path): {score}/5 (5 samples)
- Cloud-direct baseline quality: {baseline_score}/5
- Final quality gate: [PASS / FAIL] (target: >= baseline - 0.3 points)

**Latency Summary**
- Local extraction: {extraction_ms} ms
- Cloud synthesis: {cloud_ms} ms
- Inter-stage overhead: ~100ms
- Total pipeline latency: {total_ms} ms
- Cloud direct baseline: {direct_baseline_ms} ms
- Pipeline advantage: [Faster / Similar / Slower] by {delta_ms} ms

**Cost Analysis**
- Cloud direct (raw path): {direct_cost}/request, ${direct_monthly}/month
- Extraction pipeline (reduced tokens): {extraction_cost}/request, ${extraction_monthly}/month
- Cost savings gate: [PASS / FAIL] (target: >= 30% reduction)
- Monthly savings: ${monthly_savings} ({savings_percent}%)

**VRAM Summary**
- Peak VRAM during extraction: {peak_percent}% of {vram_total_mb} MB
- Status: [OK / High Pressure]

**Recommendation**
[If PASS: "Local-extract-then-cloud pipeline is justified. Extraction fidelity is strong ({fidelity_score}/5), final quality is acceptable ({final_score}/5 vs. baseline {baseline_score}/5), and monthly cost savings are {savings_percent}%." | If FAIL: "Extract-then-cloud does not meet pass criteria. {Reason: extraction fidelity too low / final quality degradation too high / cost savings below 30%}. Recommend: {send raw to cloud, or upgrade extraction model}."]
```

---

## Model Fit Gate

Before recommending a local model for production traffic:

1. Call `freecycle_list_models`.
2. Decompose the workflow into stages if needed, such as `embed`, `retrieve`, `classify`, `draft`, `rewrite`, `verify`, and `format`.
3. Mark each installed model as `clear fit`, `possible fit`, or `poor fit` for each critical stage.
4. If no installed model is a clear fit for a critical stage, lower confidence immediately.
5. Either run the candidate-model exploration loop below or keep that stage in cloud.

Prefer specialized models per stage when the quality gain is material, such as a dedicated embedding model plus a separate instruct or coder model.

## Candidate Model Exploration Loop (Full Mode only)

Use this loop when the installed models are not an obvious fit, or when the evaluation remains low confidence after the first pass. This section applies to Full Mode (Q6 = b or c, 30+ minutes). Quick Mode users should skip this loop and proceed with installed models only.

1. Use the `freecycle_model_catalog` tool to shortlist 2 to 4 candidate models for each critical workflow stage.
2. Include at least one conservative option likely to fit the current VRAM budget and one stronger option if the hardware can support it.
3. Check `freecycle_status` for `remote_model_installs_unlocked`.
4. If `remote_model_installs_unlocked=true`, proceed to automatically pull candidates in the next step without asking the user for permission (they have consented by enabling the tray unlock). If `false`, inform the user that the GPU machine owner must enable the tray's one-hour remote install unlock before candidates can be pulled, and proceed with installed models only.
5. When the unlock is active, automatically pull one candidate at a time with `freecycle_pull_model(model_name="...")`. Do not download a large batch before you have smoke-test evidence. If a pull fails due to unlock expiry, catch the error, inform the user, and continue with models that were already pulled successfully.
6. Run a 5 to 10 prompt smoke test per candidate with `freecycle_generate` or `freecycle_chat`, plus `freecycle_benchmark` for latency and throughput.
7. Score each candidate against the specific stage it is meant to serve.
8. Keep only the top 1 to 2 candidates per stage, then run the full 20 to 50 prompt benchmark set.
9. Compare two plans before finalizing the recommendation:
   - a simpler single-model path
   - a best-fit multi-model path
10. If the multi-model path meaningfully improves quality or cost for critical stages, recommend it.
11. If the gain is marginal, prefer the simpler path.
12. If no local candidate clears the required quality bar for a critical stage, route that stage or the whole workflow to cloud.

## Benchmark Dataset Guidance

Prepare a representative benchmark set (Full Mode guidance; Quick Mode uses 5 prompts instead):

- 20 to 50 prompts total (Full Mode only)
- easy, medium, and hard cases
- real examples from the user's workflow when possible
- gold outputs for high-fidelity stages such as rewriting or verification

When the workflow has multiple stages, benchmark each stage separately before benchmarking the full pipeline.

## Workload Specific Benchmarks

### Embeddings

- Measure vectors per second.
- Compare cosine similarity on a known test set such as STS Benchmark.
- `nomic-embed-text` is usually the default local baseline.

### Classification

- Use a labeled dataset.
- Measure accuracy, precision, recall, and F1.
- Local models often perform comparably for simple classification.

### Code Generation

- Use HumanEval or a task-specific coding suite.
- Small local instruct models are usually materially weaker than strong cloud models.
- Expect cloud or hybrid to win unless privacy or cost forces local execution.

### Summarization

- Use ROUGE or human evaluation.
- Short document summarization can fit local models.
- Long document summarization often benefits from stronger cloud models and larger context windows.

### Style Constrained Rewriting or Voice Matching

- Use 5 to 10 gold reference outputs written in the target voice.
- Score tone fidelity, phrase drift, formatting obedience, and factual preservation separately.
- Treat exact named persona or brand voice matching as cloud-leaning until a local model proves otherwise.
- Use local models for drafts only when some drift is acceptable.

### RAG

- Benchmark the full pipeline: embed, retrieve, generate.
- Measure end-to-end latency.
- Local embeddings plus cloud generation is a common hybrid pattern.

## Suggesting Additional Local Models

When the user wants to improve local quality instead of routing to cloud, use the `freecycle_model_catalog` tool and suggest models that match the workload and hardware budget.

Typical patterns:

- General chat and summarization: newer Llama or Qwen instruct models when the GPU has enough VRAM
- Code generation: coder-focused models when privacy or cost requires local execution
- Reasoning-heavy local fallback: larger Qwen, Llama, or distilled reasoning variants if the user can tolerate slower latency
- Style constrained rewriting: stronger instruct models with good instruction-following and long-context behavior, but keep the recommendation provisional until the gold-example benchmark passes
- Embeddings: keep `nomic-embed-text` by default unless retrieval quality is the bottleneck

Always explain the tradeoff: better local quality usually means larger downloads, more VRAM pressure, and more frequent need to route to cloud when gaming starts.

## Persistent Evaluation Artifacts

When the evaluation is likely to be rerun:

- store benchmark prompts in source control
- store gold outputs for sensitive stages
- store score rubrics in code or config
- keep per-model scorecards so the next evaluation does not start from zero

That turns future evaluations into incremental updates instead of repeated exploratory tool usage.
