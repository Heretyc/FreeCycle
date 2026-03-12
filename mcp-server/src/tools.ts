/**
 * MCP tool definitions for FreeCycle and Ollama.
 *
 * Registers 13 tools covering status/lifecycle, model management,
 * inference, task evaluation, and benchmarking.
 */

import { McpServer } from "@modelcontextprotocol/sdk/server/mcp.js";
import { z } from "zod";
import * as fc from "./freecycle-client.js";
import * as ollama from "./ollama-client.js";

// ── Helpers ──────────────────────────────────────────────────────────

function textResult(text: string) {
  return { content: [{ type: "text" as const, text }] };
}

function jsonResult(data: unknown) {
  return textResult(JSON.stringify(data, null, 2));
}

// Simple task classifier keywords
const LOCAL_KEYWORDS = [
  "summarize", "summarization", "summary",
  "embed", "embedding", "embeddings",
  "classify", "classification",
  "explain", "explanation",
  "translate", "translation",
  "extract", "extraction",
  "sentiment", "tag", "label",
  "rewrite", "paraphrase",
  "simple", "basic",
];

const CLOUD_KEYWORDS = [
  "complex code", "advanced code", "code generation",
  "math proof", "theorem", "formal verification",
  "research", "analysis", "deep reasoning",
  "creative writing", "novel", "story",
  "multi-step reasoning", "planning",
  "architecture", "system design",
];

function classifyTask(desc: string): "local" | "cloud" | "hybrid" {
  const lower = desc.toLowerCase();
  const localScore = LOCAL_KEYWORDS.filter((k) => lower.includes(k)).length;
  const cloudScore = CLOUD_KEYWORDS.filter((k) => lower.includes(k)).length;
  if (cloudScore > localScore) return "cloud";
  if (localScore > 0 && cloudScore === 0) return "local";
  return "hybrid";
}

// ── Registration ─────────────────────────────────────────────────────

