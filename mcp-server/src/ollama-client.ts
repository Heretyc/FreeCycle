/**
 * HTTP client for the Ollama REST API (default port 11434).
 *
 * Provides methods for text generation, chat, embeddings, model listing,
 * model details, and model pulling. Uses native fetch (Node 18+).
 * Generate and chat requests use a 5 minute timeout. All others use 30 seconds.
 */

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

function resolveBase(): string {
  const host = process.env.OLLAMA_HOST ?? "localhost";
  const port = process.env.OLLAMA_PORT ?? "11434";
  return `http://${host}:${port}`;
}

async function request<T>(
  url: string,
  init?: RequestInit,
  timeoutMs = 30_000,
): Promise<T> {
  const controller = new AbortController();
  const timeout = setTimeout(() => controller.abort(), timeoutMs);
  try {
    const res = await fetch(url, { ...init, signal: controller.signal });
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

const INFERENCE_TIMEOUT = 5 * 60 * 1000; // 5 minutes

/** Send a text generation request (non-streaming). */
export async function generate(
  model: string,
  prompt: string,
  options?: { system?: string; temperature?: number; num_predict?: number },
  baseUrl?: string,
): Promise<GenerateResponse> {
  const base = baseUrl ?? resolveBase();
  const body: GenerateRequest = {
    model,
    prompt,
    stream: false,
    ...(options?.system ? { system: options.system } : {}),
    options: {
      ...(options?.temperature != null ? { temperature: options.temperature } : {}),
      ...(options?.num_predict != null ? { num_predict: options.num_predict } : {}),
    },
  };
  return request<GenerateResponse>(
    `${base}/api/generate`,
    {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify(body),
    },
    INFERENCE_TIMEOUT,
  );
}

/** Send a chat completion request (non-streaming). */
export async function chat(
  model: string,
  messages: ChatMessage[],
  options?: { system?: string; temperature?: number; num_predict?: number },
  baseUrl?: string,
): Promise<ChatResponse> {
  const base = baseUrl ?? resolveBase();
  const allMessages: ChatMessage[] = [];
  if (options?.system) {
    allMessages.push({ role: "system", content: options.system });
  }
  allMessages.push(...messages);
  const body: ChatRequest = {
    model,
    messages: allMessages,
    stream: false,
    options: {
      ...(options?.temperature != null ? { temperature: options.temperature } : {}),
      ...(options?.num_predict != null ? { num_predict: options.num_predict } : {}),
    },
  };
  return request<ChatResponse>(
    `${base}/api/chat`,
    {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify(body),
    },
    INFERENCE_TIMEOUT,
  );
}

/** Generate embeddings for one or more inputs. */
export async function embed(
  model: string,
  input: string | string[],
  baseUrl?: string,
): Promise<EmbedResponse> {
  const base = baseUrl ?? resolveBase();
  const body: EmbedRequest = { model, input };
  return request<EmbedResponse>(
    `${base}/api/embed`,
    {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify(body),
    },
    INFERENCE_TIMEOUT,
  );
}

/** List all locally available models. */
export async function listModels(baseUrl?: string): Promise<ListModelsResponse> {
  const base = baseUrl ?? resolveBase();
  return request<ListModelsResponse>(`${base}/api/tags`);
}

/** Get detailed information about a specific model. */
export async function showModel(
  name: string,
  baseUrl?: string,
): Promise<ShowModelResponse> {
  const base = baseUrl ?? resolveBase();
  return request<ShowModelResponse>(`${base}/api/show`, {
    method: "POST",
    headers: { "Content-Type": "application/json" },
    body: JSON.stringify({ name }),
  });
}

/** Pull (download) a model. Non-streaming; waits for completion. */
export async function pullModel(
  name: string,
  baseUrl?: string,
): Promise<PullResponse> {
  const base = baseUrl ?? resolveBase();
  return request<PullResponse>(
    `${base}/api/pull`,
    {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify({ name, stream: false }),
    },
    10 * 60 * 1000, // 10 minute timeout for large model downloads
  );
}
