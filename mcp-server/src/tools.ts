/**
 * MCP tool definitions for FreeCycle.
 *
 * Registers 15 tools covering status, lifecycle, model management,
 * inference, task evaluation, benchmarking, and multi-server routing.
 */

import { McpServer } from "@modelcontextprotocol/sdk/server/mcp.js";
import { z } from "zod";
import {
  createCloudFallbackPayload,
  ensureLocalAvailability,
  type LocalAvailability,
} from "./availability.js";
import * as fc from "./freecycle-client.js";
import * as engine from "./engine-client.js";
import * as config from "./config.js";
import * as router from "./router.js";
import {
  runTrackedLocalOperation,
  type TrackedLocalOperationOptions,
} from "./task-signaling.js";
import {
  extractServerFingerprint,
  secureFetch,
} from "./secure-client.js";

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

export function safeTokensPerSecond(evalCount?: number, evalDurationNs?: number): number {
  if ((evalCount ?? 0) <= 0 || (evalDurationNs ?? 0) <= 0) {
    return 0;
  }

  return Math.round((evalCount! / (evalDurationNs! / 1e9)) * 100) / 100;
}

export function filterWarmResults<T extends { load_duration_ms: number }>(results: T[]): T[] {
  return results.filter((result) => result.load_duration_ms < 500);
}

