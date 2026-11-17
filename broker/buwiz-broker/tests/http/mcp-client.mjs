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
    this.exited = new Promise((done) => {
      this.#child.once("exit", done);
      this.#child.once("error", (e) => this.#fail(`could not start ${command}: ${e.message}`, done));
    });
    // a write to a dead child is an error event: it must reject, not kill the test process
    this.#child.stdin.on("error", (e) => this.#fail(`the server's stdin failed: ${e.message}`));
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
    this.#child.once("exit", (code, signal) => this.#fail(`the server exited (${signal || `code ${code}`})`));
  }

  #dead = null;

  /** From now on every request, pending or new, is rejected with `why`. */
  #fail(why, done) {
    this.#dead ??= why;
    for (const w of this.#pending.values()) w.reject(new Error(`${w.method}: ${this.#dead}`));
    this.#pending.clear();
    done?.();
  }

  #send(msg) {
    if (this.#dead || !this.#child.stdin.writable) return false;
    this.#child.stdin.write(`${JSON.stringify({ jsonrpc: "2.0", ...msg })}\n`);

    return true;
  }

  request(method, params) {
    const id = this.#next++;

    return new Promise((resolve, reject) => {
      if (this.#dead) return reject(new Error(`${method}: ${this.#dead}`));
      this.#pending.set(id, { method, resolve, reject });
      if (!this.#send({ id, method, params })) {
        this.#pending.delete(id);
        reject(new Error(`${method}: ${this.#dead || "the server's stdin is closed"}`));
      }
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
    try {
      if (this.#child.stdin.writable) this.#child.stdin.end();
    } catch {}
    const t = setTimeout(() => this.#child.kill("SIGTERM"), 2000);

    await this.exited;
    clearTimeout(t);
  }
}
