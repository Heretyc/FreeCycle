/**
 * MCP tool definitions for FreeCycle and Ollama.
 *
 * Registers 13 tools covering status, lifecycle, model management,
 * inference, task evaluation, and benchmarking.
 */

import { McpServer } from "@modelcontextprotocol/sdk/server/mcp.js";
import { z } from "zod";
import {
  createCloudFallbackPayload,
  ensureLocalAvailability,
  type LocalAvailability,
} from "./availability.js";
import * as fc from "./freecycle-client.js";
import * as ollama from "./ollama-client.js";
import {
  runTrackedLocalOperation,
  type TrackedLocalOperationOptions,
} from "./task-signaling.js";

function textResult(text: string) {
  return { content: [{ type: "text" as const, text }] };
}

function jsonResult(data: unknown) {
  return textResult(JSON.stringify(data, null, 2));
}

type JsonToolResult = ReturnType<typeof jsonResult>;
type ReadyLocalTool = {
  available: true;
  availability: LocalAvailability;
};
type LocalToolReadiness =
  | ReadyLocalTool
  | {
      available: false;
      result: JsonToolResult;
    };

function safeTokensPerSecond(evalCount?: number, evalDurationNs?: number): number {
  if ((evalCount ?? 0) <= 0 || (evalDurationNs ?? 0) <= 0) {
    return 0;
  }

  return Math.round((evalCount! / (evalDurationNs! / 1e9)) * 100) / 100;
}

async function getStatusPayload(action: string) {
  const availability = await ensureLocalAvailability();
  if (availability.available) {
    return jsonResult(await fc.getStatus());
  }

  if (availability.freecycleReachable) {
    const status = await fc.getStatus();
    return jsonResult({
      ...createCloudFallbackPayload(action, availability),
      status,
    });
  }

  return jsonResult(createCloudFallbackPayload(action, availability));
}

async function prepareLocalTool(action: string): Promise<LocalToolReadiness> {
  const availability = await ensureLocalAvailability();
  if (!availability.available) {
    return {
      available: false,
      result: jsonResult(createCloudFallbackPayload(action, availability)),
    };
  }

  return {
    available: true,
    availability,
  };
}

async function runTrackedTool<T>(
  readiness: ReadyLocalTool,
  options: Omit<TrackedLocalOperationOptions, "availability">,
  operation: () => Promise<T>,
): Promise<JsonToolResult> {
  const tracked = await runTrackedLocalOperation(
    {
      ...options,
      availability: readiness.availability,
    },
    operation,
  );

  if (tracked.kind === "unavailable") {
    return jsonResult(tracked.payload);
  }

  return jsonResult(tracked.value);
}

const LOCAL_KEYWORDS = [
  "summarize",
  "summarization",
  "summary",
  "embed",
  "embedding",
  "embeddings",
  "classify",
  "classification",
  "explain",
  "explanation",
  "translate",
  "translation",
  "extract",
  "extraction",
  "sentiment",
  "tag",
  "label",
  "rewrite",
  "paraphrase",
  "simple",
  "basic",
];

const CLOUD_KEYWORDS = [
  "complex code",
  "advanced code",
  "code generation",
  "math proof",
  "theorem",
  "formal verification",
  "research",
  "analysis",
  "deep reasoning",
  "creative writing",
  "novel",
  "story",
  "multi-step reasoning",
  "planning",
  "architecture",
  "system design",
];

function classifyTask(desc: string): "local" | "cloud" | "hybrid" {
  const lower = desc.toLowerCase();
  const localScore = LOCAL_KEYWORDS.filter((keyword) => lower.includes(keyword)).length;
  const cloudScore = CLOUD_KEYWORDS.filter((keyword) => lower.includes(keyword)).length;

  if (cloudScore > localScore) {
    return "cloud";
  }
  if (localScore > 0 && cloudScore === 0) {
    return "local";
  }
  return "hybrid";
}

