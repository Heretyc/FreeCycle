import { getConfig } from "./config.js";
import type { FreeCycleStatus } from "./freecycle-client.js";
import * as freecycle from "./freecycle-client.js";
import * as ollama from "./ollama-client.js";
import { sendWakeOnLanPackets } from "./wake-on-lan.js";

export interface LocalAvailability {
  available: boolean;
  freecycleReachable: boolean;
  ollamaReachable: boolean;
  wakeOnLanEnabled: boolean;
  wakeOnLanAttempted: boolean;
  freecycleStatus: string | null;
  blockingProcesses: string[];
  reason: string;
}

let pendingAvailabilityCheck: Promise<LocalAvailability> | null = null;

const IMMEDIATE_FALLBACK_STATUSES = new Set([
  "Blocked (Game Running)",
  "Cooldown",
  "Wake Delay",
  "Error",
]);

function sleep(ms: number): Promise<void> {
  return new Promise((resolve) => {
    setTimeout(resolve, ms);
  });
}

function formatError(error: unknown): string {
  return error instanceof Error ? error.message : String(error);
}

function availabilityResult(
  overrides: Partial<LocalAvailability> & Pick<LocalAvailability, "available" | "reason">,
): LocalAvailability {
  const config = getConfig();
  return {
    available: overrides.available,
    freecycleReachable: overrides.freecycleReachable ?? false,
    ollamaReachable: overrides.ollamaReachable ?? false,
    wakeOnLanEnabled: overrides.wakeOnLanEnabled ?? config.wakeOnLan.enabled,
    wakeOnLanAttempted: overrides.wakeOnLanAttempted ?? false,
    freecycleStatus: overrides.freecycleStatus ?? null,
    blockingProcesses: overrides.blockingProcesses ?? [],
    reason: overrides.reason,
  };
}

function isImmediatelyUnavailable(status: string): boolean {
  return IMMEDIATE_FALLBACK_STATUSES.has(status);
}

function isStatusReady(status: FreeCycleStatus): boolean {
  return (
    (status.status === "Available" || status.status === "Agent Task Active") &&
    status.ollama_running
  );
}

async function tryGetFreecycleStatus(): Promise<
  | { ok: true; status: FreeCycleStatus }
  | { ok: false; message: string }
> {
  try {
    const status = await freecycle.getStatus();
    return { ok: true, status };
  } catch (error: unknown) {
    return { ok: false, message: formatError(error) };
  }
}

async function waitForAvailability(
  wakeOnLanAttempted: boolean,
  initialReason: string,
): Promise<LocalAvailability> {
  const config = getConfig();
  const deadline = Date.now() + config.wakeOnLan.maxWaitSecs * 1000;
  let lastFreecycleMessage = initialReason;
  let lastOllamaMessage = initialReason;

  while (Date.now() <= deadline) {
    const statusResult = await tryGetFreecycleStatus();
    if (statusResult.ok) {
      const status = statusResult.status;

      if (isImmediatelyUnavailable(status.status)) {
        return availabilityResult({
          available: false,
          freecycleReachable: true,
          wakeOnLanAttempted,
          freecycleStatus: status.status,
          blockingProcesses: status.blocking_processes,
          reason:
            status.blocking_processes.length > 0
              ? `FreeCycle is awake but currently ${status.status}. Blocking processes: ${status.blocking_processes.join(", ")}.`
              : `FreeCycle is awake but currently ${status.status}.`,
        });
      }

      if (isStatusReady(status)) {
        try {
          await ollama.healthCheck();
          return availabilityResult({
            available: true,
            freecycleReachable: true,
            ollamaReachable: true,
            wakeOnLanAttempted,
            freecycleStatus: status.status,
            blockingProcesses: status.blocking_processes,
            reason: "Ollama is responding.",
          });
        } catch (error: unknown) {
          lastOllamaMessage = formatError(error);
        }
      } else {
        lastOllamaMessage = `FreeCycle reports ${status.status} and ollama_running=${status.ollama_running}.`;
      }

      lastFreecycleMessage = `FreeCycle reports ${status.status}.`;
    } else {
      lastFreecycleMessage = statusResult.message;
    }

    if (Date.now() + config.wakeOnLan.pollIntervalSecs * 1000 > deadline) {
      break;
    }

    await sleep(config.wakeOnLan.pollIntervalSecs * 1000);
  }

  return availabilityResult({
    available: false,
    wakeOnLanAttempted,
    reason:
      `Local Ollama did not become available within ${config.wakeOnLan.maxWaitSecs} seconds. ` +
      `Last FreeCycle check: ${lastFreecycleMessage}. Last Ollama check: ${lastOllamaMessage}.`,
  });
}

async function performAvailabilityCheck(): Promise<LocalAvailability> {
  const config = getConfig();

  try {
    await ollama.healthCheck();
    return availabilityResult({
      available: true,
      ollamaReachable: true,
      reason: "Ollama is responding.",
    });
  } catch (ollamaError: unknown) {
    const ollamaMessage = formatError(ollamaError);
    const statusResult = await tryGetFreecycleStatus();

    if (statusResult.ok) {
      const status = statusResult.status;
      if (isImmediatelyUnavailable(status.status)) {
        return availabilityResult({
          available: false,
          freecycleReachable: true,
          freecycleStatus: status.status,
          blockingProcesses: status.blocking_processes,
          reason:
            status.blocking_processes.length > 0
              ? `FreeCycle is reachable but local inference is ${status.status}. Blocking processes: ${status.blocking_processes.join(", ")}.`
              : `FreeCycle is reachable but local inference is ${status.status}.`,
        });
      }

      return waitForAvailability(false, ollamaMessage);
    }

    if (!config.wakeOnLan.enabled) {
      return availabilityResult({
        available: false,
        reason:
          `Local Ollama is not responding, FreeCycle is unreachable, and wake-on-LAN is disabled. ` +
          `Last Ollama error: ${ollamaMessage}.`,
      });
    }

    if (config.wakeOnLan.macAddress.trim() === "") {
      return availabilityResult({
        available: false,
        reason:
          "wakeOnLan.enabled is true but wakeOnLan.macAddress is empty in the MCP config.",
      });
    }

    await sendWakeOnLanPackets(config.wakeOnLan);
    return waitForAvailability(true, ollamaMessage);
  }
}

export async function ensureLocalAvailability(): Promise<LocalAvailability> {
  if (pendingAvailabilityCheck == null) {
    pendingAvailabilityCheck = performAvailabilityCheck().finally(() => {
      pendingAvailabilityCheck = null;
    });
  }

  return pendingAvailabilityCheck;
}

export function createCloudFallbackPayload(
  action: string,
  availability: LocalAvailability,
): Record<string, unknown> {
  return {
    ok: false,
    action,
    local_available: false,
    suggested_route: "cloud",
    wake_on_lan_enabled: availability.wakeOnLanEnabled,
    wake_on_lan_attempted: availability.wakeOnLanAttempted,
    freecycle_reachable: availability.freecycleReachable,
    ollama_reachable: availability.ollamaReachable,
    freecycle_status: availability.freecycleStatus,
    blocking_processes: availability.blockingProcesses,
    message: availability.reason,
  };
}
