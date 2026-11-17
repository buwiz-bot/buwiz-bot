// A small MCP client over stdio: newline-delimited JSON-RPC, as
// fixtures/mcp-server.mjs speaks it and `buwiz-broker office-mcp` serves
// it. `initialize`, `tools/list` and `tools/call` are all the tests need. No
// SDK, plain `node`.
import { spawn } from "node:child_process";
import { createInterface } from "node:readline";

export class McpClient {
  #child;
  #pending = new Map();
  #next = 1;

  /** `initialize`d: serverInfo, instructions and pid are set. */
  static async connect(command, args, options = {}) {
    const client = new McpClient(command, args, options);

    await client.#initialize();

    return client;
  }

  constructor(command, args, { cwd, env } = {}) {
    this.#child = spawn(command, args, { cwd, env, stdio: ["pipe", "pipe", "ignore"] });
    this.pid = this.#child.pid;
    this.exited = new Promise((done) => this.#child.once("exit", done));
    createInterface({ input: this.#child.stdout }).on("line", (line) => {
      let msg;

      try {
        msg = JSON.parse(line);
      } catch {
        return;
      }
      const waiting = this.#pending.get(msg.id);

      if (!waiting) return;
      this.#pending.delete(msg.id);
      if (msg.error) waiting.reject(Object.assign(new Error(`${waiting.method}: ${msg.error.message}`), { code: msg.error.code }));
      else waiting.resolve(msg.result);
    });
    this.#child.once("exit", () => {
      for (const w of this.#pending.values()) w.reject(new Error(`${w.method}: the server exited`));
      this.#pending.clear();
    });
  }

  #send(msg) {
    this.#child.stdin.write(`${JSON.stringify({ jsonrpc: "2.0", ...msg })}\n`);
  }

  request(method, params) {
    const id = this.#next++;

    return new Promise((resolve, reject) => {
      this.#pending.set(id, { method, resolve, reject });
      this.#send({ id, method, params });
    });
  }

  async #initialize() {
    const r = await this.request("initialize", { protocolVersion: "2025-06-18", capabilities: {}, clientInfo: { name: "http-test", version: "1.0.0" } });

    this.serverInfo = r.serverInfo;
    this.instructions = r.instructions;
    this.#send({ method: "notifications/initialized" });
  }

  listTools() {
    return this.request("tools/list", {});
  }

  /** The tool's `{content, structuredContent, isError}`; a JSON-RPC error rejects. */
  callTool(name, args) {
    return this.request("tools/call", { name, arguments: args });
  }

  async close() {
    this.#child.stdin.end();
    const t = setTimeout(() => this.#child.kill("SIGTERM"), 2000);

    await this.exited;
    clearTimeout(t);
  }
}
