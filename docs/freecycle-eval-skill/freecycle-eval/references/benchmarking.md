# Benchmarking and Model Selection

Use this reference when the main skill determines that local or hybrid execution is plausible and you need more than a coarse recommendation.

## Model Fit Gate

Before recommending a local model for production traffic:

1. Call `freecycle_list_models`.
2. Decompose the workflow into stages if needed, such as `embed`, `retrieve`, `classify`, `draft`, `rewrite`, `verify`, and `format`.
3. Mark each installed model as `clear fit`, `possible fit`, or `poor fit` for each critical stage.
4. If no installed model is a clear fit for a critical stage, lower confidence immediately.
5. Either run the candidate-model exploration loop below or keep that stage in cloud.

Prefer specialized models per stage when the quality gain is material, such as a dedicated embedding model plus a separate instruct or coder model.

## Candidate Model Exploration Loop

Use this loop when the installed models are not an obvious fit, or when the evaluation remains low confidence after the first pass:

1. Browse the official [Ollama Library](https://ollama.com/library) and shortlist 2 to 4 candidate models for each critical workflow stage.
2. Include at least one conservative option likely to fit the current VRAM budget and one stronger option if the hardware can support it.
3. Check `freecycle_status` for `remote_model_installs_unlocked`.
4. If remote installs are locked, tell the user the GPU machine owner must enable the tray's one-hour remote install unlock before remote pulls can succeed.
5. Pull one candidate at a time with `freecycle_pull_model(model_name="...")`. Do not download a large batch before you have smoke-test evidence.
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

Prepare a representative benchmark set:

- 20 to 50 prompts total
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

When the user wants to improve local quality instead of routing to cloud, inspect the [Ollama Library](https://ollama.com/library) and suggest models that match the workload and hardware budget.

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
