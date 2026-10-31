/* The v1 bridge's JS half (spec 7, design §2), prepended by Rox to an app's
   compiled module after its imports were checked. It builds the frozen
   global `rox` with only the methods the host granted (absent otherwise),
   turns the host's rejections into BuwizError, and removes the network
   globals. Every check that matters is made again in Rust: this file only
   shapes the API. */
import { host as __buwizHost, signal as __buwizSignal } from "nsh";
const __buwizBridge = (() => {
  "use strict";
  const params = __buwizHost.params || {};
  const granted = Array.isArray(params.granted) ? params.granted : [];
  const has = (name) => granted.indexOf(name) >= 0;
  class BuwizError extends Error {
    constructor(code, message, retryable, details) {
      super(message || code);
      this.name = "BuwizError";
      this.code = code;
      this.retryable = retryable === true;
      if (details !== undefined) this.details = details;
    }
  }
  const toBuwizError = (e) => {
    let o = null;
    try {
      o = JSON.parse(String(e && e.message));
    } catch (_) {
      o = null;
    }
    if (o && typeof o.code === "string") return new BuwizError(o.code, o.message, o.retryable, o.details);
    return new BuwizError("internal", String((e && e.message) || e), true);
  };
  const ask = (name, data) =>
    __buwizHost.call(name, data === undefined ? null : data).then(undefined, (e) => {
      throw toBuwizError(e);
    });
  const done = () => undefined;
  const freeze = (v) => {
    if (v && typeof v === "object" && !Object.isFrozen(v)) {
      Object.freeze(v);
      for (const k of Object.keys(v)) freeze(v[k]);
    }
    return v;
  };
  const viewport = __buwizSignal(freeze(params.viewport || { width: 0, height: 0, placement: "full" }));
  __buwizHost.on("buwiz.viewport", (v) => {
    if (v && typeof v === "object") viewport.set(freeze(v));
  });
  // presses of the card's footer buttons (buwiz.actions): the id pressed
  const pressed = new Set();
  __buwizHost.on("buwiz.action", (e) => {
    if (e && typeof e.id === "string") for (const f of [...pressed]) f(e.id);
  });
  const B64 = "ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
  const table = new Int16Array(128).fill(-1);
  for (let i = 0; i < 64; i++) table[B64.charCodeAt(i)] = i;
  const bytes = (text) => {
    const s = String(text);
    let n = s.length;
    while (n > 0 && s.charCodeAt(n - 1) === 61) n--;
    const out = new Uint8Array(Math.floor((n * 3) / 4));
    let acc = 0;
    let bits = 0;
    let o = 0;
    for (let i = 0; i < n; i++) {
      const v = table[s.charCodeAt(i) & 127];
      if (v < 0) throw new BuwizError("internal", "the host sent bad file bytes", true);
      acc = (acc << 6) | v;
      bits += 6;
      if (bits >= 8) {
        bits -= 8;
        out[o++] = (acc >> bits) & 255;
      }
    }
    return out;
  };
  const buwiz = {
    props: freeze(params.props || {}),
    app: freeze(params.app || {}),
    viewport: () => viewport(),
    ticket: Object.freeze({ title: (ticket) => ask("buwiz.ticket.title", { ticket }) }),
    // an inline card's content height in points: the host sizes the card
    // to it within the manifest's inline range (full screen: ignored)
    fit: (height) => {
      const h = Number(height);
      if (!(h > 0) || !Number.isFinite(h)) throw new BuwizError("invalid_args", "buwiz.fit takes a height in points");
      __buwizHost.post("buwiz.fit", { height: Math.ceil(h) });
    },
    // an inline card's data is in: with the manifest's inline.wait_ready
    // the card holds its placeholder until this (full screen: ignored)
    ready: () => {
      __buwizHost.post("buwiz.ready", {});
    },
    // an inline card's chrome, drawn by Rox (full screen: checked, then
    // ignored): footer buttons, the outcome line it settles to, a status
    actions: (list) => ask("buwiz.actions", { actions: list === undefined ? null : list }).then(done),
    onAction: (f) => {
      if (typeof f !== "function") throw new BuwizError("invalid_args", "buwiz.onAction takes a function");
      pressed.add(f);
      return () => {
        pressed.delete(f);
      };
    },
    settle: (text) => ask("buwiz.settle", { text: text === undefined ? null : text }).then(done),
    meta: (meta) => ask("buwiz.meta", { meta: meta === undefined ? null : meta }).then(done),
  };
  if (has("buwiz.call")) buwiz.call = (tool, args) => ask("buwiz.call", { tool, args: args === undefined ? {} : args });
  if (has("buwiz.open")) buwiz.open = (target) => ask("buwiz.open", target).then(done);
  if (has("buwiz.attach")) buwiz.attach = (scope, value) => ask("buwiz.attach", { scope, value });
  if (has("buwiz.pickFile")) {
    buwiz.pickFile = (options) =>
      ask("buwiz.pickFile", options === undefined ? {} : options).then((files) =>
        files.map((f) =>
          Object.freeze({
            name: f.name,
            size: f.size,
            type: f.type,
            read: () => ask("buwiz.pickFile", { read: f.id }).then(bytes),
          }),
        ),
      );
  }
  if (has("buwiz.storage.get")) {
    buwiz.storage = Object.freeze({
      get: (key) => ask("buwiz.storage.get", { key }),
      set: (key, value) => ask("buwiz.storage.set", { key, value }).then(done),
      remove: (key) => ask("buwiz.storage.remove", { key }).then(done),
      keys: () => ask("buwiz.storage.keys", null),
    });
  }
  if (has("buwiz.copy")) buwiz.copy = (text) => ask("buwiz.copy", { text: String(text) }).then(done);
  if (has("buwiz.preview")) buwiz.preview = (options) => ask("buwiz.preview", options === undefined ? {} : options).then(done);
  return { buwiz: Object.freeze(buwiz), BuwizError };
})();
for (const name of ["fetch", "XMLHttpRequest", "WebSocket", "EventSource", "localStorage", "sessionStorage"]) {
  try {
    delete globalThis[name];
  } catch (_) {}
  if (globalThis[name] !== undefined) {
    try {
      Object.defineProperty(globalThis, name, { value: undefined, writable: false, configurable: false });
    } catch (_) {}
  }
}
Object.defineProperty(globalThis, "buwiz", { value: __buwizBridge.buwiz, writable: false, configurable: false });
Object.defineProperty(globalThis, "BuwizError", { value: __buwizBridge.BuwizError, writable: false, configurable: false });