export function registerTools(server: McpServer): void {

  // ── Status and lifecycle tools ──

  server.tool(
    "freecycle_status",
    "Get the complete FreeCycle status including GPU availability, VRAM usage, " +
    "Ollama state, active agent tasks, local IP, port info, and blocking processes. " +
    "Agentic workflows should call this FIRST before taking any other action to " +
    "understand the current state of the system.",
    {},
    async () => {
      const status = await fc.getStatus();
      return jsonResult(status);
    },
  );

  server.tool(
    "freecycle_health",
    "Quick health check. Returns whether the FreeCycle agent server is reachable. " +
    "Use this for a fast connectivity test before attempting heavier operations.",
    {},
    async () => {
      const res = await fc.healthCheck();
      return jsonResult(res);
    },
  );

  server.tool(
    "freecycle_start_task",
    "Signal that an agentic workflow is beginning GPU work. The FreeCycle tray " +
    "icon turns blue and the task is shown in the tooltip. Returns a 409 conflict " +
    "if the GPU is currently blocked (game running or cooldown active). Always " +
    "call freecycle_status first to check availability.",
    {
      task_id: z.string().min(1).describe("Unique identifier for this task."),
      description: z.string().min(1).describe("Human readable description of the GPU work being performed."),
    },
    async ({ task_id, description }) => {
      const res = await fc.startTask(task_id, description);
      return jsonResult(res);
    },
  );

  server.tool(
    "freecycle_stop_task",
    "Signal that an agentic workflow has finished GPU work. The FreeCycle tray " +
    "icon reverts to green. Returns 404 if the task_id is not found.",
    {
      task_id: z.string().min(1).describe("The task_id that was provided when starting the task."),
    },
    async ({ task_id }) => {
      const res = await fc.stopTask(task_id);
      return jsonResult(res);
    },
  );

  server.tool(
    "freecycle_check_availability",
    "Convenience tool that checks whether the GPU is currently available for " +
    "agentic work. Returns a boolean 'available' flag along with the current " +
    "status label and any blocking reasons.",
    {},
    async () => {
      const status = await fc.getStatus();
      const available =
        status.status === "Available" || status.status === "AgentTaskActive";
      const reasons: string[] = [];
      if (status.blocking_processes.length > 0) {
        reasons.push(`Blocking processes: ${status.blocking_processes.join(", ")}`);
      }
      if (!status.ollama_running) {
        reasons.push("Ollama is not running");
      }
      if (status.active_task_id) {
        reasons.push(`Active task: ${status.active_task_id}`);
      }
      return jsonResult({
        available,
        status: status.status,
        ollama_running: status.ollama_running,
        vram_percent: status.vram_percent,
        reasons: reasons.length > 0 ? reasons : ["GPU is available for work"],
      });
    },
  );

  // ── Model management tools ──

  server.tool(
    "freecycle_list_models",
    "List all models currently downloaded on the local Ollama instance. " +
    "Returns model names, sizes, and metadata.",
    {},
    async () => {
      const res = await ollama.listModels();
      const models = res.models.map((m) => ({
        name: m.name,
        size_mb: Math.round(m.size / (1024 * 1024)),
        modified_at: m.modified_at,
        digest: m.digest.slice(0, 12),
      }));
      return jsonResult({ count: models.length, models });
    },
  );

  server.tool(
    "freecycle_show_model",
    "Get detailed information about a specific Ollama model including its " +
    "parameters, template, and model info. Useful for understanding model " +
    "capabilities before running inference.",
    {
      model_name: z.string().min(1).describe("Name of the model to inspect (e.g. 'llama3.1:8b-instruct-q4_K_M')."),
    },
    async ({ model_name }) => {
      const res = await ollama.showModel(model_name);
      return jsonResult(res);
    },
  );

  server.tool(
    "freecycle_pull_model",
    "Request download of a new model to the local Ollama instance. This may " +
    "take a long time for large models. The request blocks until the download " +
    "completes (up to 10 minutes).",
    {
      model_name: z.string().min(1).describe("Name of the model to pull (e.g. 'codellama:7b')."),
    },
    async ({ model_name }) => {
      const res = await ollama.pullModel(model_name);
      return jsonResult(res);
    },
  );

  // ── Inference tools ──

  server.tool(
    "freecycle_generate",
    "Send a text generation (completion) request to the local Ollama instance. " +
    "Best for single turn prompts like summarization, code explanation, or " +
    "classification. Returns the generated text plus timing metadata.",
    {
      model: z.string().default("llama3.1:8b-instruct-q4_K_M").describe("Model name. Defaults to 'llama3.1:8b-instruct-q4_K_M'."),
      prompt: z.string().min(1).describe("The text prompt to complete."),
      system_prompt: z.string().optional().describe("Optional system prompt to set context."),
      temperature: z.number().min(0).max(2).optional().describe("Sampling temperature (0.0 to 2.0). Lower is more deterministic."),
      max_tokens: z.number().int().positive().optional().describe("Maximum tokens to generate."),
    },
    async ({ model, prompt, system_prompt, temperature, max_tokens }) => {
      const res = await ollama.generate(model, prompt, {
        system: system_prompt,
        temperature,
        num_predict: max_tokens,
      });
      const evalCount = res.eval_count ?? 0;
      const evalDurationNs = res.eval_duration ?? 1;
      const tokensPerSec = evalCount / (evalDurationNs / 1e9);
      return jsonResult({
        response: res.response,
        model: res.model,
        tokens_generated: evalCount,
        tokens_per_second: Math.round(tokensPerSec * 100) / 100,
        total_duration_ms: res.total_duration
          ? Math.round(res.total_duration / 1e6)
          : null,
      });
    },
  );

  server.tool(
    "freecycle_chat",
    "Send a multi turn chat completion request to the local Ollama instance. " +
    "Use this for conversational interactions that require message history. " +
    "Each message has a role ('system', 'user', or 'assistant') and content.",
    {
      model: z.string().default("llama3.1:8b-instruct-q4_K_M").describe("Model name. Defaults to 'llama3.1:8b-instruct-q4_K_M'."),
      messages: z.array(
        z.object({
          role: z.enum(["system", "user", "assistant"]),
          content: z.string(),
        }),
      ).min(1).describe("Array of chat messages. Each must have 'role' and 'content'."),
      system_prompt: z.string().optional().describe("Optional system prompt prepended to messages."),
      temperature: z.number().min(0).max(2).optional().describe("Sampling temperature (0.0 to 2.0)."),
    },
    async ({ model, messages, system_prompt, temperature }) => {
      const res = await ollama.chat(model, messages, {
        system: system_prompt,
        temperature,
      });
      const evalCount = res.eval_count ?? 0;
      const evalDurationNs = res.eval_duration ?? 1;
      const tokensPerSec = evalCount / (evalDurationNs / 1e9);
      return jsonResult({
        message: res.message,
        model: res.model,
        tokens_generated: evalCount,
        tokens_per_second: Math.round(tokensPerSec * 100) / 100,
        total_duration_ms: res.total_duration
          ? Math.round(res.total_duration / 1e6)
          : null,
      });
    },
  );

  server.tool(
    "freecycle_embed",
    "Generate vector embeddings for one or more text inputs using the local " +
    "Ollama instance. Useful for semantic search, clustering, or RAG pipelines.",
    {
      model: z.string().default("nomic-embed-text").describe("Embedding model name. Defaults to 'nomic-embed-text'."),
      input: z.union([
        z.string().min(1),
        z.array(z.string().min(1)).min(1),
      ]).describe("A single string or array of strings to embed."),
    },
    async ({ model, input }) => {
      const res = await ollama.embed(model, input);
      const dims = res.embeddings.length > 0 ? res.embeddings[0].length : 0;
      return jsonResult({
        model: res.model,
        embedding_count: res.embeddings.length,
        dimensions: dims,
        embeddings: res.embeddings,
        total_duration_ms: res.total_duration
          ? Math.round(res.total_duration / 1e6)
          : null,
      });
    },
  );

  // ── Evaluation and routing tools ──

  server.tool(
    "freecycle_evaluate_task",
    "Evaluate whether a given task should be run locally (on FreeCycle/Ollama), " +
    "on cloud (Claude, OpenAI), or in a hybrid split. Considers GPU availability, " +
    "task complexity, latency, quality, cost, and privacy requirements. Returns a " +
    "recommendation with reasoning.",
    {
      task_description: z.string().min(1).describe("Description of the task to evaluate."),
      requirements: z.object({
        latency: z.enum(["low", "normal"]).default("normal"),
        quality: z.enum(["high", "normal"]).default("normal"),
        cost: z.enum(["minimize", "normal"]).default("normal"),
        privacy: z.enum(["critical", "normal"]).default("normal"),
      }).optional().describe(
        "Optional constraints: latency ('low'|'normal'), quality ('high'|'normal'), " +
        "cost ('minimize'|'normal'), privacy ('critical'|'normal').",
      ),
    },
    async ({ task_description, requirements }) => {
      const reqs = requirements ?? {
        latency: "normal" as const,
        quality: "normal" as const,
        cost: "normal" as const,
        privacy: "normal" as const,
      };

      let fcAvailable = false;
      let fcStatus = "unreachable";
      try {
        const status = await fc.getStatus();
        fcAvailable = status.status === "Available";
        fcStatus = status.status;
      } catch {
        fcAvailable = false;
      }

      const taskClass = classifyTask(task_description);
      const reasoning: string[] = [];
      let recommendation: "local" | "cloud" | "hybrid";

      // Privacy override: always recommend local if privacy is critical
      if (reqs.privacy === "critical") {
        reasoning.push("Privacy is critical. Data must stay local.");
        recommendation = "local";
        if (fcAvailable) {
          reasoning.push("FreeCycle is available. Recommending local execution.");
        } else {
          reasoning.push(
            `FreeCycle is ${fcStatus}. Local execution recommended but may need to wait for GPU availability.`,
          );
        }
        return jsonResult({ recommendation, reasoning, freecycle_status: fcStatus });
      }

      // Low latency with available GPU: prefer local
      if (reqs.latency === "low" && fcAvailable && taskClass !== "cloud") {
        recommendation = "local";
        reasoning.push("Low latency requested and FreeCycle is available.");
        reasoning.push("Task is suitable for local execution.");
        return jsonResult({ recommendation, reasoning, freecycle_status: fcStatus });
      }

      // High quality complex tasks: prefer cloud
      if (reqs.quality === "high" && taskClass === "cloud") {
        recommendation = "cloud";
        reasoning.push("Task requires high quality reasoning. Cloud models excel here.");
        return jsonResult({ recommendation, reasoning, freecycle_status: fcStatus });
      }

      // Cost minimization: prefer local if available
      if (reqs.cost === "minimize" && fcAvailable && taskClass !== "cloud") {
        recommendation = "local";
        reasoning.push("Cost minimization requested. Local inference is free.");
        return jsonResult({ recommendation, reasoning, freecycle_status: fcStatus });
      }

      // General routing based on task classification and availability
      if (!fcAvailable) {
        reasoning.push(`FreeCycle is ${fcStatus}. Local GPU is not available.`);
        recommendation = "cloud";
        reasoning.push("Recommending cloud since local GPU is unavailable.");
      } else if (taskClass === "local") {
        recommendation = "local";
        reasoning.push("Task is well suited for local 8B models (summarization, embedding, classification, etc.).");
        reasoning.push("FreeCycle is available. Recommending local execution.");
      } else if (taskClass === "cloud") {
        recommendation = "cloud";
        reasoning.push("Task requires advanced reasoning beyond local model capabilities.");
      } else {
        recommendation = "hybrid";
        reasoning.push(
          "Task has mixed complexity. Recommend splitting: use local Ollama for " +
          "simple subtasks (summarization, embedding) and cloud for complex reasoning.",
        );
      }

      return jsonResult({ recommendation, reasoning, freecycle_status: fcStatus });
    },
  );

  server.tool(
    "freecycle_benchmark",
    "Run a simple benchmark against a local Ollama model. Sends the same prompt " +
    "multiple times and reports average latency and tokens per second. Useful for " +
    "gauging local inference performance before committing to a workflow.",
    {
      model: z.string().min(1).describe("Model to benchmark."),
      prompt: z.string().min(1).describe("Prompt to use for each iteration."),
      iterations: z.number().int().min(1).max(10).default(3).describe("Number of iterations (default 3, max 10)."),
    },
    async ({ model, prompt, iterations }) => {
      const results: { latency_ms: number; tokens: number; tokens_per_sec: number }[] = [];

      for (let i = 0; i < iterations; i++) {
        const start = Date.now();
        const res = await ollama.generate(model, prompt, { num_predict: 100 });
        const latency = Date.now() - start;
        const evalCount = res.eval_count ?? 0;
        const evalDurationNs = res.eval_duration ?? 1;
        const tps = evalCount / (evalDurationNs / 1e9);
        results.push({
          latency_ms: latency,
          tokens: evalCount,
          tokens_per_sec: Math.round(tps * 100) / 100,
        });
      }

      const avgLatency =
        Math.round(results.reduce((s, r) => s + r.latency_ms, 0) / results.length);
      const avgTps =
        Math.round(
          (results.reduce((s, r) => s + r.tokens_per_sec, 0) / results.length) * 100,
        ) / 100;

      return jsonResult({
        model,
        iterations: results.length,
        results,
        average_latency_ms: avgLatency,
        average_tokens_per_second: avgTps,
      });
    },
  );
}
