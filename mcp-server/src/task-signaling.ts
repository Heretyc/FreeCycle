import { randomUUID } from "node:crypto";
import {
  createCloudFallbackPayload,
  type LocalAvailability,
} from "./availability.js";
import * as freecycle from "./freecycle-client.js";

const MAX_TASK_DESCRIPTION_LENGTH = 64;
const TASK_ID_SUFFIX_LENGTH = 8;

export interface TrackedLocalOperationOptions {
  action: string;
  operationLabel: string;
  modelName: string;
  availability: LocalAvailability;
  detail?: string;
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

function truncateDescription(description: string): string {
  if (description.length <= MAX_TASK_DESCRIPTION_LENGTH) {
    return description;
  }

  return `${description.slice(0, MAX_TASK_DESCRIPTION_LENGTH - 3)}...`;
}

function buildTaskId(action: string): string {
  const suffix = randomUUID().replace(/-/g, "").slice(0, TASK_ID_SUFFIX_LENGTH);
  return `mcp-${sanitizeAction(action)}-${Date.now()}-${suffix}`;
}

function buildTaskDescription(options: TrackedLocalOperationOptions): string {
  const detail = options.detail?.trim();
  const description = detail
    ? `MCP ${options.operationLabel}: ${options.modelName} ${detail}`
    : `MCP ${options.operationLabel}: ${options.modelName}`;

  return truncateDescription(description);
}

async function buildConflictPayload(
  options: TrackedLocalOperationOptions,
  message: string,
): Promise<Record<string, unknown>> {
  let conflictAvailability: LocalAvailability = {
    ...options.availability,
    available: false,
    freecycleReachable: true,
    ollamaReachable: false,
    reason: message,
  };

  try {
    const status = await freecycle.getStatus();
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
    const response = await freecycle.startTaskDetailed(taskId, description);
    if (response.ok && response.body.ok) {
      return { kind: "continue", started: true };
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

async function stopTrackedTask(action: string, taskId: string): Promise<void> {
  try {
    const response = await freecycle.stopTaskDetailed(taskId);
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
      await stopTrackedTask(options.action, taskId);
    }
  }
}
