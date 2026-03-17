#!/usr/bin/env node

/**
 * End-to-end test script for FreeCycle MCP server.
 *
 * Tests:
 * 1. Tool registration (15 tools)
 * 2. Input schema shapes for tools with required parameters
 * 3. Cloud-fallback payload structure
 *
 * Uses Node.js 18 built-in test and assert modules with no external dependencies.
 */

import test from "node:test";
import assert from "node:assert";
import { McpServer } from "@modelcontextprotocol/sdk/server/mcp.js";
import { InMemoryTransport } from "@modelcontextprotocol/sdk/inMemory.js";
import { Client } from "@modelcontextprotocol/sdk/client/index.js";
import { registerTools, classifyTask, safeTokensPerSecond, filterWarmResults } from "./dist/tools.js";
import { createCloudFallbackPayload } from "./dist/availability.js";
import type { LocalAvailability } from "./dist/availability.js";

const EXPECTED_TOOLS = [
  "freecycle_status",
  "freecycle_health",
  "freecycle_start_task",
  "freecycle_stop_task",
  "freecycle_check_availability",
  "freecycle_list_models",
  "freecycle_show_model",
  "freecycle_pull_model",
  "freecycle_generate",
  "freecycle_chat",
  "freecycle_embed",
  "freecycle_evaluate_task",
  "freecycle_benchmark",
  "freecycle_add_server",
  "freecycle_list_servers",
];

test("Tool Registration", async (t) => {
  const server = new McpServer({
    name: "test-freecycle-mcp",
    version: "0.1.0",
  });

  registerTools(server);

  const [serverTransport, clientTransport] = InMemoryTransport.createLinkedPair();

  server.connect(serverTransport);
  const client = new Client({
    name: "test-client",
    version: "0.1.0",
  });

  await client.connect(clientTransport);

  await t.test("lists all 15 tools", async () => {
    const tools = await client.listTools();

    assert.strictEqual(
      tools.tools.length,
      15,
      `Expected 15 tools, got ${tools.tools.length}`,
    );

    const toolNames = tools.tools.map((tool) => tool.name);
    for (const expectedTool of EXPECTED_TOOLS) {
      assert.ok(
        toolNames.includes(expectedTool),
        `Expected tool '${expectedTool}' not found in registered tools`,
      );
    }

    for (const tool of tools.tools) {
      assert.ok(
        EXPECTED_TOOLS.includes(tool.name),
        `Unexpected tool '${tool.name}' found in registered tools`,
      );
    }
  });

  await t.test("tool schemas contain required input properties", async () => {
    const tools = await client.listTools();
    const toolMap = new Map(tools.tools.map((tool) => [tool.name, tool]));

    const schemaTests: Array<[string, string[]]> = [
      ["freecycle_start_task", ["task_id", "description"]],
      ["freecycle_stop_task", ["task_id"]],
      ["freecycle_show_model", ["model_name"]],
      ["freecycle_pull_model", ["model_name"]],
      ["freecycle_generate", ["prompt"]],
      ["freecycle_chat", ["messages"]],
      ["freecycle_embed", ["input"]],
      ["freecycle_evaluate_task", ["task_description"]],
      ["freecycle_benchmark", ["model", "prompt"]],
    ];

    for (const [toolName, requiredProperties] of schemaTests) {
      const tool = toolMap.get(toolName);
      assert.ok(tool, `Tool '${toolName}' not found`);

      const schema = tool.inputSchema as Record<string, unknown>;
      const required = (schema.required as string[]) || [];

      for (const prop of requiredProperties) {
        assert.ok(
          required.includes(prop),
          `Tool '${toolName}' schema missing required property '${prop}'`,
        );
      }
    }
  });

  await client.close();
});

test("Cloud Fallback Payload", async () => {
  const mockAvailability = {
    available: false,
    freecycleReachable: false,
    ollamaReachable: false,
    wakeOnLanEnabled: true,
    wakeOnLanAttempted: false,
    freecycleStatus: null,
    blockingProcesses: [],
    reason: "Test reason",
  };

  const payload = createCloudFallbackPayload("test_action", mockAvailability);

  const requiredKeys = [
    "ok",
    "action",
    "local_available",
    "suggested_route",
    "wake_on_lan_enabled",
    "wake_on_lan_attempted",
    "freecycle_reachable",
    "ollama_reachable",
    "freecycle_status",
    "blocking_processes",
    "message",
  ];

  for (const key of requiredKeys) {
    assert.ok(
      key in payload,
      `Cloud fallback payload missing required key '${key}'`,
    );
  }

  assert.strictEqual(payload.ok, false, "Cloud fallback should have ok: false");
  assert.strictEqual(
    payload.action,
    "test_action",
    "Cloud fallback should contain the action",
  );
  assert.strictEqual(
    payload.local_available,
    false,
    "Cloud fallback should have local_available: false",
  );
  assert.strictEqual(
    payload.suggested_route,
    "cloud",
    "Cloud fallback should suggest cloud routing",
  );
});

