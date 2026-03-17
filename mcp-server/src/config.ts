import { existsSync, mkdirSync, readFileSync, writeFileSync } from "node:fs";
import { dirname } from "node:path";
import { fileURLToPath } from "node:url";

export interface EndpointConfig {
  host: string;
  port: number;
}

export interface TimeoutConfig {
  requestSecs: number;
  inferenceSecs: number;
  pullSecs: number;
}

export interface WakeOnLanConfig {
  enabled: boolean;
  macAddress: string;
  broadcastAddress: string;
  port: number;
  packetCount: number;
  packetIntervalSecs: number;
  pollIntervalSecs: number;
  maxWaitSecs: number;
}

export interface ServerEntry {
  host: string;
  port: number;
  name?: string;
  approved: boolean;
  tls_fingerprint?: string;
  identity_uuid?: string;
  wakeOnLan?: Partial<WakeOnLanConfig>;
  timeouts?: Partial<TimeoutConfig>;
}

export interface McpServerConfig {
  freecycle?: EndpointConfig;
  servers?: ServerEntry[];
  engine: EndpointConfig;
  timeouts: TimeoutConfig;
  wakeOnLan: WakeOnLanConfig;
}

type PartialConfig = Partial<{
  freecycle: Partial<EndpointConfig>;
  servers: ServerEntry[];
  engine: Partial<EndpointConfig>;
  timeouts: Partial<TimeoutConfig>;
  wakeOnLan: Partial<WakeOnLanConfig>;
}>;

const DEFAULT_SERVER: ServerEntry = {
  host: "localhost",
  port: 7443,
  approved: true,
};

const DEFAULT_CONFIG: McpServerConfig = {
  servers: [DEFAULT_SERVER],
  engine: {
    host: "localhost",
    port: 11434,
  },
  timeouts: {
    requestSecs: 10,
    inferenceSecs: 300,
    pullSecs: 600,
  },
  wakeOnLan: {
    enabled: false,
    macAddress: "",
    broadcastAddress: "255.255.255.255",
    port: 9,
    packetCount: 5,
    packetIntervalSecs: 0.25,
    pollIntervalSecs: 30,
    maxWaitSecs: 900,
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

function normalizeServersArray(
  fileConfig: PartialConfig,
): ServerEntry[] {
  // If servers array exists, use it
  if (fileConfig.servers != null && fileConfig.servers.length > 0) {
    return fileConfig.servers;
  }

  // Backward compat: if old freecycle key exists, treat as single-server config
  if (fileConfig.freecycle != null) {
    const freecycleHost =
      process.env.FREECYCLE_HOST ?? fileConfig.freecycle.host ?? DEFAULT_SERVER.host;
    const freecyclePort = parseNumber(
      process.env.FREECYCLE_PORT,
      fileConfig.freecycle.port ?? DEFAULT_SERVER.port,
    );
    return [
      {
        host: freecycleHost,
        port: freecyclePort,
        approved: true,
      },
    ];
  }

  // Fallback to default
  return [DEFAULT_SERVER];
}

function mergeConfig(fileConfig: PartialConfig): McpServerConfig {
  const servers = normalizeServersArray(fileConfig);

  const engineHost =
    process.env.ENGINE_HOST ??
    fileConfig.engine?.host ??
    (servers[0]?.host ?? DEFAULT_SERVER.host);
  const enginePort = parseNumber(
    process.env.ENGINE_PORT,
    fileConfig.engine?.port ?? DEFAULT_CONFIG.engine.port,
  );

  return {
    servers,
    engine: {
      host: engineHost,
      port: enginePort,
    },
    timeouts: {
      requestSecs: parseNumber(
        process.env.FREECYCLE_REQUEST_TIMEOUT_SECS,
        fileConfig.timeouts?.requestSecs ?? DEFAULT_CONFIG.timeouts.requestSecs,
      ),
      inferenceSecs: parseNumber(
        process.env.FREECYCLE_INFERENCE_TIMEOUT_SECS,
        fileConfig.timeouts?.inferenceSecs ?? DEFAULT_CONFIG.timeouts.inferenceSecs,
      ),
      pullSecs: parseNumber(
        process.env.FREECYCLE_PULL_TIMEOUT_SECS,
        fileConfig.timeouts?.pullSecs ?? DEFAULT_CONFIG.timeouts.pullSecs,
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
      packetIntervalSecs: parseNumber(
        process.env.FREECYCLE_WOL_PACKET_INTERVAL_SECS,
        fileConfig.wakeOnLan?.packetIntervalSecs ??
          DEFAULT_CONFIG.wakeOnLan.packetIntervalSecs,
      ),
      pollIntervalSecs: parseNumber(
        process.env.FREECYCLE_WOL_POLL_INTERVAL_SECS,
        fileConfig.wakeOnLan?.pollIntervalSecs ??
          DEFAULT_CONFIG.wakeOnLan.pollIntervalSecs,
      ),
      maxWaitSecs: parseNumber(
        process.env.FREECYCLE_WOL_MAX_WAIT_SECS,
        fileConfig.wakeOnLan?.maxWaitSecs ?? DEFAULT_CONFIG.wakeOnLan.maxWaitSecs,
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

  const configPath = getConfigPath();

  // If the config file is missing, write defaults to disk so users have a
  // starting point to edit. Fatal if the write fails.
  if (!existsSync(configPath)) {
    try {
      const configDir = dirname(configPath);
      if (!existsSync(configDir)) {
        mkdirSync(configDir, { recursive: true });
      }
      writeFileSync(configPath, JSON.stringify(DEFAULT_CONFIG, null, 2), "utf8");
    } catch (err: unknown) {
      throw new Error(
        `Config file not found at ${configPath} and could not be recreated: ${err instanceof Error ? err.message : String(err)}`,
      );
    }
  }

  cachedConfig = mergeConfig(readConfigFile(configPath));
  return cachedConfig;
}

export function resetConfigCache(): void {
  cachedConfig = null;
}

export function getActiveServer(): ServerEntry {
  const config = getConfig();
  const approved = config.servers?.find((entry) => entry.approved);
  if (approved != null) {
    return approved;
  }

  // Fallback to first server if none approved
  if (config.servers != null && config.servers.length > 0) {
    return config.servers[0]!;
  }

  // Absolute fallback
  return DEFAULT_SERVER;
}

export function saveConfig(patch: Partial<McpServerConfig>): void {
  const configPath = getConfigPath();
  const fileConfig = readConfigFile(configPath);

  // Merge patch into file config
  const updated = {
    ...fileConfig,
    ...patch,
  };

  const configDir = dirname(configPath);
  const tempPath = `${configPath}.tmp`;

  try {
    // Write to temp file first (atomic-ish)
    writeFileSync(tempPath, JSON.stringify(updated, null, 2), "utf8");
    // Atomic rename (within same directory)
    const fs = require("node:fs");
    fs.renameSync(tempPath, configPath);
  } catch (err) {
    // Clean up temp file if write failed
    try {
      if (existsSync(tempPath)) {
        require("node:fs").unlinkSync(tempPath);
      }
    } catch {
      // Ignore cleanup error
    }
    throw err;
  }

  // Reset cache so next read sees new values
  resetConfigCache();
}
