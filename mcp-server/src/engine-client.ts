/**
 * HTTP client for the local inference engine REST API (default port 11434).
 *
 * Provides methods for text generation, chat, embeddings, model listing,
 * model details, and model pulling. Uses native fetch (Node 18+) with secure client support.
 * In secure mode, routes all calls through FreeCycle proxy. In compatibility mode, uses direct engine.
 * Generate and chat requests use a 5 minute timeout. All others use 30 seconds.
 */

import { getConfig, getActiveServer, type ServerEntry } from "./config.js";
import { secureFetch } from "./secure-client.js";

export interface GenerateRequest {
  model: string;
  prompt: string;
  system?: string;
  options?: Record<string, unknown>;
  stream?: boolean;
}

export interface GenerateResponse {
  model: string;
  created_at: string;
  response: string;
  done: boolean;
  total_duration?: number;
  load_duration?: number;
  prompt_eval_count?: number;
  prompt_eval_duration?: number;
  eval_count?: number;
  eval_duration?: number;
}

export interface ChatMessage {
  role: "system" | "user" | "assistant";
  content: string;
}

export interface ChatRequest {
  model: string;
  messages: ChatMessage[];
  options?: Record<string, unknown>;
  stream?: boolean;
}

export interface ChatResponse {
  model: string;
  created_at: string;
  message: ChatMessage;
  done: boolean;
  total_duration?: number;
  load_duration?: number;
  prompt_eval_count?: number;
  prompt_eval_duration?: number;
  eval_count?: number;
  eval_duration?: number;
}

export interface EmbedRequest {
  model: string;
  input: string | string[];
}

export interface EmbedResponse {
  model: string;
  embeddings: number[][];
  total_duration?: number;
  load_duration?: number;
  prompt_eval_count?: number;
}

export interface LegacyEmbedRequest {
  model: string;
  prompt: string;
}

export interface LegacyEmbedResponse {
  embedding: number[];
}

export interface ModelInfo {
  name: string;
  model: string;
  modified_at: string;
  size: number;
  digest: string;
  details: Record<string, unknown>;
}

export interface ListModelsResponse {
  models: ModelInfo[];
}

export interface ShowModelResponse {
  modelfile: string;
  parameters: string;
  template: string;
  details: Record<string, unknown>;
  model_info: Record<string, unknown>;
}

export interface PullResponse {
  status: string;
  digest?: string;
  total?: number;
  completed?: number;
}

export interface DeleteModelResponse {
  status: string;
}

export interface CopyModelResponse {
  status: string;
}

export interface RunningModel {
  name: string;
  model: string;
  size: number;
  size_vram: number;
  digest: string;
  details?: Record<string, unknown>;
  expires_at?: string;
  modified_at?: string;
}

export interface ListRunningResponse {
  models: RunningModel[];
}

export interface EngineVersionResponse {
  version: string;
}

let activeServer: ServerEntry | undefined = undefined;

export function setActiveServer(server: ServerEntry | undefined): void {
  activeServer = server;
}

export function resolveBase(server?: ServerEntry): string {
  const config = getConfig();
  const resolvedServer = server ?? activeServer ?? getActiveServer();

  // Route through FreeCycle proxy when a server entry exists and its port
  // differs from the direct engine port (i.e., it's a FreeCycle server).
  // Previously this was gated on tls_fingerprint presence, which caused
  // secure-mode setups without a stored fingerprint to silently bypass
  // FreeCycle and talk directly to the engine.
  if (
    resolvedServer &&
    resolvedServer.port !== config.engine.port
  ) {
    // Use HTTPS for FreeCycle servers; secureFetch handles self-signed certs
    return `https://${resolvedServer.host}:${resolvedServer.port}`;
  }

  // Direct engine connection (no FreeCycle server configured, or port matches engine)
  return `http://${config.engine.host}:${config.engine.port}`;
}

/** Handle fetch errors consistently: timeout detection, method wrapping, and re-throw. */
function handleFetchError(
  err: unknown,
  url: string,
  method: string,
  timeoutMs: number,
): never {
  if (err instanceof Error && err.name === "AbortError") {
    throw new Error(
      `Request to ${url} timed out after ${Math.round(timeoutMs / 1000)} seconds`,
    );
  }
  if (err instanceof Error && !err.message.includes("HTTP")) {
    throw new Error(`${method} ${url}: ${err.message}`);
  }
  throw err;
}

