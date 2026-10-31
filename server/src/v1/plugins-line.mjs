// v1/plugins-line.mjs — v1 plugins hosted by rox-broker (Rust broker plan
// 3.4): the interface of createPlugins() over the line, rox-broker's
// /v1/line/* (broker/rox-broker/src/helper/line.rs).
//
// rox-broker owns the processes, the keys and the v1_plugins records; this
// keeps what the rest of the broker reads synchronously: the records (the
// same app.db, row by row), each plugin's runtime (a snapshot the line
// sends on every change, newest wins by `seq`) and the plugin list. Tool
// results come back raw and go through the MCP SDK's schemas here, so
// callers see what the in-process client gave them. App sessions live in
// rox-broker too (sessions-line.mjs), which ends a plugin's opens itself.
//
// With a fake clock (tests) rox-broker runs a manual clock: each restart it
// schedules is mirrored as a timer on that clock, whose firing tells the
// line to fire it.
import { EventEmitter } from "node:events";
import { CallToolResultSchema, ListToolsResultSchema, ReadResourceResultSchema } from "@modelcontextprotocol/sdk/types.js";
import { V1Error } from "./errors.mjs";
import { createViews, systemClock } from "./plugins.mjs";
import { loadTrust } from "./trust.mjs";

/** `base` is rox-broker's address (or a promise of it), `token` the line's. */
export function createLinePlugins({ base, token, db, clock = systemClock, trust = loadTrust, timeouts = {} }) {
  const T = { call: 120_000, resolve: 10_000, reserved: 5_000, cancel: 5_000, ...timeouts };
  const events = new EventEmitter();
  const runtimes = new Map(); // id → runtime, as rox-broker last sent it
  const seqs = new Map(); // id → the newest snapshot's seq
  const said = new Map(); // id → the last state announced to Rox
  const manual = clock !== systemClock;
  const views = createViews({ db, runtime: (id) => runtime(id), trust });
  let stopped = false;

  events.setMaxListeners(0);

  function runtime(id) {
    if (!runtimes.has(id)) runtimes.set(id, { id, state: "stopped", code: null, message: "", generation: 0, conn: null, tools: [], instructions: "", timer: null });

    return runtimes.get(id);
  }

  const tools = (list) => {
    try {
      return ListToolsResultSchema.parse({ tools: list }).tools;
    } catch {
      return list;
    }
  };

  // what Rox hears of a plugin: a change of these (session answers carry a
  // snapshot of an unchanged plugin, which is no event: Rox reloads a
  // plugin's cards on each one)
  function tell(e) {
    const key = JSON.stringify([e.state, e.code, e.generation, e.enabled]);

    if (said.get(e.id) === key) return;
    said.set(e.id, key);
    events.emit("plugin", e);
  }

  // a snapshot from the line; `announce`: tell listeners (Rox's events)
  function take(s, announce) {
    if (s.seq <= (seqs.get(s.id) ?? 0)) return;
    seqs.set(s.id, s.seq);

    if (s.state === "uninstalled") {
      runtimes.delete(s.id);

      if (announce) tell({ id: s.id, state: s.state, code: null, generation: s.generation, enabled: false });

      return;
    }

    const rt = runtime(s.id);
    const restart = manual && s.timer && s.timer !== rt.timer;

    Object.assign(rt, {
      state: s.state, code: s.code, message: s.message, generation: s.generation, tools: tools(s.tools), instructions: s.instructions,
      conn: s.state === "running" && s.pid ? { transport: { pid: s.pid } } : null, timer: s.timer,
    });

    if (s.next_delay_ms != null) rt.nextDelay = s.next_delay_ms;

    if (restart) clock.setTimeout(() => post("clock", { now: clock.now(), fire: true }).catch(() => {}), s.next_delay_ms);

    if (announce) tell({ id: s.id, state: s.state, code: s.code, generation: s.generation, enabled: s.enabled });
  }

  const address = Promise.resolve(base);

  async function request(path, init = {}) {
    return fetch(`${await address}/v1/line/${path}`, { ...init, headers: { "content-type": "application/json", "x-buwiz-line": token, ...init.headers } });
  }

  async function post(path, body = {}) {
    await ready;
    const out = await (await request(path, { method: "POST", body: JSON.stringify(body) })).json();

    if (out.runtime) {
      take(out.runtime, true);
      delete out.runtime;
    }

    if (out.ok === false && "retryable" in out) throw new V1Error(out.code, out.message, out.details);

    return out;
  }

  // the line's events, for as long as this lives
  let signalReady;
  const ready = new Promise((r) => (signalReady = r));

  async function listen() {
    while (!stopped) {
      try {
        const res = await request("events");
        const decoder = new TextDecoder();
        let buf = "";

        for await (const chunk of res.body) {
          buf += decoder.decode(chunk, { stream: true });
          let cut;

          while ((cut = buf.indexOf("\n\n")) >= 0) {
            const block = buf.slice(0, cut);

            buf = buf.slice(cut + 2);
            const name = /^event: (.*)$/m.exec(block)?.[1];
            const data = /^data: (.*)$/m.exec(block)?.[1];

            if (name === "ready") signalReady();
            else if (name === "snapshot" || name === "plugin") take(JSON.parse(data), name === "plugin");
          }
        }
      } catch {}

      if (!stopped) await new Promise((r) => setTimeout(r, 500));
    }
  }

  listen();

  if (manual) address.then(() => request("clock", { method: "POST", body: JSON.stringify({ now: clock.now() }) })).catch(() => {});

  const action = (id, what, body) => post(`plugins/${encodeURIComponent(id)}/${what}`, body);

  async function callTool(id, name, args, { meta, timeout = T.call } = {}) {
    const { result } = await post("call", { plugin: id, tool: name, args, ...(meta ? { meta } : {}), timeout_ms: timeout });

    try {
      return CallToolResultSchema.parse(result);
    } catch (e) {
      throw new V1Error("internal", `${name}: ${String(e?.message || e).slice(0, 300)}`);
    }
  }

  async function readResource(id, uri) {
    const { result } = await post("resource", { plugin: id, uri });

    return ReadResourceResultSchema.parse(result);
  }

  return {
    ...views, events, runtime, trust, timeouts: T, clock, callTool, readResource,
    inspect: (body) => post("plugins/inspect", body),
    install: (body) => post("plugins/install", body),
    enable: (id) => action(id, "enable"),
    disable: (id) => action(id, "disable"),
    uninstall: (id) => action(id, "uninstall"),
    setTool: (id, tool, disabled) => action(id, "tools", { tool, disabled }),
    callReserved: async (id, name, args) => (await post("reserved", { plugin: id, name, args })).value,
    fail: (id, code, message) => action(id, "fail", { code, message }),
    startAll: () => post("start"),
    stopAll: async () => {
      await post("stop").catch(() => {});
      stopped = true;
    },
    /** One line request (sessions-line.mjs): its answer, or a V1Error. */
    line: post,
  };
}
