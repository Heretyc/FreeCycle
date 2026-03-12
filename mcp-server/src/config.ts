import { existsSync, readFileSync } from "node:fs";
import { fileURLToPath } from "node:url";

export interface EndpointConfig {
  host: string;
  port: number;
}

export interface TimeoutConfig {
  requestMs: number;
  inferenceMs: number;
  pullMs: number;
}

export interface WakeOnLanConfig {
  enabled: boolean;
  macAddress: string;
  broadcastAddress: string;
  port: number;
  packetCount: number;
  packetIntervalMs: number;
  pollIntervalMs: number;
  maxWaitMs: number;
}

export interface McpServerConfig {
  freecycle: EndpointConfig;
  ollama: EndpointConfig;
  timeouts: TimeoutConfig;
  wakeOnLan: WakeOnLanConfig;
}

type PartialConfig = Partial<{
  freecycle: Partial<EndpointConfig>;
  ollama: Partial<EndpointConfig>;
  timeouts: Partial<TimeoutConfig>;
  wakeOnLan: Partial<WakeOnLanConfig>;
}>;

const DEFAULT_CONFIG: McpServerConfig = {
  freecycle: {
    host: "localhost",
    port: 7443,
  },
  ollama: {
    host: "localhost",
    port: 11434,
  },
  timeouts: {
    requestMs: 10_000,
    inferenceMs: 5 * 60 * 1000,
    pullMs: 10 * 60 * 1000,
  },
  wakeOnLan: {
    enabled: false,
    macAddress: "",
    broadcastAddress: "255.255.255.255",
    port: 9,
    packetCount: 5,
    packetIntervalMs: 250,
    pollIntervalMs: 30_000,
    maxWaitMs: 15 * 60 * 1000,
  },
};

const DEFAULT_CONFIG_PATH = fileURLToPath(
  new URL("../freecycle-mcp.config.json", import.meta.url),
);

let cachedConfig: McpServerConfig | null = null;

function parseNumber(value: string | undefined, fallback: number): number {
  if (value == null || value.trim() === "") {
    return fallback;
  }

  const parsed = Number(value);
  return Number.isFinite(parsed) ? parsed : fallback;
}

function parseBoolean(value: string | undefined, fallback: boolean): boolean {
  if (value == null || value.trim() === "") {
    return fallback;
  }

  return value.trim().toLowerCase() === "true";
}

function readConfigFile(configPath: string): PartialConfig {
  if (!existsSync(configPath)) {
    return {};
  }

  const raw = readFileSync(configPath, "utf8");
  const parsed = JSON.parse(raw) as PartialConfig;
  return parsed;
}

function mergeConfig(fileConfig: PartialConfig): McpServerConfig {
  const freecycleHost =
    process.env.FREECYCLE_HOST ??
    fileConfig.freecycle?.host ??
    DEFAULT_CONFIG.freecycle.host;
  const freecyclePort = parseNumber(
    process.env.FREECYCLE_PORT,
    fileConfig.freecycle?.port ?? DEFAULT_CONFIG.freecycle.port,
  );

  const ollamaHost =
    process.env.OLLAMA_HOST ??
    fileConfig.ollama?.host ??
    freecycleHost;
  const ollamaPort = parseNumber(
    process.env.OLLAMA_PORT,
    fileConfig.ollama?.port ?? DEFAULT_CONFIG.ollama.port,
  );

  return {
    freecycle: {
      host: freecycleHost,
      port: freecyclePort,
    },
    ollama: {
      host: ollamaHost,
      port: ollamaPort,
    },
    timeouts: {
      requestMs: parseNumber(
        process.env.FREECYCLE_REQUEST_TIMEOUT_MS,
        fileConfig.timeouts?.requestMs ?? DEFAULT_CONFIG.timeouts.requestMs,
      ),
      inferenceMs: parseNumber(
        process.env.FREECYCLE_INFERENCE_TIMEOUT_MS,
        fileConfig.timeouts?.inferenceMs ?? DEFAULT_CONFIG.timeouts.inferenceMs,
      ),
      pullMs: parseNumber(
        process.env.FREECYCLE_PULL_TIMEOUT_MS,
        fileConfig.timeouts?.pullMs ?? DEFAULT_CONFIG.timeouts.pullMs,
      ),
    },
    wakeOnLan: {
      enabled: parseBoolean(
        process.env.FREECYCLE_WOL_ENABLED,
        fileConfig.wakeOnLan?.enabled ?? DEFAULT_CONFIG.wakeOnLan.enabled,
      ),
      macAddress:
        process.env.FREECYCLE_WOL_MAC ??
        fileConfig.wakeOnLan?.macAddress ??
        DEFAULT_CONFIG.wakeOnLan.macAddress,
      broadcastAddress:
        process.env.FREECYCLE_WOL_BROADCAST ??
        fileConfig.wakeOnLan?.broadcastAddress ??
        DEFAULT_CONFIG.wakeOnLan.broadcastAddress,
      port: parseNumber(
        process.env.FREECYCLE_WOL_PORT,
        fileConfig.wakeOnLan?.port ?? DEFAULT_CONFIG.wakeOnLan.port,
      ),
      packetCount: parseNumber(
        process.env.FREECYCLE_WOL_PACKET_COUNT,
        fileConfig.wakeOnLan?.packetCount ?? DEFAULT_CONFIG.wakeOnLan.packetCount,
      ),
      packetIntervalMs: parseNumber(
        process.env.FREECYCLE_WOL_PACKET_INTERVAL_MS,
        fileConfig.wakeOnLan?.packetIntervalMs ??
          DEFAULT_CONFIG.wakeOnLan.packetIntervalMs,
      ),
      pollIntervalMs: parseNumber(
        process.env.FREECYCLE_WOL_POLL_INTERVAL_MS,
        fileConfig.wakeOnLan?.pollIntervalMs ??
          DEFAULT_CONFIG.wakeOnLan.pollIntervalMs,
      ),
      maxWaitMs: parseNumber(
        process.env.FREECYCLE_WOL_MAX_WAIT_MS,
        fileConfig.wakeOnLan?.maxWaitMs ?? DEFAULT_CONFIG.wakeOnLan.maxWaitMs,
      ),
    },
  };
}

export function getConfigPath(): string {
  return process.env.FREECYCLE_MCP_CONFIG ?? DEFAULT_CONFIG_PATH;
}

export function getConfig(): McpServerConfig {
  if (cachedConfig != null) {
    return cachedConfig;
  }

  cachedConfig = mergeConfig(readConfigFile(getConfigPath()));
  return cachedConfig;
}

export function resetConfigCache(): void {
  cachedConfig = null;
}