async function requestJson<T>(
  url: string,
  init?: RequestInit,
  timeoutMs = getConfig().timeouts.requestSecs * 1000,
  server?: ServerEntry,
): Promise<T> {
  const controller = new AbortController();
  const timeout = setTimeout(() => controller.abort(), timeoutMs);
  const method = init?.method ?? "GET";
  try {
    const resolvedServer = server ?? activeServer ?? getActiveServer();
    const res = await secureFetch(url, resolvedServer, { ...init, signal: controller.signal });
    const body = await res.text();
    let parsed: T;
    try {
      parsed = JSON.parse(body) as T;
    } catch {
      throw new Error(`Non-JSON response from ${url}: ${body.slice(0, 300)}`);
    }
    if (!res.ok) {
      const msg = (parsed as Record<string, unknown>)?.error ?? body.slice(0, 300);
      throw new Error(`HTTP ${res.status} from ${url}: ${msg}`);
    }
    return parsed;
  } catch (err: unknown) {
    handleFetchError(err, url, method, timeoutMs);
  } finally {
    clearTimeout(timeout);
  }
}

async function requestText(
  url: string,
  init?: RequestInit,
  timeoutMs = getConfig().timeouts.requestSecs * 1000,
  server?: ServerEntry,
): Promise<string> {
  const controller = new AbortController();
  const timeout = setTimeout(() => controller.abort(), timeoutMs);
  const method = init?.method ?? "GET";
  try {
    const resolvedServer = server ?? activeServer ?? getActiveServer();
    const res = await secureFetch(url, resolvedServer, { ...init, signal: controller.signal });
    const body = await res.text();
    if (!res.ok) {
      // Try JSON error extraction first
      let errorMsg = body.slice(0, 300);
      try {
        const parsed = JSON.parse(body) as Record<string, unknown>;
        if (typeof parsed.error === "string") {
          errorMsg = parsed.error;
        }
      } catch {
        // Fall back to raw body slice
      }
      throw new Error(`HTTP ${res.status} from ${url}: ${errorMsg}`);
    }

    return body;
  } catch (err: unknown) {
    handleFetchError(err, url, method, timeoutMs);
  } finally {
    clearTimeout(timeout);
  }
}

/**
 * Quick connectivity check against GET /api/version.
 *
 * Uses /api/version rather than GET / because the FreeCycle proxy (port 7443)
 * only exposes /api/* endpoints for engine traffic. GET / returns 404 through
 * the proxy, whereas /api/version is a lightweight probe that works both when
 * connecting directly to the engine and when routing through the FreeCycle proxy.
 */
export async function healthCheck(baseUrl?: string, server?: ServerEntry): Promise<string> {
  const base = baseUrl ?? resolveBase(server);
  return requestText(`${base}/api/version`, undefined, undefined, server);
}

/** Send a text generation request (non-streaming). */
export async function generate(
  model: string,
  prompt: string,
  options?: { system?: string; temperature?: number; num_predict?: number },
  baseUrl?: string,
  server?: ServerEntry,
): Promise<GenerateResponse> {
  const base = baseUrl ?? resolveBase(server);
  const body: GenerateRequest = {
    model,
    prompt,
    stream: false,
    ...(options?.system ? { system: options.system } : {}),
  };
  const requestOptions: Record<string, unknown> = {};
  if (options?.temperature != null) {
    requestOptions.temperature = options.temperature;
  }
  if (options?.num_predict != null) {
    requestOptions.num_predict = options.num_predict;
  }
  if (Object.keys(requestOptions).length > 0) {
    body.options = requestOptions;
  }
  return requestJson<GenerateResponse>(
    `${base}/api/generate`,
    {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify(body),
    },
    getConfig().timeouts.inferenceSecs * 1000,
    server,
  );
}

/** Send a chat completion request (non-streaming). */
export async function chat(
  model: string,
  messages: ChatMessage[],
  options?: { system?: string; temperature?: number; num_predict?: number },
  baseUrl?: string,
  server?: ServerEntry,
): Promise<ChatResponse> {
  const base = baseUrl ?? resolveBase(server);
  const allMessages: ChatMessage[] = [];
  if (options?.system) {
    allMessages.push({ role: "system", content: options.system });
  }
  allMessages.push(...messages);
  const body: ChatRequest = {
    model,
    messages: allMessages,
    stream: false,
  };
  const requestOptions: Record<string, unknown> = {};
  if (options?.temperature != null) {
    requestOptions.temperature = options.temperature;
  }
  if (options?.num_predict != null) {
    requestOptions.num_predict = options.num_predict;
  }
  if (Object.keys(requestOptions).length > 0) {
    body.options = requestOptions;
  }
  return requestJson<ChatResponse>(
    `${base}/api/chat`,
    {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify(body),
    },
    getConfig().timeouts.inferenceSecs * 1000,
    server,
  );
}

