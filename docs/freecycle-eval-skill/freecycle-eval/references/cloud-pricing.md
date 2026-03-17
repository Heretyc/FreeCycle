# Cloud Provider Pricing Reference

> **Pricing last collected: March 2026.**
>
> Cloud provider pricing changes frequently. Always verify against the provider's current pricing page before using these numbers in cost estimates. Links to official pricing pages are included for each provider.

---

## Anthropic

Claude is well-suited for reasoning tasks, long-context analysis, and high-quality code generation. Opus 4.6 is the flagship with the highest quality, while Haiku 4.5 is the most cost-effective.

| Model | Input ($/M tokens) | Output ($/M tokens) | Notes |
|---|---|---|---|
| Claude Opus 4.6 | 5.00 | 25.00 | Highest quality, best for complex reasoning and math |
| Claude Sonnet 4.6 | 3.00 (up to 200K), ~6.00 (beyond 200K) | 15.00 (up to 200K), ~22.50 (beyond 200K) | Balanced quality and cost, recommended for most tasks |
| Claude Haiku 4.5 | 0.25 | 1.25 | Most cost-effective, suitable for classification and simple inference |

**Pricing page:** https://platform.claude.com/docs/en/about-claude/pricing

**Free tier:** No free tier, but all new accounts receive $5 in free credits for the first month.

**Volume discounts:** Prompt caching (50% discount on cached input tokens beyond 1024 token threshold). Batch API pricing (50% discount) available on request for high-volume workloads.

---

## OpenAI

GPT-4o and GPT-4o mini are multi-modal and well-suited for vision, code generation, and reasoning. GPT-4.1 and o3-mini are newer alternatives with different cost/quality tradeoffs.

| Model | Input ($/M tokens) | Output ($/M tokens) | Notes |
|---|---|---|---|
| GPT-4o | 2.50 | 10.00 | Multi-modal, balanced quality and cost |
| GPT-4o mini | 0.15 | 0.60 | Most cost-effective with vision support |

**Pricing page:** https://openai.com/api/pricing/

**Free tier:** No free tier, but new accounts receive $5 in free credits valid for 3 months.

**Volume discounts:** Batch API pricing (50% discount) available for non-real-time inference. Structured output support available without extra cost.

---

## Google Gemini

Google offers multiple Gemini models at different quality tiers and context lengths. Flash and Flash-Lite are optimized for cost and speed. Pro models are better for complex reasoning.

| Model | Input ($/M tokens) | Output ($/M tokens) | Notes |
|---|---|---|---|
| Gemini 2.5 Pro | 1.25 | 10.00 | Latest flagship, best for reasoning |
| Gemini 2 Flash | 0.30 | 2.50 | Optimized for speed and cost |
| Gemini 2 Flash-Lite | 0.10 | 0.40 | Most cost-effective for simple tasks |

**Pricing page:** https://ai.google.dev/gemini-api/docs/pricing

**Free tier:** Yes. Free plan includes 60 requests per minute with rate limits. No credit card required to start.

**Notes:** Pricing varies by context window length. Longer contexts (>200K tokens) may have different rates. Check official docs for precise tiers.

---

## Mistral

Mistral offers several models from large (2400+ parameters) to small (7B). Prices are among the most competitive. Good for cost-conscious workloads.

| Model | Input ($/M tokens) | Output ($/M tokens) | Notes |
|---|---|---|---|
| Mistral Large 2411 | 2.00 | 6.00 | Most capable, good balance of quality and cost |
| Mistral Medium 3 | 0.40 | 2.00 | Solid middle ground |
| Mistral 7B Instruct v0.3 | 0.14 | 0.20 | Very cost-effective for simple tasks |
| Mistral 7B | 0.20 | 0.20 | General-purpose open model hosting |

**Pricing page:** https://mistral.ai/pricing

**Free tier:** Yes. Free plan with limited requests. No credit card required.

---

## Cohere

Cohere specializes in embeddings, retrieval, and text generation. Command R+ is their flagship. Command R7B is extremely cost-effective.

| Model | Input ($/M tokens) | Output ($/M tokens) | Notes |
|---|---|---|---|
| Command R+ | 2.50 | 10.00 | Best quality, suitable for complex reasoning |
| Command R | 0.15 | 0.60 | Good balance, suitable for most tasks |
| Command R7B | 0.04 | 0.15 | Most cost-effective option |

**Pricing page:** https://cohere.com/pricing

**Free tier:** Yes. Limited free requests with no credit card required. Perfect for testing.

**Embeddings:** Cohere also offers embedding models (Embed v3) with separate per-token pricing for embeddings vs. generation.

---

## Together AI

Together AI hosts open models from Meta and others. Llama models are popular for local-to-cloud comparison and extremely cost-effective. Prices are among the lowest available.