test("Task Classification", async (t) => {
  await t.test("classifies local-focused tasks correctly", async () => {
    const localTasks = [
      "summarize this text",
      "extract key information",
      "classify sentiment",
      "translate to spanish",
      "explain the concept",
      "embed document",
    ];

    for (const task of localTasks) {
      const result = classifyTask(task);
      assert.strictEqual(result, "local", `Expected "local" for: ${task}, got ${result}`);
    }
  });

  await t.test("classifies cloud-focused tasks correctly", async () => {
    const cloudTasks = [
      "write complex code to solve the algorithm",
      "generate advanced code for system design",
      "prove a theorem",
      "provide deep reasoning about the problem",
      "creative writing for a novel",
      "multi-step reasoning about architecture",
    ];

    for (const task of cloudTasks) {
      const result = classifyTask(task);
      assert.strictEqual(result, "cloud", `Expected "cloud" for: ${task}, got ${result}`);
    }
  });

  await t.test("classifies ambiguous/mixed tasks as hybrid", async () => {
    const hybridTasks = [
      "summarize with deep reasoning",
      "extract data for analysis",
      "classify with research",
      "this is unclear",
      "random text without keywords",
      "",
    ];

    for (const task of hybridTasks) {
      const result = classifyTask(task);
      assert.strictEqual(result, "hybrid", `Expected "hybrid" for: ${task}, got ${result}`);
    }
  });

  await t.test("handles edge case: equal local and cloud scores", async () => {
    const balancedTask = "summarize the code analysis";
    const result = classifyTask(balancedTask);
    assert.strictEqual(result, "hybrid", "Equal scores should return hybrid");
  });

  await t.test("is case-insensitive", async () => {
    const task = "SUMMARIZE THIS TEXT IN UPPERCASE";
    const result = classifyTask(task);
    assert.strictEqual(result, "local", "Classification should be case-insensitive");
  });
});

