/**
 * HTTP client for the FreeCycle Agent Signal API (default port 7443).
 *
 * Provides methods for status queries, task lifecycle signaling, and health checks.
 * Uses native fetch (Node 18+). All methods accept an optional baseUrl override.
 */

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
}

/** Shape returned by POST /task/start and POST /task/stop. */
export interface ApiResponse {
  ok: boolean;
  message: string;
}

function resolveBase(): string {
  const host = process.env.FREECYCLE_HOST ?? "localhost";
  const port = process.env.FREECYCLE_PORT ?? "7443";
  return `http://${host}:${port}`;
}

async function request<T>(url: string, init?: RequestInit): Promise<T> {
  const controller = new AbortController();
  const timeout = setTimeout(() => controller.abort(), 10_000);
  try {
    const res = await fetch(url, { ...init, signal: controller.signal });
    const body = await res.text();
    let parsed: T;
    try {
      parsed = JSON.parse(body) as T;
    } catch {
      throw new Error(`Non-JSON response from ${url}: ${body.slice(0, 200)}`);
    }
    if (!res.ok) {
      const msg = (parsed as Record<string, unknown>)?.message ?? body.slice(0, 200);
      throw new Error(`HTTP ${res.status} from ${url}: ${msg}`);
    }
    return parsed;
  } catch (err: unknown) {
    if (err instanceof DOMException && err.name === "AbortError") {
      throw new Error(`Request to ${url} timed out after 10 seconds`);
    }
    throw err;
  } finally {
    clearTimeout(timeout);
  }
}

/** Fetch the full FreeCycle status (GPU, VRAM, Ollama, active tasks, network). */
export async function getStatus(baseUrl?: string): Promise<FreeCycleStatus> {
  const base = baseUrl ?? resolveBase();
  return request<FreeCycleStatus>(`${base}/status`);
}

/** Signal that an agentic workflow is beginning GPU work. */
export async function startTask(
  taskId: string,
  description: string,
  baseUrl?: string,
): Promise<ApiResponse> {
  const base = baseUrl ?? resolveBase();
  return request<ApiResponse>(`${base}/task/start`, {
    method: "POST",
    headers: { "Content-Type": "application/json" },
    body: JSON.stringify({ task_id: taskId, description }),
  });
}

/** Signal that an agentic workflow has finished GPU work. */
export async function stopTask(
  taskId: string,
  baseUrl?: string,
): Promise<ApiResponse> {
  const base = baseUrl ?? resolveBase();
  return request<ApiResponse>(`${base}/task/stop`, {
    method: "POST",
    headers: { "Content-Type": "application/json" },
    body: JSON.stringify({ task_id: taskId }),
  });
}

/** Quick connectivity check. Returns the ApiResponse from GET /health. */
export async function healthCheck(baseUrl?: string): Promise<ApiResponse> {
  const base = baseUrl ?? resolveBase();
  return request<ApiResponse>(`${base}/health`);
}
