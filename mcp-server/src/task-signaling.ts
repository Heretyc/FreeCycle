import { randomUUID } from "node:crypto";
import { type ServerEntry } from "./config.js";
import {
  createCloudFallbackPayload,
  type LocalAvailability,
} from "./availability.js";
import * as freecycle from "./freecycle-client.js";

const MIN_TASK_DESCRIPTION_LENGTH = 30;
const MAX_TASK_DESCRIPTION_LENGTH = 40;
const TASK_ID_SUFFIX_LENGTH = 8;

export interface TrackedLocalOperationOptions {
  action: string;
  operationLabel: string;
  modelName: string;
  availability: LocalAvailability;
  detail?: string;
  server?: ServerEntry;
}

export type TrackedLocalOperationResult<T> =
  | { kind: "completed"; value: T }
  | { kind: "unavailable"; payload: Record<string, unknown> };

type StartDecision =
  | { kind: "continue"; started: boolean }
  | { kind: "fatal"; error: Error }
  | { kind: "unavailable"; payload: Record<string, unknown> };

function formatError(error: unknown): string {
  return error instanceof Error ? error.message : String(error);
}

function warn(message: string): void {
  console.error(`[freecycle-mcp] ${message}`);
}

function sanitizeAction(action: string): string {
  const sanitized = action
    .replace(/^freecycle_/, "")
    .toLowerCase()
    .replace(/[^a-z0-9]+/g, "-")
    .replace(/^-+|-+$/g, "");

  return sanitized === "" ? "task" : sanitized;
}

function buildTaskId(action: string): string {
  const suffix = randomUUID().replace(/-/g, "").slice(0, TASK_ID_SUFFIX_LENGTH);
  return `mcp-${sanitizeAction(action)}-${Date.now()}-${suffix}`;
}

/// Validates a task description according to Priority 10 constraints.
/// Returns null if valid, or an error string if invalid.
function validateTaskDescription(description: string): string | null {
  // Check 1: Length must be 30-40 characters
  if (description.length < MIN_TASK_DESCRIPTION_LENGTH || description.length > MAX_TASK_DESCRIPTION_LENGTH) {
    return "Task description must be 30-40 characters";
  }

  // Check 2: No char dominance (>60% of description)
  const charFreq = new Map<string, number>();
  for (const ch of description.toLowerCase()) {
    charFreq.set(ch, (charFreq.get(ch) || 0) + 1);
  }
  const maxCharFreq = Math.max(...Array.from(charFreq.values()));
  if (maxCharFreq / description.length > 0.60) {
    return "Task description appears to contain padding";
  }

  // Check 3: Must contain at least one alphanumeric character
  if (!/[a-z0-9]/i.test(description)) {
    return "Task description appears to contain padding";
  }

  // Check 4: Repeated words dominance (>60% of qualifying words)
  const words = description
    .toLowerCase()
    .split(/\s+/)
    .filter(w => w.length >= 2);

  if (words.length >= 3) {
    const wordFreq = new Map<string, number>();
    for (const word of words) {
      wordFreq.set(word, (wordFreq.get(word) || 0) + 1);
    }
    const maxWordFreq = Math.max(...Array.from(wordFreq.values()));
    if (maxWordFreq / words.length > 0.60) {
      return "Task description appears to contain padding";
    }
  }

  return null;
}

function buildTaskDescription(options: TrackedLocalOperationOptions): string {
  const detail = options.detail?.trim();
  const base = detail
    ? `MCP ${options.operationLabel}: ${options.modelName} ${detail}`
    : `MCP ${options.operationLabel}: ${options.modelName}`;

  // Trim to at most 40 chars
  let description = base.length > MAX_TASK_DESCRIPTION_LENGTH
    ? base.slice(0, MAX_TASK_DESCRIPTION_LENGTH)
    : base;

  // Pad to at least 30 chars with meaningful text
  if (description.length < MIN_TASK_DESCRIPTION_LENGTH) {
    // Try padding with (local) first
    if (!description.includes("(local)")) {
      const candidate = `${description} (local)`;
      if (candidate.length <= MAX_TASK_DESCRIPTION_LENGTH) {
        description = candidate;
      }
    }
    // Still short? Try via API
    if (description.length < MIN_TASK_DESCRIPTION_LENGTH && !description.includes("via API")) {
      const candidate = `${description} via API`;
      if (candidate.length <= MAX_TASK_DESCRIPTION_LENGTH) {
        description = candidate;
      }
    }
    // Still short? Pad with spaces to reach minimum
    if (description.length < MIN_TASK_DESCRIPTION_LENGTH) {
      description = description.padEnd(MIN_TASK_DESCRIPTION_LENGTH, " ");
    }
  }

  // Final validation and safe fallback
  const validation = validateTaskDescription(description);
  if (validation !== null) {
    warn(`Task description validation failed: ${validation}. Using fallback.`);
    return "MCP task via FreeCycle local API";
  }

  return description;
}

