/* The v1 bridge's JS half (spec 7, design §2), prepended by Rox to an app's
   compiled module after its imports were checked. It builds the frozen
   global `rox` with only the methods the host granted (absent otherwise),
   turns the host's rejections into RoxError, and removes the network
   globals. Every check that matters is made again in Rust: this file only
   shapes the API. */
import { host as __roxHost, signal as __roxSignal } from "nsh";
const __roxBridge = (() => {
  "use strict";
  const params = __roxHost.params || {};
  const granted = Array.isArray(params.granted) ? params.granted : [];
  const has = (name) => granted.indexOf(name) >= 0;
  class RoxError extends Error {
    constructor(code, message, retryable, details) {
      super(message || code);
      this.name = "RoxError";
      this.code = code;
      this.retryable = retryable === true;
      if (details !== undefined) this.details = details;
    }
  }
  const toRoxError = (e) => {
    let o = null;
    try {
      o = JSON.parse(String(e && e.message));
    } catch (_) {
      o = null;
    }
    if (o && typeof o.code === "string") return new RoxError(o.code, o.message, o.retryable, o.details);
    return new RoxError("internal", String((e && e.message) || e), true);
  };
  const ask = (name, data) =>
    __roxHost.call(name, data === undefined ? null : data).then(undefined, (e) => {
      throw toRoxError(e);
    });
  const done = () => undefined;
  const freeze = (v) => {
    if (v && typeof v === "object" && !Object.isFrozen(v)) {
      Object.freeze(v);
      for (const k of Object.keys(v)) freeze(v[k]);
    }
    return v;
  };
  const viewport = __roxSignal(freeze(params.viewport || { width: 0, height: 0, placement: "full" }));
  __roxHost.on("rox.viewport", (v) => {
    if (v && typeof v === "object") viewport.set(freeze(v));
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
      if (v < 0) throw new RoxError("internal", "the host sent bad file bytes", true);
      acc = (acc << 6) | v;
      bits += 6;
      if (bits >= 8) {
        bits -= 8;
        out[o++] = (acc >> bits) & 255;
      }
    }
    return out;
  };
  const rox = {
    props: freeze(params.props || {}),
    app: freeze(params.app || {}),
    viewport: () => viewport(),
    ticket: Object.freeze({ title: (ticket) => ask("rox.ticket.title", { ticket }) }),
    // an inline card's content height in points: the host sizes the card
    // to it within the manifest's inline range (full screen: ignored)
    fit: (height) => {
      const h = Number(height);
      if (!(h > 0) || !Number.isFinite(h)) throw new RoxError("invalid_args", "rox.fit takes a height in points");
      __roxHost.post("rox.fit", { height: Math.ceil(h) });
    },
  };
  if (has("rox.call")) rox.call = (tool, args) => ask("rox.call", { tool, args: args === undefined ? {} : args });
  if (has("rox.open")) rox.open = (target) => ask("rox.open", target).then(done);
  if (has("rox.attach")) rox.attach = (scope, value) => ask("rox.attach", { scope, value });
  if (has("rox.pickFile")) {
    rox.pickFile = (options) =>
      ask("rox.pickFile", options === undefined ? {} : options).then((files) =>
        files.map((f) =>
          Object.freeze({
            name: f.name,
            size: f.size,
            type: f.type,
            read: () => ask("rox.pickFile", { read: f.id }).then(bytes),
          }),
        ),
      );
  }
  if (has("rox.storage.get")) {
    rox.storage = Object.freeze({
      get: (key) => ask("rox.storage.get", { key }),
      set: (key, value) => ask("rox.storage.set", { key, value }).then(done),
      remove: (key) => ask("rox.storage.remove", { key }).then(done),
      keys: () => ask("rox.storage.keys", null),
    });
  }
  if (has("rox.copy")) rox.copy = (text) => ask("rox.copy", { text: String(text) }).then(done);
  if (has("rox.preview")) rox.preview = (options) => ask("rox.preview", options === undefined ? {} : options).then(done);
  return { rox: Object.freeze(rox), RoxError };
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
Object.defineProperty(globalThis, "rox", { value: __roxBridge.rox, writable: false, configurable: false });
Object.defineProperty(globalThis, "RoxError", { value: __roxBridge.RoxError, writable: false, configurable: false });
