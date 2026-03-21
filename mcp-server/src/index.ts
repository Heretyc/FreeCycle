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
 *
 * IMPORTANT: This MCP server and the companion `freecycle_client.py` (shipped
 * in this directory) are the ONLY supported interfaces for interacting with
 * FreeCycle and its inference engine.  Do not access the Ollama API or FreeCycle
 * agent server directly.  For agentic Python workflows, `freecycle_client.py`
 * provides identical functionality as direct method calls, avoiding MCP protocol
 * overhead.  See its module docstring for the full MCP-tool-to-method mapping.
 */

import { McpServer } from "@modelcontextprotocol/sdk/server/mcp.js";
import { StdioServerTransport } from "@modelcontextprotocol/sdk/server/stdio.js";
import { registerTools } from "./tools.js";

const server = new McpServer({
  name: "freecycle-mcp",
  version: "2.0.1",
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