export function registerTools(server: McpServer): void {
  server.tool(
    "freecycle_status",
    "Get the complete FreeCycle status. If Ollama is not responding and wake-on-LAN is enabled in the MCP config, the server silently wakes the FreeCycle host and waits for it before reporting status.",
    {},
    async () => getStatusPayload("freecycle_status"),
  );

  server.tool(
    "freecycle_health",
    "Quick health check for local inference readiness. Uses the same wake-on-LAN preflight as the other local tools when it is enabled in the MCP config.",
    {},
    async () => {
      const availability = await ensureLocalAvailability();
      if (!availability.available) {
        return jsonResult(createCloudFallbackPayload("freecycle_health", availability));
      }

      const freecycleHealth = await fc.healthCheck();
      const ollamaHealth = await ollama.healthCheck();
      return jsonResult({
        ok: true,
        freecycle: freecycleHealth,
        ollama: ollamaHealth,
      });
    },
  );

  server.tool(
    "freecycle_start_task",
    "Signal that an agent workflow is beginning GPU work. If the FreeCycle machine is asleep and wake-on-LAN is enabled, this tool wakes it first. If local inference stays unavailable, the response suggests routing to cloud tools.",
    {
      task_id: z.string().min(1).describe("Unique identifier for this task."),
      description: z
        .string()
        .min(1)
        .describe("Human readable description of the GPU work being performed."),
    },
    async ({ task_id, description }) => {
      const readiness = await prepareLocalTool("freecycle_start_task");
      if (!readiness.available) {
        return readiness.result;
      }

      return jsonResult(await fc.startTask(task_id, description));
    },
  );

  server.tool(
    "freecycle_stop_task",
    "Signal that an agent workflow has finished GPU work. Uses the same local availability preflight as task start.",
    {
      task_id: z
        .string()
        .min(1)
        .describe("The task_id that was provided when starting the task."),
    },
    async ({ task_id }) => {
      const readiness = await prepareLocalTool("freecycle_stop_task");
      if (!readiness.available) {
        return readiness.result;
      }

      return jsonResult(await fc.stopTask(task_id));
    },
  );

  server.tool(
    "freecycle_check_availability",
    "Check whether local FreeCycle and Ollama inference is available right now. If wake-on-LAN is enabled, the server wakes the remote FreeCycle host and waits up to the configured maximum before declaring it unavailable.",
    {},
    async () => {
      const availability = await ensureLocalAvailability();
      if (!availability.available) {
        if (availability.freecycleReachable) {
          const status = await fc.getStatus();
          return jsonResult({
            ...createCloudFallbackPayload("freecycle_check_availability", availability),
            vram_percent: status.vram_percent,
            status: status.status,
          });
        }

        return jsonResult(
          createCloudFallbackPayload("freecycle_check_availability", availability),
        );
      }

      const status = await fc.getStatus();
      return jsonResult({
        available: true,
        status: status.status,
        ollama_running: status.ollama_running,
        vram_percent: status.vram_percent,
        blocking_processes: status.blocking_processes,
        wake_on_lan_enabled: availability.wakeOnLanEnabled,
        wake_on_lan_attempted: availability.wakeOnLanAttempted,
      });
    },
  );

  server.tool(
    "freecycle_list_models",
    "List all models currently downloaded on the local Ollama instance. Uses the shared wake-on-LAN readiness flow before hitting Ollama.",
    {},
    async () => {
      const readiness = await prepareLocalTool("freecycle_list_models");
      if (!readiness.available) {
        return readiness.result;
      }

      const response = await ollama.listModels();
      const models = response.models.map((model) => ({
        name: model.name,
        size_mb: Math.round(model.size / (1024 * 1024)),
        modified_at: model.modified_at,
        digest: model.digest.slice(0, 12),
      }));

      return jsonResult({ count: models.length, models });
    },
  );

  server.tool(
    "freecycle_show_model",
    "Get detailed information about a specific Ollama model after the local server is confirmed awake and reachable.",
    {
      model_name: z
        .string()
        .min(1)
        .describe("Name of the model to inspect, for example llama3.1:8b-instruct-q4_K_M."),
    },
    async ({ model_name }) => {
      const readiness = await prepareLocalTool("freecycle_show_model");
      if (!readiness.available) {
        return readiness.result;
      }

      return jsonResult(await ollama.showModel(model_name));
    },
  );

  server.tool(
    "freecycle_pull_model",
    "Request download of a new model through FreeCycle's tray-gated install API. The local user must enable remote model installs from the tray menu first; the unlock automatically expires after one hour. Once local readiness succeeds, the tool automatically signals FreeCycle task start and stop around the pull so the tray reflects the active MCP job.",
    {
      model_name: z
        .string()
        .min(1)
        .describe("Name of the model to pull, for example codellama:7b."),
    },
    async ({ model_name }) => {
      const readiness = await prepareLocalTool("freecycle_pull_model");
      if (!readiness.available) {
        return readiness.result;
      }

      return runTrackedTool(
        readiness,
        {
          action: "freecycle_pull_model",
          operationLabel: "pull",
          modelName: model_name,
        },
        async () => {
          const response = await fc.installModelDetailed(model_name);
          if (response.ok) {
            return response.body;
          }

          return {
            ok: false,
            http_status: response.status,
            message: response.body.message,
          };
        },
      );
    },
  );

  server.tool(
    "freecycle_generate",
    "Send a text generation request to the local Ollama instance. The tool first checks Ollama health, optionally wakes the FreeCycle host, and automatically signals FreeCycle task start and stop around the local job.",
    {
      model: z
        .string()
        .default("llama3.1:8b-instruct-q4_K_M")
        .describe("Model name. Defaults to llama3.1:8b-instruct-q4_K_M."),
      prompt: z.string().min(1).describe("The text prompt to complete."),
      system_prompt: z
        .string()
        .optional()
        .describe("Optional system prompt to set context."),
      temperature: z
        .number()
        .min(0)
        .max(2)
        .optional()
        .describe("Sampling temperature from 0.0 to 2.0."),
      max_tokens: z
        .number()
        .int()
        .positive()
        .optional()
        .describe("Maximum tokens to generate."),
    },
    async ({ model, prompt, system_prompt, temperature, max_tokens }) => {
      const readiness = await prepareLocalTool("freecycle_generate");
      if (!readiness.available) {
        return readiness.result;
      }

      return runTrackedTool(
        readiness,
        {
          action: "freecycle_generate",
          operationLabel: "generate",
          modelName: model,
        },
        async () => {
          const response = await ollama.generate(model, prompt, {
            system: system_prompt,
            temperature,
            num_predict: max_tokens,
          });

          return {
            response: response.response,
            model: response.model,
            tokens_generated: response.eval_count ?? 0,
            tokens_per_second: safeTokensPerSecond(
              response.eval_count,
              response.eval_duration,
            ),
            total_duration_ms:
              response.total_duration != null
                ? Math.round(response.total_duration / 1e6)
                : null,
          };
        },
      );
    },
  );

  server.tool(
    "freecycle_chat",
    "Send a multi-turn chat completion request to the local Ollama instance. The tool uses the same silent wake-and-wait logic as text generation and automatically signals FreeCycle while the local chat request is running.",
    {
      model: z
        .string()
        .default("llama3.1:8b-instruct-q4_K_M")
        .describe("Model name. Defaults to llama3.1:8b-instruct-q4_K_M."),
      messages: z
        .array(
          z.object({
            role: z.enum(["system", "user", "assistant"]),
            content: z.string(),
          }),
        )
        .min(1)
        .describe("Array of chat messages with role and content."),
      system_prompt: z
        .string()
        .optional()
        .describe("Optional system prompt prepended to messages."),
      temperature: z
        .number()
        .min(0)
        .max(2)
        .optional()
        .describe("Sampling temperature from 0.0 to 2.0."),
    },
    async ({ model, messages, system_prompt, temperature }) => {
      const readiness = await prepareLocalTool("freecycle_chat");
      if (!readiness.available) {
        return readiness.result;
      }

      return runTrackedTool(
        readiness,
        {
          action: "freecycle_chat",
          operationLabel: "chat",
          modelName: model,
        },
        async () => {
          const response = await ollama.chat(model, messages, {
            system: system_prompt,
            temperature,
          });

          return {
            message: response.message,
            model: response.model,
            tokens_generated: response.eval_count ?? 0,
            tokens_per_second: safeTokensPerSecond(
              response.eval_count,
              response.eval_duration,
            ),
            total_duration_ms:
              response.total_duration != null
                ? Math.round(response.total_duration / 1e6)
                : null,
          };
        },
      );
    },
  );

  server.tool(
    "freecycle_embed",
    "Generate vector embeddings using the local Ollama instance. After the shared readiness check passes, the tool automatically signals FreeCycle task start and stop around the embedding job.",
    {
      model: z
        .string()
        .default("nomic-embed-text")
        .describe("Embedding model name. Defaults to nomic-embed-text."),
      input: z
        .union([z.string().min(1), z.array(z.string().min(1)).min(1)])
        .describe("A single string or array of strings to embed."),
    },
    async ({ model, input }) => {
      const readiness = await prepareLocalTool("freecycle_embed");
      if (!readiness.available) {
        return readiness.result;
      }

      return runTrackedTool(
        readiness,
        {
          action: "freecycle_embed",
          operationLabel: "embed",
          modelName: model,
        },
        async () => {
          const response = await ollama.embed(model, input);
          const dimensions =
            response.embeddings.length > 0 ? response.embeddings[0].length : 0;

          return {
            model: response.model,
            embedding_count: response.embeddings.length,
            dimensions,
            embeddings: response.embeddings,
            total_duration_ms:
              response.total_duration != null
                ? Math.round(response.total_duration / 1e6)
                : null,
          };
        },
      );
    },
  );

  server.tool(
    "freecycle_evaluate_task",
    "Evaluate whether a task should run locally, in the cloud, or as a hybrid workflow. The local side of the decision uses the same Ollama health check and optional wake-on-LAN flow as the executable tools.",
    {
      task_description: z
        .string()
        .min(1)
        .describe("Description of the task to evaluate."),
      requirements: z
        .object({
          latency: z.enum(["low", "normal"]).default("normal"),
          quality: z.enum(["high", "normal"]).default("normal"),
          cost: z.enum(["minimize", "normal"]).default("normal"),
          privacy: z.enum(["critical", "normal"]).default("normal"),
        })
        .optional()
        .describe(
          "Optional constraints: latency, quality, cost, and privacy priorities.",
        ),
    },
    async ({ task_description, requirements }) => {
      const reqs = requirements ?? {
        latency: "normal" as const,
        quality: "normal" as const,
        cost: "normal" as const,
        privacy: "normal" as const,
      };

      const availability = await ensureLocalAvailability();
      const taskClass = classifyTask(task_description);
      const reasoning: string[] = [];
      let recommendation: "local" | "cloud" | "hybrid";

      if (!availability.available) {
        reasoning.push(availability.reason);
      }

      if (reqs.privacy === "critical" && availability.available) {
        recommendation = "local";
        reasoning.push("Privacy is critical and local inference is reachable.");
        return jsonResult({
          recommendation,
          reasoning,
          freecycle_status: availability.freecycleStatus,
          local_available: true,
          wake_on_lan_attempted: availability.wakeOnLanAttempted,
        });
      }

      if (reqs.privacy === "critical" && !availability.available) {
        recommendation = "cloud";
        reasoning.push(
          "Privacy would normally force local execution, but the local server is not available. Route to cloud only if the workflow can tolerate that policy tradeoff.",
        );
        return jsonResult({
          recommendation,
          reasoning,
          freecycle_status: availability.freecycleStatus,
          local_available: false,
          wake_on_lan_attempted: availability.wakeOnLanAttempted,
        });
      }

      if (!availability.available) {
        recommendation = "cloud";
        reasoning.push(
          "Local inference is unavailable, so the default fallback is cloud routing.",
        );
        return jsonResult({
          recommendation,
          reasoning,
          freecycle_status: availability.freecycleStatus,
          local_available: false,
          wake_on_lan_attempted: availability.wakeOnLanAttempted,
        });
      }

      if (reqs.latency === "low" && taskClass !== "cloud") {
        recommendation = "local";
        reasoning.push("Low latency is requested and local Ollama is reachable.");
      } else if (reqs.quality === "high" && taskClass === "cloud") {
        recommendation = "cloud";
        reasoning.push("This task needs higher reasoning quality than the local 8B stack usually provides.");
      } else if (reqs.cost === "minimize" && taskClass !== "cloud") {
        recommendation = "local";
        reasoning.push("Cost minimization favors local inference because the GPU is already available.");
      } else if (taskClass === "local") {
        recommendation = "local";
        reasoning.push("The task matches workloads that fit local models well.");
      } else if (taskClass === "cloud") {
        recommendation = "cloud";
        reasoning.push("The task description suggests advanced reasoning or complex code generation.");
      } else {
        recommendation = "hybrid";
        reasoning.push(
          "The task has mixed complexity. Use local for embeddings or simple summarization, then escalate the harder reasoning steps to cloud.",
        );
      }

      return jsonResult({
        recommendation,
        reasoning,
        freecycle_status: availability.freecycleStatus ?? "Available",
        local_available: true,
        wake_on_lan_attempted: availability.wakeOnLanAttempted,
      });
    },
  );

  server.tool(
    "freecycle_benchmark",
    "Run a simple benchmark against a local Ollama model. Uses the same wake-on-LAN readiness flow before starting the benchmark and automatically signals FreeCycle once for the full benchmark run.",
    {
      model: z.string().min(1).describe("Model to benchmark."),
      prompt: z.string().min(1).describe("Prompt to use for each iteration."),
      iterations: z
        .number()
        .int()
        .min(1)
        .max(10)
        .default(3)
        .describe("Number of iterations. Default 3, max 10."),
    },
    async ({ model, prompt, iterations }) => {
      const readiness = await prepareLocalTool("freecycle_benchmark");
      if (!readiness.available) {
        return readiness.result;
      }

      return runTrackedTool(
        readiness,
        {
          action: "freecycle_benchmark",
          operationLabel: "benchmark",
          modelName: model,
          detail: `x${iterations}`,
        },
        async () => {
          const results: Array<{
            latency_ms: number;
            tokens: number;
            tokens_per_sec: number;
          }> = [];

          for (let index = 0; index < iterations; index += 1) {
            const startedAt = Date.now();
            const response = await ollama.generate(model, prompt, {
              num_predict: 100,
            });
            results.push({
              latency_ms: Date.now() - startedAt,
              tokens: response.eval_count ?? 0,
              tokens_per_sec: safeTokensPerSecond(
                response.eval_count,
                response.eval_duration,
              ),
            });
          }

          const averageLatencyMs = Math.round(
            results.reduce((sum, result) => sum + result.latency_ms, 0) /
              results.length,
          );
          const averageTokensPerSecond =
            Math.round(
              (results.reduce((sum, result) => sum + result.tokens_per_sec, 0) /
                results.length) *
                100,
            ) / 100;

          return {
            model,
            iterations: results.length,
            results,
            average_latency_ms: averageLatencyMs,
            average_tokens_per_second: averageTokensPerSecond,
          };
        },
      );
    },
  );
}