| Model | Input ($/M tokens) | Output ($/M tokens) | Blended Price | Notes |
|---|---|---|---|---|
| Llama 3.1 70B | Standard | Standard | ~0.88 / 1M tokens | Good for complex reasoning, cost-effective alternative to larger closed models |
| Llama 3.1 8B | Standard | Standard | ~0.18 / 1M tokens | Very cost-effective for classification and simple generation |
| Llama 4 Maverick | Standard | Standard | ~0.27 / 1M tokens | Latest open flagship, 80%+ cheaper than GPT-4o |

**Pricing page:** https://www.together.ai/pricing

**Free tier:** Yes. $5 in free credits. No credit card required.

**Notes:** Blended pricing (combined input/output rate) is quoted per request. Pay-as-you-go starting at $0.10/M tokens for smaller models.

---

## Fireworks AI

Fireworks specializes in fast inference for open models and provides competitive pricing with special discounts for cached and batch inference.

| Pricing Range | Type | Notes |
|---|---|---|
| $0.20 to $1.55 per 1M tokens | Model-dependent | Range depends on model size and complexity |
| Typical hosted pricing: $0.05 to $0.90 per 1M tokens | Standard inference | Most common models fall in this range |

**Pricing page:** https://fireworks.ai/pricing

**Volume discounts:** Cached input tokens are priced at 50% of standard rate (especially valuable for RAG workloads with repeated context). Batch inference is priced at 50% of serverless pricing for both input and output tokens.

**Free tier:** Yes. Free trial with credits. No credit card required to start.

**Notes:** Fine-tuned models cost the same as base models. Only pay for the training run once.

---

## Groq

Groq specializes in ultra-fast inference via proprietary hardware. Prices are competitive, and throughput is exceptional. Ideal for real-time and batch workloads where latency matters.

| Model | Input ($/M tokens) | Output ($/M tokens) | Notes |
|---|---|---|---|
| Llama 3.3 70B Versatile 128K | 0.59 | 0.79 | Latest and fastest, good for reasoning |
| Llama 3.1 8B | 0.06 (blended) | - | Ultra-fast and cheap, great for simple tasks |

**Pricing page:** https://groq.com/pricing

**Free tier:** Yes. Free tier with rate limits. No credit card required.

**Batch processing:** 50% cost reduction on batch API for non-real-time inference, enabling very cheap high-volume workloads.

**Notes:** Groq's latency advantage (sub-100ms for token generation) makes it excellent for real-time use cases, especially on low-spec hardware.

---

## Summary and Recommendations

### By Use Case

- **Maximum quality reasoning:** Anthropic Opus 4.6, OpenAI GPT-4o
- **Best value reasoning:** Mistral Large 2411, Anthropic Sonnet 4.6
- **Cost-sensitive tasks:** Cohere Command R7B, Mistral 7B, Groq Llama 3.1 8B
- **Low-latency real-time:** Groq (fastest by margin), Fireworks AI (with caching)
- **Embeddings and RAG:** Cohere (optimized), local nomic-embed-text (free)
- **Code generation:** OpenAI GPT-4o, Anthropic Opus 4.6, Mistral Large

### By Price Tier

- **Under $0.20 per 1M tokens:** Mistral 7B, Cohere Command R7B, Groq Llama 8B, Google Flash-Lite
- **$0.20 to $1.00:** Google Flash, Mistral Medium 3, Anthropic Haiku, Cohere Command R, Groq 70B
- **$1.00 to $3.00:** Mistral Large, Anthropic Sonnet, OpenAI GPT-4o mini
- **$3.00 and above:** Anthropic Opus, OpenAI GPT-4o, Cohere Command R+

### Batch vs. Real-Time

If your workload can tolerate 30 minutes to several hours of latency (batch, scheduled background tasks, offline analysis), use batch APIs where available:
- OpenAI Batch API: 50% discount
- Groq Batch API: 50% discount
- Fireworks Batch: 50% discount

These can cut your cloud costs in half for non-urgent workloads.

---

## Cost Estimation Template

When completing Phase 3 of the evaluation skill, use this template to estimate monthly cloud costs:

```
Estimated volume: X requests per month
Average tokens per request: Y tokens (multiply by 2 for input + output)
Monthly tokens: X * Y * 2 = Z million tokens
Cost per provider: Z * rate = $Cost/month
```

Example: 1000 requests/month, avg 1000 tokens per request = 2M tokens/month.
- Claude Sonnet 4.6: 2M * $0.0045 (avg in/out) = $9/month
- OpenAI GPT-4o: 2M * $0.00625 (avg in/out) = $12.50/month
- Groq Llama 8B: 2M * $0.00006 = $0.12/month