async function getStatusPayload(action: string) {
  const best = await router.selectBestServer();
  const selectedServer = best?.server;
  const cfg = config.getConfig();

  const availability = await ensureLocalAvailability();
  if (availability.available) {
    try {
      const status = await fc.getStatus(selectedServer);
      const statusObj = status as unknown as Record<string, unknown>;

      // Add server metadata if multi-server config
      if (cfg.servers && cfg.servers.length > 1 && selectedServer) {
        statusObj.server_selected = `${selectedServer.host}:${selectedServer.port}`;
        statusObj.server_name = selectedServer.name ?? `${selectedServer.host}:${selectedServer.port}`;
      }

      return jsonResult(statusObj);
    } catch (fcError: unknown) {
      // Engine is reachable but FreeCycle agent server is not
      return jsonResult({
        ok: false,
        action,
        engine_reachable: true,
        freecycle_reachable: false,
        message: `Engine is responding but the FreeCycle agent server is unreachable: ${fcError instanceof Error ? fcError.message : String(fcError)}`,
      });
    }
  }

  if (availability.freecycleReachable) {
    const status = await fc.getStatus(selectedServer);
    const resultObj: Record<string, unknown> = {
      ...createCloudFallbackPayload(action, availability),
      status,
    };

    // Add server metadata if multi-server config
    if (cfg.servers && cfg.servers.length > 1 && selectedServer) {
      resultObj.server_selected = `${selectedServer.host}:${selectedServer.port}`;
      resultObj.server_name = selectedServer.name ?? `${selectedServer.host}:${selectedServer.port}`;
    }

    return jsonResult(resultObj);
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

export function classifyTask(desc: string): "local" | "cloud" | "hybrid" {
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
    "Get the complete FreeCycle status. If the local inference engine is not responding and wake-on-LAN is enabled in the MCP config, the server silently wakes the FreeCycle host and waits for it before reporting status.",
    {},
    async () => getStatusPayload("freecycle_status"),
  );

  server.tool(
    "freecycle_health",
    "Quick health check for local inference readiness. Uses the same wake-on-LAN preflight as the other local tools when it is enabled in the MCP config.",
    {},
    async () => {
      const best = await router.selectBestServer();
      const selectedServer = best?.server;

      const availability = await ensureLocalAvailability();
      if (!availability.available) {
        return jsonResult(createCloudFallbackPayload("freecycle_health", availability));
      }

      let freecycleHealth: unknown;
      let freecycleReachable = true;
      try {
        freecycleHealth = await fc.healthCheck(selectedServer);
      } catch (fcError: unknown) {
        freecycleReachable = false;
        freecycleHealth = {
          ok: false,
          message: fcError instanceof Error ? fcError.message : String(fcError),
        };
      }

      const engineHealth = await engine.healthCheck(undefined, selectedServer);
      const result: Record<string, unknown> = {
        ok: freecycleReachable,
        freecycle: freecycleHealth,
        freecycle_reachable: freecycleReachable,
        engine: engineHealth,
      };

      // Add server metadata if multi-server config
      const cfg = config.getConfig();
      if (cfg.servers && cfg.servers.length > 1 && selectedServer) {
        result.server_selected = `${selectedServer.host}:${selectedServer.port}`;
        result.server_name = selectedServer.name ?? `${selectedServer.host}:${selectedServer.port}`;
      }

      return jsonResult(result);
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
      const best = await router.selectBestServer();
      const selectedServer = best?.server;

      const readiness = await prepareLocalTool("freecycle_start_task");
      if (!readiness.available) {
        return readiness.result;
      }

      const result = await fc.startTask(task_id, description, selectedServer);
      const resultObj = result as unknown as Record<string, unknown>;

      // Add server metadata if multi-server config
      const cfg = config.getConfig();
      if (cfg.servers && cfg.servers.length > 1 && selectedServer) {
        resultObj.server_selected = `${selectedServer.host}:${selectedServer.port}`;
        resultObj.server_name = selectedServer.name ?? `${selectedServer.host}:${selectedServer.port}`;
      }

      return jsonResult(resultObj);
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
      const best = await router.selectBestServer();
      const selectedServer = best?.server;

      const readiness = await prepareLocalTool("freecycle_stop_task");
      if (!readiness.available) {
        return readiness.result;
      }

      const result = await fc.stopTask(task_id, selectedServer);
      const resultObj = result as unknown as Record<string, unknown>;

      // Add server metadata if multi-server config
      const cfg = config.getConfig();
      if (cfg.servers && cfg.servers.length > 1 && selectedServer) {
        resultObj.server_selected = `${selectedServer.host}:${selectedServer.port}`;
        resultObj.server_name = selectedServer.name ?? `${selectedServer.host}:${selectedServer.port}`;
      }

      return jsonResult(resultObj);
    },
  );

  server.tool(
    "freecycle_check_availability",
    "Check whether local FreeCycle inference engine is available right now. If wake-on-LAN is enabled, the server wakes the remote FreeCycle host and waits up to the configured maximum before declaring it unavailable.",
    {},
    async () => {
      const best = await router.selectBestServer();
      const selectedServer = best?.server;

      const availability = await ensureLocalAvailability();
      if (!availability.available) {
        if (availability.freecycleReachable) {
          const status = await fc.getStatus(selectedServer);
          const result: Record<string, unknown> = {
            ...createCloudFallbackPayload("freecycle_check_availability", availability),
            vram_percent: status.vram_percent,
            status: status.status,
          };

          const cfg = config.getConfig();
          if (cfg.servers && cfg.servers.length > 1 && selectedServer) {
            result.server_selected = `${selectedServer.host}:${selectedServer.port}`;
            result.server_name = selectedServer.name ?? `${selectedServer.host}:${selectedServer.port}`;
          }

          return jsonResult(result);
        }

        return jsonResult(
          createCloudFallbackPayload("freecycle_check_availability", availability),
        );
      }

      try {
        const status = await fc.getStatus(selectedServer);
        const result: Record<string, unknown> = {
          available: true,
          status: status.status,
          engine_running: status.ollama_running,
          vram_percent: status.vram_percent,
          blocking_processes: status.blocking_processes,
          wake_on_lan_enabled: availability.wakeOnLanEnabled,
          wake_on_lan_attempted: availability.wakeOnLanAttempted,
        };

        const cfg = config.getConfig();
        if (cfg.servers && cfg.servers.length > 1 && selectedServer) {
          result.server_selected = `${selectedServer.host}:${selectedServer.port}`;
          result.server_name = selectedServer.name ?? `${selectedServer.host}:${selectedServer.port}`;
        }

        return jsonResult(result);
      } catch (fcError: unknown) {
        // Engine is reachable but FreeCycle agent server is not
        return jsonResult({
          available: true,
          engine_reachable: true,
          freecycle_reachable: false,
          wake_on_lan_enabled: availability.wakeOnLanEnabled,
          wake_on_lan_attempted: availability.wakeOnLanAttempted,
          message: `Engine is responding but the FreeCycle agent server is unreachable: ${fcError instanceof Error ? fcError.message : String(fcError)}`,
        });
      }
    },
  );

  server.tool(
    "freecycle_list_models",
    "List all models currently downloaded on the local inference engine. Uses the shared wake-on-LAN readiness flow before hitting the engine.",
    {},
    async () => {
      const best = await router.selectBestServer();
      const selectedServer = best?.server;

      const readiness = await prepareLocalTool("freecycle_list_models");
      if (!readiness.available) {
        return readiness.result;
      }

      const response = await engine.listModels(selectedServer);
      const models = response.models.map((model) => ({
        name: model.name,
        size_mb: Math.round(model.size / (1024 * 1024)),
        modified_at: model.modified_at,
        digest: model.digest.slice(0, 12),
      }));

      const result: Record<string, unknown> = { count: models.length, models };

      const cfg = config.getConfig();
      if (cfg.servers && cfg.servers.length > 1 && selectedServer) {
        result.server_selected = `${selectedServer.host}:${selectedServer.port}`;
        result.server_name = selectedServer.name ?? `${selectedServer.host}:${selectedServer.port}`;
      }

      return jsonResult(result);
    },
  );

  server.tool(
    "freecycle_show_model",
    "Get detailed information about a specific model after the local server is confirmed awake and reachable.",
    {
      model_name: z
        .string()
        .min(1)
        .describe("Name of the model to inspect, for example llama3.1:8b-instruct-q4_K_M."),
    },
    async ({ model_name }) => {
      const best = await router.selectBestServer();
      const selectedServer = best?.server;

      const readiness = await prepareLocalTool("freecycle_show_model");
      if (!readiness.available) {
        return readiness.result;
      }

      const result = await engine.showModel(model_name, undefined, selectedServer);
      const resultObj = result as unknown as Record<string, unknown>;

      const cfg = config.getConfig();
      if (cfg.servers && cfg.servers.length > 1 && selectedServer) {
        resultObj.server_selected = `${selectedServer.host}:${selectedServer.port}`;
        resultObj.server_name = selectedServer.name ?? `${selectedServer.host}:${selectedServer.port}`;
      }

      return jsonResult(resultObj);
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
      const best = await router.selectBestServer();
      const selectedServer = best?.server;

      const readiness = await prepareLocalTool("freecycle_pull_model");
      if (!readiness.available) {
        return readiness.result;
      }

      const result = await runTrackedTool(
        readiness,
        {
          action: "freecycle_pull_model",
          operationLabel: "pull",
          modelName: model_name,
          server: selectedServer,
        },
        async () => {
          const response = await fc.installModelDetailed(model_name, selectedServer);
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

      // Invalidate model cache if pull succeeded
      if (selectedServer) {
        const cacheKey = `${selectedServer.host}:${selectedServer.port}`;
        router.clearModelCache(cacheKey);
      }

      return result;
    },
  );

  server.tool(
    "freecycle_generate",
    "Send a text generation request to the local inference engine. The tool first checks engine health, optionally wakes the FreeCycle host, and automatically signals FreeCycle task start and stop around the local job.",
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
      const best = await router.selectBestServer();
      const selectedServer = best?.server;

      const readiness = await prepareLocalTool("freecycle_generate");
      if (!readiness.available) {
        return readiness.result;
      }

      const tracked = await runTrackedTool(
        readiness,
        {
          action: "freecycle_generate",
          operationLabel: "generate",
          modelName: model,
          server: selectedServer,
        },
        async () => {
          const response = await engine.generate(model, prompt, {
            system: system_prompt,
            temperature,
            num_predict: max_tokens,
          }, undefined, selectedServer);

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


      return tracked;
    },
  );

  server.tool(
    "freecycle_chat",
    "Send a multi-turn chat completion request to the local inference engine. The tool uses the same silent wake-and-wait logic as text generation and automatically signals FreeCycle while the local chat request is running.",
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
      const best = await router.selectBestServer();
      const selectedServer = best?.server;

      const readiness = await prepareLocalTool("freecycle_chat");
      if (!readiness.available) {
        return readiness.result;
      }

      const tracked = await runTrackedTool(
        readiness,
        {
          action: "freecycle_chat",
          operationLabel: "chat",
          modelName: model,
          server: selectedServer,
        },
        async () => {
          const response = await engine.chat(model, messages, {
            system: system_prompt,
            temperature,
          }, undefined, selectedServer);

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


      return tracked;
    },
  );

  server.tool(
    "freecycle_embed",
    "Generate vector embeddings using the local inference engine. After the shared readiness check passes, the tool automatically signals FreeCycle task start and stop around the embedding job.",
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
      const best = await router.selectBestServer();
      const selectedServer = best?.server;

      const readiness = await prepareLocalTool("freecycle_embed");
      if (!readiness.available) {
        return readiness.result;
      }

      const tracked = await runTrackedTool(
        readiness,
        {
          action: "freecycle_embed",
          operationLabel: "embed",
          modelName: model,
          server: selectedServer,
        },
        async () => {
          const response = await engine.embed(model, input, undefined, selectedServer);
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


      return tracked;
    },
  );

  server.tool(
    "freecycle_evaluate_task",
    "Evaluate whether a task should run locally, in the cloud, or as a hybrid workflow. The local side of the decision uses the same engine health check and optional wake-on-LAN flow as the executable tools.",
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
        reasoning.push("Low latency is requested and local inference engine is reachable.");
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
    "Run a simple benchmark against a local model. Uses the same wake-on-LAN readiness flow before starting the benchmark and automatically signals FreeCycle once for the full benchmark run.",
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
      const best = await router.selectBestServer();
      const selectedServer = best?.server;

      const readiness = await prepareLocalTool("freecycle_benchmark");
      if (!readiness.available) {
        return readiness.result;
      }

      const tracked = await runTrackedTool(
        readiness,
        {
          action: "freecycle_benchmark",
          operationLabel: "benchmark",
          modelName: model,
          detail: `x${iterations}`,
          server: selectedServer,
        },
        async () => {
          const results: Array<{
            latency_ms: number;
            tokens: number;
            tokens_per_sec: number;
            load_duration_ms: number;
            prompt_tokens: number;
            prompt_eval_ms: number;
            generation_ms: number;
            engine_total_ms: number;
          }> = [];

          for (let index = 0; index < iterations; index += 1) {
            const startedAt = Date.now();
            const response = await engine.generate(model, prompt, {
              num_predict: 100,
            }, undefined, selectedServer);
            results.push({
              latency_ms: Date.now() - startedAt,
              tokens: response.eval_count ?? 0,
              tokens_per_sec: safeTokensPerSecond(
                response.eval_count,
                response.eval_duration,
              ),
              load_duration_ms: Math.round((response.load_duration ?? 0) / 1e6),
              prompt_tokens: response.prompt_eval_count ?? 0,
              prompt_eval_ms: Math.round((response.prompt_eval_duration ?? 0) / 1e6),
              generation_ms: Math.round((response.eval_duration ?? 0) / 1e6),
              engine_total_ms: Math.round((response.total_duration ?? 0) / 1e6),
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

          const warmResults = filterWarmResults(results);
          let warmAverageLatencyMs: number | null = null;
          let warmAverageTokensPerSecond: number | null = null;
          if (warmResults.length > 0) {
            warmAverageLatencyMs = Math.round(
              warmResults.reduce((sum, result) => sum + result.latency_ms, 0) /
                warmResults.length,
            );
            warmAverageTokensPerSecond =
              Math.round(
                (warmResults.reduce((sum, result) => sum + result.tokens_per_sec, 0) /
                  warmResults.length) *
                  100,
              ) / 100;
          }

          return {
            model,
            iterations: results.length,
            results,
            average_latency_ms: averageLatencyMs,
            average_tokens_per_second: averageTokensPerSecond,
            warm_average_latency_ms: warmAverageLatencyMs,
            warm_average_tokens_per_second: warmAverageTokensPerSecond,
            warm_iteration_count: warmResults.length,
          };
        },
      );


      return tracked;
    },
  );

  server.tool(
    "freecycle_add_server",
    "Add a new FreeCycle server to the MCP config. Attempts TLS first, falls back to plaintext (compatibility mode). Stores the server entry with approved=false; user must set approved=true in the config file to connect.",
    {
      ip: z.string().describe("IP address of the FreeCycle server."),
      port: z.number().int().default(7443).describe("Port number (default: 7443)."),
      name: z.string().optional().describe("Optional server name for reference."),
    },
    async ({ ip, port, name }) => {
      try {
        // Try TLS first: extract fingerprint and verify the server responds
        let fingerprint: string | undefined;
        let tlsReachable = false;
        let httpsError: string | undefined;

        try {
          fingerprint = await extractServerFingerprint(ip, port);
          // Verify the server actually responds to a FreeCycle request
          const probeEntry: config.ServerEntry = {
            host: ip,
            port,
            approved: false,
          };
          const baseUrl = `https://${ip}:${port}`;
          const res = await secureFetch(`${baseUrl}/health`, probeEntry);
          if (res.ok) {
            tlsReachable = true;
          }
        } catch (error: unknown) {
          httpsError =
            error instanceof Error ? error.message : String(error);
        }

        if (tlsReachable) {
          const newEntry: config.ServerEntry = {
            host: ip,
            port,
            ...(name ? { name } : {}),
            approved: false,
            tls_fingerprint: fingerprint,
          };

          const currentConfig = config.getConfig();
          const servers = currentConfig.servers ?? [];
          servers.push(newEntry);
          config.saveConfig({ servers });

          return jsonResult({
            ok: true,
            message: `Server added (TLS, fingerprint captured) with approved=false. Edit freecycle-mcp.config.json and set approved=true for '${name ?? ip}:${port}' to enable connections.`,
            server: newEntry,
          });
        }

        // Try HTTP fallback (compatibility mode)
        try {
          const httpUrl = `http://${ip}:${port}/health`;
          const res = await secureFetch(httpUrl);
          if (res.ok) {
            const newEntry: config.ServerEntry = {
              host: ip,
              port,
              ...(name ? { name } : {}),
              approved: false,
            };

            const currentConfig = config.getConfig();
            const servers = currentConfig.servers ?? [];
            servers.push(newEntry);
            config.saveConfig({ servers });

            return jsonResult({
              ok: true,
              message: `Server added in compatibility mode (plaintext) with approved=false. Edit freecycle-mcp.config.json and set approved=true for '${name ?? ip}:${port}' to enable connections.`,
              server: newEntry,
            });
          }
        } catch {
          // Both TLS and plaintext failed
        }

        return jsonResult({
          ok: false,
          message: `Failed to reach FreeCycle server at ${ip}:${port}. ` +
            (httpsError
              ? `HTTPS probe: ${httpsError}. `
              : "") +
            `Verify the IP, port, and network connectivity.`,
        });
      } catch (error: unknown) {
        return jsonResult({
          ok: false,
          message: `Error adding server: ${error instanceof Error ? error.message : String(error)}`,
        });
      }
    },
  );

  server.tool(
    "freecycle_list_servers",
    "List all configured FreeCycle servers with their current status, reachability, VRAM availability, and approval status. Shows both approved and unapproved servers to help users manage multi-server configurations.",
    {},
    async () => {
      const cfg = config.getConfig();
      const probes = await router.queryAllServers();

      // Build response with both approved and unapproved servers
      const servers: Array<Record<string, unknown>> = [];
      const configServers = cfg.servers ?? [];
      for (const server of configServers) {
        const probe = probes.find(
          (p) => p.server.host === server.host && p.server.port === server.port,
        );

        if (probe && probe.reachable) {
          servers.push({
            host: probe.server.host,
            port: probe.server.port,
            name: probe.server.name ?? `${probe.server.host}:${probe.server.port}`,
            approved: probe.server.approved,
            reachable: true,
            status: probe.status.status,
            vram_free_mb: probe.freeVramMb,
            vram_total_mb: probe.status.vram_total_mb,
            vram_percent: probe.status.vram_percent,
            engine_running: probe.status.ollama_running,
          });
        } else {
          servers.push({
            host: server.host,
            port: server.port,
            name: server.name ?? `${server.host}:${server.port}`,
            approved: server.approved,
            reachable: false,
            error: probe ? probe.error : "Not queried",
          });
        }
      }

      const approvedCount = servers.filter((s) => s.approved).length;
      const reachableCount = servers.filter((s) => s.reachable).length;

      return jsonResult({
        servers,
        approved_count: approvedCount,
        reachable_count: reachableCount,
      });
    },
  );

  server.tool(
    "freecycle_model_catalog",
    "Fetch the complete model catalog of available models. Returns metadata including model name, description, parameter sizes, quantization variants, tags, and download counts. Use this to browse available models before requesting installations.",
    {},
    async () => {
      const best = await router.selectBestServer();
      const selectedServer = best?.server;

      try {
        const catalog = await fc.getModelCatalog(selectedServer);
        return jsonResult({
          ok: true,
          catalog,
          total_models: catalog.models.length,
          synthesized: catalog.synthesized,
          scraped_at: catalog.scraped_at,
        });
      } catch (error: unknown) {
        return jsonResult({
          ok: false,
          error: error instanceof Error ? error.message : String(error),
          message: "Failed to fetch model catalog",
        });
      }
    },
  );
}