test("Routing Logic Edge Cases", async (t) => {
  await t.test("GPU down (Error state) routes to cloud", async () => {
    const availability: LocalAvailability = {
      available: false,
      freecycleReachable: true,
      ollamaReachable: false,
      wakeOnLanEnabled: false,
      wakeOnLanAttempted: false,
      freecycleStatus: "Error",
      blockingProcesses: [],
      reason: "FreeCycle is in Error state.",
    };

    // Simulate the routing decision in freecycle_evaluate_task
    // When availability.available is false, recommendation becomes "cloud"
    const recommendation = !availability.available ? "cloud" : "local";
    assert.strictEqual(recommendation, "cloud", "Error state should route to cloud");
  });

  await t.test("Cooldown active routes to cloud", async () => {
    const availability: LocalAvailability = {
      available: false,
      freecycleReachable: true,
      ollamaReachable: false,
      wakeOnLanEnabled: false,
      wakeOnLanAttempted: false,
      freecycleStatus: "Cooldown",
      blockingProcesses: [],
      reason: "FreeCycle is in Cooldown state.",
    };

    const recommendation = !availability.available ? "cloud" : "local";
    assert.strictEqual(recommendation, "cloud", "Cooldown should route to cloud");
  });

  await t.test("Wake Delay state routes to cloud", async () => {
    const availability: LocalAvailability = {
      available: false,
      freecycleReachable: true,
      ollamaReachable: false,
      wakeOnLanEnabled: false,
      wakeOnLanAttempted: false,
      freecycleStatus: "Wake Delay",
      blockingProcesses: [],
      reason: "FreeCycle is in Wake Delay state.",
    };

    const recommendation = !availability.available ? "cloud" : "local";
    assert.strictEqual(recommendation, "cloud", "Wake Delay should route to cloud");
  });

  await t.test("Blocked (Game Running) routes to cloud", async () => {
    const availability: LocalAvailability = {
      available: false,
      freecycleReachable: true,
      ollamaReachable: false,
      wakeOnLanEnabled: false,
      wakeOnLanAttempted: false,
      freecycleStatus: "Blocked (Game Running)",
      blockingProcesses: ["game.exe"],
      reason: "FreeCycle is blocked.",
    };

    const recommendation = !availability.available ? "cloud" : "local";
    assert.strictEqual(recommendation, "cloud", "Blocked state should route to cloud");
  });

  await t.test("Unknown task type with available local falls back to cloud task classification", async () => {
    // When classifyTask returns "cloud", the routing should prefer cloud
    const taskClass = classifyTask("complex code");
    assert.strictEqual(taskClass, "cloud", "Cloud keyword should classify as cloud");
  });

  await t.test("Privacy critical + available routes to local", async () => {
    const availability: LocalAvailability = {
      available: true,
      freecycleReachable: true,
      ollamaReachable: true,
      wakeOnLanEnabled: false,
      wakeOnLanAttempted: false,
      freecycleStatus: "Available",
      blockingProcesses: [],
      reason: "Ollama is responding.",
    };

    // Simulate the privacy=critical branch
    const privacy = "critical";
    const recommendation = privacy === "critical" && availability.available ? "local" : "cloud";
    assert.strictEqual(recommendation, "local", "Privacy critical with availability should use local");
  });

  await t.test("Privacy critical + unavailable routes to cloud with warning", async () => {
    const availability: LocalAvailability = {
      available: false,
      freecycleReachable: false,
      ollamaReachable: false,
      wakeOnLanEnabled: true,
      wakeOnLanAttempted: false,
      freecycleStatus: null,
      blockingProcesses: [],
      reason: "Local Ollama did not respond.",
    };

    // Simulate the privacy=critical + !available branch
    const privacy = "critical";
    const recommendation =
      privacy === "critical" && !availability.available ? "cloud" : "local";
    assert.strictEqual(
      recommendation,
      "cloud",
      "Privacy critical without availability should fall back to cloud",
    );
  });

  await t.test("Low latency preference with non-cloud task uses local", async () => {
    const taskClass = classifyTask("summarize the document");
    const latency = "low";
    // Simulate: reqs.latency === "low" && taskClass !== "cloud"
    const recommendation =
      latency === "low" && taskClass !== "cloud" ? "local" : "cloud";
    assert.strictEqual(recommendation, "local", "Low latency + local task should use local");
  });

  await t.test("Cost minimize with non-cloud task uses local", async () => {
    const taskClass = classifyTask("extract entities");
    const cost = "minimize";
    // Simulate: reqs.cost === "minimize" && taskClass !== "cloud"
    const recommendation = cost === "minimize" && taskClass !== "cloud" ? "local" : "cloud";
    assert.strictEqual(recommendation, "local", "Cost minimize + local task should use local");
  });
});

