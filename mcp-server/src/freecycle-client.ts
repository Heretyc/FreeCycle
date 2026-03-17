/**
 * HTTP client for the FreeCycle Agent Signal API (default port 7443).
 *
 * Provides methods for status queries, task lifecycle signaling, tray-gated model installs,
 * and health checks.
 * Uses native fetch (Node 18+) via secureFetch for TLS support. All methods accept an optional baseUrl override.
 */

import { getConfig, getActiveServer, type ServerEntry } from "./config.js";
import { secureFetch } from "./secure-client.js";

/** Shape returned by GET /status. */
export interface FreeCycleStatus {
  status: string;
  ollama_running: boolean;
  vram_used_mb: number;
  vram_total_mb: number;
  vram_percent: number;
  active_task_id: string | null;
  active_task_description: string | null;
  local_ip: string;
  ollama_port: number;
  blocking_processes: string[];
  model_status: string[];
  remote_model_installs_unlocked: boolean;
  remote_model_installs_expires_in_seconds: number | null;
}

/** A single model card in the catalog. */
export interface ModelCard {
  name: string;
  description: string;
  parameter_sizes: string[];
  quantization_variants: string[];
  tags: string[];
  download_count: number | null;
  last_updated: string | null;
}

/** Complete model catalog with metadata. */
export interface ModelCatalog {
  scraped_at: string;
  synthesized: boolean;
  models: ModelCard[];
}

/** Shape returned by POST /task/start and POST /task/stop. */
export interface ApiResponse {
  ok: boolean;
  message: string;
}

export interface JsonHttpResponse<T> {
  status: number;
  ok: boolean;
  body: T;
}

export function resolveBase(server?: ServerEntry): string {
  const entry = server ?? getActiveServer();
  return `https://${entry.host}:${entry.port}`;
}

function extractResponseMessage(parsed: unknown, fallback: string): string {
  if (typeof parsed !== "object" || parsed == null) {
    return fallback;
  }

  const candidate = (parsed as Record<string, unknown>).message;
  if (typeof candidate === "string" && candidate.trim() !== "") {
    return candidate;
  }

  return fallback;
}

async function requestResponse<T>(
  url: string,
  init?: RequestInit,
  timeoutMs = getConfig().timeouts.requestSecs * 1000,
  server?: ServerEntry,
): Promise<JsonHttpResponse<T>> {
  const controller = new AbortController();
  const timeout = setTimeout(() => controller.abort(), timeoutMs);
  try {
    const res = await secureFetch(url, server, { ...init, signal: controller.signal });
    const body = await res.text();
    let parsed: T;
    try {
      parsed = JSON.parse(body) as T;
    } catch {
      throw new Error(`Non-JSON response from ${url}: ${body.slice(0, 200)}`);
    }
    return {
      status: res.status,
      ok: res.ok,
      body: parsed,
    };
  } catch (err: unknown) {
    if (err instanceof DOMException && err.name === "AbortError") {
      throw new Error(
        `Request to ${url} timed out after ${Math.round(timeoutMs / 1000)} seconds`,
      );
    }
    throw err;
  } finally {
    clearTimeout(timeout);
  }
}

async function request<T>(
  url: string,
  init?: RequestInit,
  timeoutMs = getConfig().timeouts.requestSecs * 1000,
  server?: ServerEntry,
): Promise<T> {
  const response = await requestResponse<T>(url, init, timeoutMs, server);
  if (!response.ok) {
    const message = extractResponseMessage(
      response.body,
      JSON.stringify(response.body).slice(0, 200),
    );
    throw new Error(`HTTP ${response.status} from ${url}: ${message}`);
  }

  return response.body;
}

/** Fetch the full FreeCycle status (GPU, VRAM, engine, active tasks, network). */
export async function getStatus(baseUrl?: string | ServerEntry): Promise<FreeCycleStatus> {
  const server = typeof baseUrl === "object" ? baseUrl : undefined;
  const base = typeof baseUrl === "string" ? baseUrl : resolveBase(server);
  return request<FreeCycleStatus>(`${base}/status`, undefined, undefined, server);
}

