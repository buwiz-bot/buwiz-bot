// v1/routes.mjs — the host-internal API Rox calls (`/v1/host/*`).
//
// Not part of the protocol (spec 14.3): the routes the host design (its
// section 4) fixes between Rox and this broker. Every request carries
// `X-Rox-Host: <token>`; the token is 32 random bytes the broker writes to
// <GROK_DATA_DIR>/host-token (0600) when it starts, and a request without
// it is answered 401 before anything else happens. The Origin and JSON
// guard (guard.mjs) has already run. Bodies are JSON; refusals are
// {ok:false, code, message, retryable} with the spec's codes. Bridge calls
// need Rox's session signature on top (sessions.mjs): the token alone opens
// sessions, it cannot make calls.
import { randomBytes, timingSafeEqual } from "node:crypto";
import { chmodSync, mkdirSync, writeFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { DATA_DIR } from "../config.mjs";
import { plugins, sessions } from "./broker.mjs";
import { errorBody } from "./errors.mjs";
import { isObject } from "./jcs.mjs";

export const HOST_TOKEN_FILE = join(DATA_DIR, "host-token");
const BODY_MAX = 2 * 1024 * 1024;
let token = null;

/** A fresh token in <data dir>/host-token (0600); called once at start. */
export function initHostToken(file = HOST_TOKEN_FILE) {
  token = randomBytes(32).toString("base64url");
  mkdirSync(dirname(file), { recursive: true });
  writeFileSync(file, token, { mode: 0o600 });
  chmodSync(file, 0o600);

  return file;
}

function authorized(req) {
  const got = Buffer.from(String(req.headers["x-rox-host"] || ""));
  const want = Buffer.from(token || "");

  return !!token && got.length === want.length && timingSafeEqual(got, want);
}

const send = (res, status, obj) => {
  res.statusCode = status;
  res.setHeader("content-type", "application/json");
  res.end(JSON.stringify(obj));
};

const STATUS = { not_found: 404, plugin_unavailable: 503, plugin_restarted: 503, timeout: 504, rate_limited: 429, internal: 500 };

function readBody(req) {
  return new Promise((resolve) => {
    const chunks = [];
    let size = 0;

    req.on("data", (c) => {
      size += c.length;

      if (size <= BODY_MAX) chunks.push(c);
    });
    req.on("end", () => {
      if (size > BODY_MAX) return resolve(null);

      try {
        const v = JSON.parse(Buffer.concat(chunks).toString("utf8") || "{}");

        resolve(isObject(v) ? v : null);
      } catch {
        resolve(null);
      }
    });
  });
}

// GET /v1/host/events: plugin state and generation changes, as SSE
function events(req, res) {
  res.writeHead(200, { "content-type": "text/event-stream", "cache-control": "no-cache", connection: "keep-alive" });
  res.write(": rox broker events\n\n");
  const on = (e) => res.write(`event: plugin\ndata: ${JSON.stringify(e)}\n\n`);
  const beat = setInterval(() => res.write(": ping\n\n"), 25_000);

  plugins.events.on("plugin", on);
  req.on("close", () => {
    clearInterval(beat);
    plugins.events.off("plugin", on);
  });
}

const POSTS = {
  "/v1/host/plugins/inspect": (b) => plugins.inspect(b),
  "/v1/host/plugins/install": (b) => plugins.install(b),
  "/v1/host/apps/source": (b) => sessions.appSource(b),
  "/v1/host/sessions/open": (b) => sessions.open(b),
  "/v1/host/sessions/close": (b) => sessions.close(b),
  "/v1/host/call": (b) => sessions.call(b),
  "/v1/host/tickets/title": (b) => sessions.ticketTitle(b),
  "/v1/host/resolve": (b) => sessions.resolve(b),
};

const ACTIONS = {
  enable: (id) => plugins.enable(id),
  disable: (id) => plugins.disable(id),
  uninstall: (id) => plugins.uninstall(id),
  tools: (id, b) => plugins.setTool(id, b.tool, b.disabled),
};

async function answer(req, res, path) {
  if (req.method === "GET" && path === "/v1/host/plugins") return send(res, 200, { ok: true, plugins: plugins.list() });

  if (req.method === "GET" && path === "/v1/host/events") return events(req, res);

  if (req.method !== "POST") return send(res, 405, { ok: false, code: "not_supported", message: `${req.method} ${path}`, retryable: false });
  const action = /^\/v1\/host\/plugins\/([^/]+)\/(enable|disable|uninstall|tools)$/.exec(path);
  const run = POSTS[path] || (action && ((b) => ACTIONS[action[2]](decodeURIComponent(action[1]), b)));

  if (!run) return send(res, 404, { ok: false, code: "not_found", message: `no route ${path}`, retryable: false });
  const body = await readBody(req);

  if (!body) return send(res, 400, { ok: false, code: "invalid_args", message: "the body must be a JSON object (at most 2 MiB)", retryable: false });

  try {
    send(res, 200, await run(body));
  } catch (e) {
    const err = errorBody(e);

    if (err.code === "internal") console.error(`  v1 ${path}: ${err.message.slice(0, 200)}`);
    send(res, STATUS[err.code] || 400, err);
  }
}

export default async function handle(req, res) {
  const path = new URL(req.url || "/", "http://x").pathname;

  if (!path.startsWith("/v1/host/")) return false;

  if (!authorized(req)) return send(res, 401, { ok: false, code: "unauthorized", message: "X-Rox-Host is missing or wrong", retryable: false });
  await answer(req, res, path);

  return true;
}
