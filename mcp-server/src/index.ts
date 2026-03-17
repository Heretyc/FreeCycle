#!/usr/bin/env node

/**
 * FreeCycle MCP Server
 *
 * Exposes FreeCycle (GPU lifecycle manager) and its local LLM inference engine
 * as MCP tools for Claude Code, OpenAI Codex, or any MCP compatible client.
 *
 * Transport: stdio
 * Runtime config: ../freecycle-mcp.config.json by default
 * Optional env overrides: FREECYCLE_MCP_CONFIG, FREECYCLE_HOST, FREECYCLE_PORT,
 * ENGINE_HOST, ENGINE_PORT, and FREECYCLE_WOL_*
 */

import { McpServer } from "@modelcontextprotocol/sdk/server/mcp.js";
import { StdioServerTransport } from "@modelcontextprotocol/sdk/server/stdio.js";
import { registerTools } from "./tools.js";

const server = new McpServer({
  name: "freecycle-mcp",
  version: "0.1.0",
});

registerTools(server);

async function main() {
  const transport = new StdioServerTransport();
  await server.connect(transport);
}

main().catch((e) => {
  process.stderr.write(`Fatal: ${e instanceof Error ? e.message : String(e)}\n`);
  process.exit(1);
});
