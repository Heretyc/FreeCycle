/**
 * Multi-server routing and status querying.
 *
 * Queries FreeCycle servers concurrently, selects the best based on available VRAM
 * or model presence, and caches model lists with a 5-minute TTL.
 */

import { getConfig, type ServerEntry } from "./config.js";
import * as fc from "./freecycle-client.js";
import * as engine from "./engine-client.js";

export interface ServerStatusResult {
  server: ServerEntry;
  status: fc.FreeCycleStatus;
  reachable: true;
  freeVramMb: number;
}

export interface ServerStatusError {
  server: ServerEntry;
  reachable: false;
  error: string;
}

export type ServerProbe = ServerStatusResult | ServerStatusError;

interface CacheEntry {
  models: string[];
  timestamp: number;
}

const modelCache = new Map<string, CacheEntry>();
const MODEL_CACHE_TTL_MS = 5 * 60 * 1000; // 5 minutes
const downedServers = new Set<string>(); // keys: "host:port"

function getCacheKey(server: ServerEntry): string {
  return `${server.host}:${server.port}`;
}

function isReady(status: fc.FreeCycleStatus): boolean {
  return (
    (status.status === "Available" || status.status === "Agent Task Active") &&
    status.ollama_running
  );
}

/** Query the status of a single FreeCycle server with timeout and error handling. */
async function queryServer(server: ServerEntry): Promise<ServerProbe> {
  const cacheKey = getCacheKey(server);
  if (downedServers.has(cacheKey)) {
    return {
      server,
      reachable: false,
      error: "Server marked as down in this session",
    };
  }

  try {
    const status = await fc.getStatus(server);
    const freeVramMb = status.vram_total_mb - status.vram_used_mb;
    return {
      server,
      status,
      reachable: true,
      freeVramMb,
    };
  } catch (error: unknown) {
    const msg = error instanceof Error ? error.message : String(error);
    return {
      server,
      reachable: false,
      error: msg,
    };
  }
}

/** Query all approved servers concurrently. */
export async function queryAllServers(): Promise<ServerProbe[]> {
  const config = getConfig();
  const servers = config.servers ?? [];
  const approved = servers.filter((s) => s.approved);

  if (approved.length === 0) {
    return [];
  }

  const promises = approved.map((server) => queryServer(server));
  return Promise.all(promises);
}

/** Get the list of model names on a server, using the cache if available and fresh. */
async function getModelsForServer(server: ServerEntry): Promise<string[]> {
  const cacheKey = getCacheKey(server);
  const cached = modelCache.get(cacheKey);

  if (cached && Date.now() - cached.timestamp < MODEL_CACHE_TTL_MS) {
    return cached.models;
  }

  try {
    const response = await engine.listModels(server);
    const modelNames = response.models.map((m) => m.name);
    modelCache.set(cacheKey, {
      models: modelNames,
      timestamp: Date.now(),
    });
    return modelNames;
  } catch {
    // On error, return empty list (cache miss)
    return [];
  }
}

/**
 * Select the best available server based on:
 * 1. If modelName is provided:
 *    a. Partition servers into has_model and missing_model
 *    b. If has_model is non-empty, pick the one with max freeVramMb
 *    c. Else pick max freeVramMb from all ready servers
 * 2. If no modelName, pick max freeVramMb from ready servers
 * 3. If no ready servers, return null (cloud fallback)
 */
export async function selectBestServer(
  modelName?: string,
): Promise<ServerStatusResult | null> {
  const probes = await queryAllServers();

  // Filter to ready servers
  const readyServers: ServerStatusResult[] = [];
  for (const probe of probes) {
    if (probe.reachable && isReady(probe.status)) {
      readyServers.push(probe);
    }
  }

  if (readyServers.length === 0) {
    return null;
  }

  // If no model name, just pick the one with most free VRAM
  if (!modelName) {
    return readyServers.reduce((best, current) =>
      current.freeVramMb > best.freeVramMb ? current : best,
    );
  }

  // Partition by model availability
  const hasModelServers: ServerStatusResult[] = [];
  const missingModelServers: ServerStatusResult[] = [];

  for (const server of readyServers) {
    const models = await getModelsForServer(server.server);
    if (models.some((m) => m === modelName)) {
      hasModelServers.push(server);
    } else {
      missingModelServers.push(server);
    }
  }

  // Prefer servers that have the model
  const candidates = hasModelServers.length > 0 ? hasModelServers : missingModelServers;

  return candidates.reduce((best, current) =>
    current.freeVramMb > best.freeVramMb ? current : best,
  );
}

/** Mark a server as down for this session (not persisted). */
export function markServerDown(server: ServerEntry): void {
  const cacheKey = getCacheKey(server);
  downedServers.add(cacheKey);
}

/** Clear the model cache for all servers or a specific one. */
export function clearModelCache(serverKey?: string): void {
  if (serverKey) {
    modelCache.delete(serverKey);
  } else {
    modelCache.clear();
  }
}