/** Signal that an agentic workflow is beginning GPU work. */
export async function startTask(
  taskId: string,
  description: string,
  baseUrl?: string | ServerEntry,
): Promise<ApiResponse> {
  const response = await startTaskDetailed(taskId, description, baseUrl);
  if (!response.ok) {
    const server = typeof baseUrl === "object" ? baseUrl : undefined;
    const base = typeof baseUrl === "string" ? baseUrl : resolveBase(server);
    throw new Error(
      `HTTP ${response.status} from ${base}/task/start: ${response.body.message}`,
    );
  }

  return response.body;
}

/** Signal that an agentic workflow is beginning GPU work and inspect the HTTP status. */
export async function startTaskDetailed(
  taskId: string,
  description: string,
  baseUrl?: string | ServerEntry,
): Promise<JsonHttpResponse<ApiResponse>> {
  const server = typeof baseUrl === "object" ? baseUrl : undefined;
  const base = typeof baseUrl === "string" ? baseUrl : resolveBase(server);
  return requestResponse<ApiResponse>(
    `${base}/task/start`,
    {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify({ task_id: taskId, description }),
    },
    undefined,
    server,
  );
}

/** Signal that an agentic workflow has finished GPU work. */
export async function stopTask(
  taskId: string,
  baseUrl?: string | ServerEntry,
): Promise<ApiResponse> {
  const response = await stopTaskDetailed(taskId, baseUrl);
  if (!response.ok) {
    const server = typeof baseUrl === "object" ? baseUrl : undefined;
    const base = typeof baseUrl === "string" ? baseUrl : resolveBase(server);
    throw new Error(
      `HTTP ${response.status} from ${base}/task/stop: ${response.body.message}`,
    );
  }

  return response.body;
}

/** Signal that an agentic workflow has finished GPU work and inspect the HTTP status. */
export async function stopTaskDetailed(
  taskId: string,
  baseUrl?: string | ServerEntry,
): Promise<JsonHttpResponse<ApiResponse>> {
  const server = typeof baseUrl === "object" ? baseUrl : undefined;
  const base = typeof baseUrl === "string" ? baseUrl : resolveBase(server);
  return requestResponse<ApiResponse>(
    `${base}/task/stop`,
    {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify({ task_id: taskId }),
    },
    undefined,
    server,
  );
}

/** Quick connectivity check. Returns the ApiResponse from GET /health. */
export async function healthCheck(baseUrl?: string | ServerEntry): Promise<ApiResponse> {
  const server = typeof baseUrl === "object" ? baseUrl : undefined;
  const base = typeof baseUrl === "string" ? baseUrl : resolveBase(server);
  return request<ApiResponse>(`${base}/health`, undefined, undefined, server);
}

/** Request a model install through FreeCycle's tray-gated API. */
export async function installModel(
  modelName: string,
  baseUrl?: string | ServerEntry,
): Promise<ApiResponse> {
  const response = await installModelDetailed(modelName, baseUrl);
  if (!response.ok) {
    const server = typeof baseUrl === "object" ? baseUrl : undefined;
    const base = typeof baseUrl === "string" ? baseUrl : resolveBase(server);
    throw new Error(
      `HTTP ${response.status} from ${base}/models/install: ${response.body.message}`,
    );
  }

  return response.body;
}

/** Request a model install and inspect the HTTP status. */
export async function installModelDetailed(
  modelName: string,
  baseUrl?: string | ServerEntry,
): Promise<JsonHttpResponse<ApiResponse>> {
  const server = typeof baseUrl === "object" ? baseUrl : undefined;
  const base = typeof baseUrl === "string" ? baseUrl : resolveBase(server);
  return requestResponse<ApiResponse>(
    `${base}/models/install`,
    {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify({ model_name: modelName }),
    },
    getConfig().timeouts.pullSecs * 1000,
    server,
  );
}

/** Fetch the model catalog (scraped models from ollama.com/library). */
export async function getModelCatalog(server?: ServerEntry): Promise<ModelCatalog> {
  const base = resolveBase(server);
  return request<ModelCatalog>(`${base}/models/catalog`, undefined, undefined, server);
}