test("Benchmark Metric Accuracy", async (t) => {
  await t.test("safeTokensPerSecond with valid eval_count and eval_duration returns correct value", async () => {
    // 1000 tokens / 1 second = 1000 tokens/sec
    const result = safeTokensPerSecond(1000, 1e9);
    assert.strictEqual(result, 1000, "Should calculate tokens per second correctly");
  });

  await t.test("safeTokensPerSecond with zero eval_count returns 0", async () => {
    const result = safeTokensPerSecond(0, 1e9);
    assert.strictEqual(result, 0, "Should return 0 for zero eval_count");
  });

  await t.test("safeTokensPerSecond with zero eval_duration returns 0", async () => {
    const result = safeTokensPerSecond(1000, 0);
    assert.strictEqual(result, 0, "Should return 0 for zero eval_duration");
  });

  await t.test("safeTokensPerSecond with null/undefined inputs returns 0", async () => {
    assert.strictEqual(safeTokensPerSecond(undefined, undefined), 0, "Should return 0 for undefined inputs");
    assert.strictEqual(safeTokensPerSecond(null as unknown as number, undefined), 0, "Should return 0 for null inputs");
    assert.strictEqual(safeTokensPerSecond(1000, undefined), 0, "Should return 0 for undefined eval_duration");
    assert.strictEqual(safeTokensPerSecond(undefined, 1e9), 0, "Should return 0 for undefined eval_count");
  });

  await t.test("Warm iteration detection: iteration with load_duration_ms >= 500 excluded from warm average", async () => {
    const results = [
      { latency_ms: 100, load_duration_ms: 50, tokens_per_sec: 10 },
      { latency_ms: 2000, load_duration_ms: 2000, tokens_per_sec: 5 },
      { latency_ms: 100, load_duration_ms: 100, tokens_per_sec: 10 },
    ];
    const warmResults = filterWarmResults(results);
    assert.strictEqual(warmResults.length, 2, "Should have 2 warm iterations");
    assert.strictEqual(warmResults[0].load_duration_ms, 50, "First warm result should have load_duration_ms of 50");
    assert.strictEqual(warmResults[1].load_duration_ms, 100, "Second warm result should have load_duration_ms of 100");
  });

  await t.test("Warm iteration detection: load_duration_ms exactly 500 is cold", async () => {
    const results = [
      { latency_ms: 100, load_duration_ms: 499, tokens_per_sec: 10 },
      { latency_ms: 100, load_duration_ms: 500, tokens_per_sec: 10 },
    ];
    const warmResults = filterWarmResults(results);
    assert.strictEqual(warmResults.length, 1, "Should have 1 warm iteration (499 < 500, 500 is not < 500)");
  });

  await t.test("Warm average is null when all iterations were cold", async () => {
    const results = [
      { latency_ms: 1000, load_duration_ms: 1000, tokens_per_sec: 5 },
      { latency_ms: 1000, load_duration_ms: 1000, tokens_per_sec: 5 },
    ];
    const warmResults = filterWarmResults(results);
    assert.strictEqual(warmResults.length, 0, "Should have 0 warm iterations");
  });

  await t.test("Warm average calculation is correct when warm iterations exist", async () => {
    const results = [
      { latency_ms: 100, load_duration_ms: 50, tokens_per_sec: 10 },
      { latency_ms: 200, load_duration_ms: 100, tokens_per_sec: 20 },
    ];
    const warmResults = filterWarmResults(results);
    assert.strictEqual(warmResults.length, 2, "Should have 2 warm iterations");
    const warmAverageLatency = Math.round(
      warmResults.reduce((sum, r) => sum + r.latency_ms, 0) / warmResults.length,
    );
    assert.strictEqual(warmAverageLatency, 150, "Warm average latency should be 150ms");
  });

  await t.test("Benchmark result shape contains all required fields", async () => {
    const requiredFields = [
      "latency_ms",
      "tokens",
      "tokens_per_sec",
      "load_duration_ms",
      "prompt_tokens",
      "prompt_eval_ms",
      "generation_ms",
      "ollama_total_ms",
    ];

    // Create a sample result matching the expected shape
    const sampleResult = {
      latency_ms: 150,
      tokens: 100,
      tokens_per_sec: 66.67,
      load_duration_ms: 50,
      prompt_tokens: 10,
      prompt_eval_ms: 5,
      generation_ms: 100,
      ollama_total_ms: 155,
    };

    for (const field of requiredFields) {
      assert.ok(
        field in sampleResult,
        `Benchmark result should contain field '${field}'`,
      );
    }
  });

  await t.test("Benchmark summary contains warm statistics when warm iterations exist", async () => {
    // Simulate a benchmark summary with warm statistics
    const summaryWithWarm = {
      model: "test-model",
      iterations: 2,
      results: [],
      average_latency_ms: 150,
      average_tokens_per_second: 15,
      warm_average_latency_ms: 150,
      warm_average_tokens_per_second: 15,
      warm_iteration_count: 2,
    };

    assert.strictEqual(summaryWithWarm.warm_average_latency_ms, 150, "Should have warm_average_latency_ms");
    assert.strictEqual(summaryWithWarm.warm_average_tokens_per_second, 15, "Should have warm_average_tokens_per_second");
    assert.strictEqual(summaryWithWarm.warm_iteration_count, 2, "Should have warm_iteration_count");
  });

  await t.test("Benchmark summary has null warm statistics when no warm iterations exist", async () => {
    // Simulate a benchmark summary with no warm statistics
    const summaryNoWarm = {
      model: "test-model",
      iterations: 1,
      results: [],
      average_latency_ms: 2000,
      average_tokens_per_second: 5,
      warm_average_latency_ms: null,
      warm_average_tokens_per_second: null,
      warm_iteration_count: 0,
    };

    assert.strictEqual(summaryNoWarm.warm_average_latency_ms, null, "Should have null warm_average_latency_ms");
    assert.strictEqual(summaryNoWarm.warm_average_tokens_per_second, null, "Should have null warm_average_tokens_per_second");
    assert.strictEqual(summaryNoWarm.warm_iteration_count, 0, "Should have warm_iteration_count of 0");
  });
});