async function buildConflictPayload(
  options: TrackedLocalOperationOptions,
  message: string,
): Promise<Record<string, unknown>> {
  let conflictAvailability: LocalAvailability = {
    ...options.availability,
    available: false,
    freecycleReachable: true,
    engineReachable: false,
    reason: message,
  };

  try {
    const status = await freecycle.getStatus(options.server);
    conflictAvailability = {
      ...conflictAvailability,
      freecycleStatus: status.status,
      blockingProcesses: status.blocking_processes,
      reason:
        status.blocking_processes.length > 0
          ? `FreeCycle became unavailable while starting ${options.action}. Status: ${status.status}. Blocking processes: ${status.blocking_processes.join(", ")}.`
          : `FreeCycle became unavailable while starting ${options.action}. Status: ${status.status}.`,
    };
  } catch (error: unknown) {
    conflictAvailability = {
      ...conflictAvailability,
      reason:
        `${message} Last status refresh failed: ${formatError(error)}.`,
    };
  }

  return {
    ...createCloudFallbackPayload(options.action, conflictAvailability),
    task_signal_conflict: true,
  };
}

async function beginTrackedTask(
  options: TrackedLocalOperationOptions,
  taskId: string,
  description: string,
): Promise<StartDecision> {
  try {
    const response = await freecycle.startTaskDetailed(taskId, description, options.server);
    if (response.ok && response.body.ok) {
      return { kind: "continue", started: true };
    }

    if (response.status === 400) {
      warn(
        `Task description rejected by server for ${options.action} (${taskId}): ${response.body.message}. Continuing without tray tracking.`,
      );
      return { kind: "continue", started: false };
    }

    if (response.status === 409) {
      return {
        kind: "unavailable",
        payload: await buildConflictPayload(options, response.body.message),
      };
    }

    if (response.status >= 500) {
      warn(
        `Automatic task start failed for ${options.action} (${taskId}) with HTTP ${response.status}: ${response.body.message}. Continuing without tray tracking.`,
      );
      return { kind: "continue", started: false };
    }

    return {
      kind: "fatal",
      error: new Error(
        `FreeCycle rejected automatic task start for ${options.action} (${taskId}) with HTTP ${response.status}: ${response.body.message}`,
      ),
    };
  } catch (error: unknown) {
    warn(
      `Automatic task start failed for ${options.action} (${taskId}): ${formatError(error)}. Continuing without tray tracking.`,
    );
    return { kind: "continue", started: false };
  }
}

async function stopTrackedTask(
  action: string,
  taskId: string,
  server?: ServerEntry,
): Promise<void> {
  try {
    const response = await freecycle.stopTaskDetailed(taskId, server);
    if (response.ok && response.body.ok) {
      return;
    }

    if (response.status === 404) {
      warn(
        `Automatic task stop drifted for ${action} (${taskId}). Another local request likely replaced the active FreeCycle task before cleanup.`,
      );
      return;
    }

    warn(
      `Automatic task stop failed for ${action} (${taskId}) with HTTP ${response.status}: ${response.body.message}`,
    );
  } catch (error: unknown) {
    warn(`Automatic task stop failed for ${action} (${taskId}): ${formatError(error)}`);
  }
}

export async function runTrackedLocalOperation<T>(
  options: TrackedLocalOperationOptions,
  operation: () => Promise<T>,
): Promise<TrackedLocalOperationResult<T>> {
  const taskId = buildTaskId(options.action);
  const description = buildTaskDescription(options);
  const startDecision = await beginTrackedTask(options, taskId, description);

  if (startDecision.kind === "fatal") {
    throw startDecision.error;
  }

  if (startDecision.kind === "unavailable") {
    return startDecision;
  }

  try {
    const value = await operation();
    return { kind: "completed", value };
  } finally {
    if (startDecision.started) {
      await stopTrackedTask(options.action, taskId, options.server);
    }
  }
}