/** Generate embeddings for one or more inputs. */
export async function embed(
  model: string,
  input: string | string[],
  baseUrl?: string,
  server?: ServerEntry,
): Promise<EmbedResponse> {
  const base = baseUrl ?? resolveBase(server);
  const body: EmbedRequest = { model, input };
  return requestJson<EmbedResponse>(
    `${base}/api/embed`,
    {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify(body),
    },
    getConfig().timeouts.inferenceSecs * 1000,
    server,
  );
}

/** Generate embeddings using the legacy /api/embeddings endpoint (single string input). */
export async function legacyEmbed(
  model: string,
  prompt: string,
  baseUrl?: string,
  server?: ServerEntry,
): Promise<LegacyEmbedResponse> {
  const base = baseUrl ?? resolveBase(server);
  const body: LegacyEmbedRequest = { model, prompt };
  return requestJson<LegacyEmbedResponse>(
    `${base}/api/embeddings`,
    {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify(body),
    },
    getConfig().timeouts.inferenceSecs * 1000,
    server,
  );
}

/** Get the engine version. */
export async function getVersion(baseUrl?: string, server?: ServerEntry): Promise<EngineVersionResponse> {
  const base = baseUrl ?? resolveBase(server);
  return requestJson<EngineVersionResponse>(`${base}/api/version`, undefined, undefined, server);
}

/** List all locally available models. */
export async function listModels(baseUrl?: string | ServerEntry, server?: ServerEntry): Promise<ListModelsResponse> {
  const resolvedServer = typeof baseUrl === "object" ? baseUrl : server;
  const base = typeof baseUrl === "string" ? baseUrl : resolveBase(resolvedServer);
  return requestJson<ListModelsResponse>(`${base}/api/tags`, undefined, undefined, resolvedServer);
}

/** Get detailed information about a specific model. */
export async function showModel(
  name: string,
  baseUrl?: string,
  server?: ServerEntry,
): Promise<ShowModelResponse> {
  const base = baseUrl ?? resolveBase(server);
  return requestJson<ShowModelResponse>(`${base}/api/show`, {
    method: "POST",
    headers: { "Content-Type": "application/json" },
    body: JSON.stringify({ model: name }),
  }, undefined, server);
}

/**
 * Pull (download) a model. Non-streaming; waits for completion.
 *
 * Note: Large models (>20 GB) may exceed the default 10-minute timeout.
 * Consider increasing `timeouts.pullSecs` in the MCP config for very large models.
 */
export async function pullModel(
  name: string,
  baseUrl?: string,
  server?: ServerEntry,
): Promise<PullResponse> {
  const base = baseUrl ?? resolveBase(server);
  return requestJson<PullResponse>(
    `${base}/api/pull`,
    {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify({ name, stream: false }),
    },
    getConfig().timeouts.pullSecs * 1000,
    server,
  );
}

/** Delete a model by name. */
export async function deleteModel(
  name: string,
  baseUrl?: string,
  server?: ServerEntry,
): Promise<DeleteModelResponse> {
  const base = baseUrl ?? resolveBase(server);
  return requestJson<DeleteModelResponse>(`${base}/api/delete`, {
    method: "DELETE",
    headers: { "Content-Type": "application/json" },
    body: JSON.stringify({ name }),
  }, undefined, server);
}

/** Copy (rename) a model from source to destination name. */
export async function copyModel(
  source: string,
  destination: string,
  baseUrl?: string,
  server?: ServerEntry,
): Promise<CopyModelResponse> {
  const base = baseUrl ?? resolveBase(server);
  return requestJson<CopyModelResponse>(`${base}/api/copy`, {
    method: "POST",
    headers: { "Content-Type": "application/json" },
    body: JSON.stringify({ source, destination }),
  }, undefined, server);
}

/** List all models currently loaded in memory. */
export async function listRunning(
  baseUrl?: string,
  server?: ServerEntry,
): Promise<ListRunningResponse> {
  const base = baseUrl ?? resolveBase(server);
  return requestJson<ListRunningResponse>(`${base}/api/ps`, undefined, undefined, server);
}
