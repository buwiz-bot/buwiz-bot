// v1/routes.mjs — the host-internal API Rox calls (`/v1/host/*`).
//
// Not part of the protocol (spec 14.3): the routes the host design (its
// section 4) fixes between Rox and this broker. Every request carries
// `X-Buwiz-Host: <token>`; the token is 32 random bytes the broker writes to
// <BUWIZ_BROKER_DATA_DIR>/host-token (0600) when it starts, and a request without
// it is answered 401 before anything else happens. The Origin and JSON
// guard (guard.mjs) has already run. Bodies are JSON; refusals are
// {ok:false, code, message, retryable} with the spec's codes. Bridge calls
// need Rox's session signature on top (sessions.mjs): the token alone opens
// sessions, it cannot make calls.
import { randomBytes, timingSafeEqual } from "node:crypto";
import { chmodSync, mkdirSync, writeFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { DATA_DIR } from "../config.mjs";
import { plugins, routines, sessions } from "./broker.mjs";
import { V1Error, errorBody } from "./errors.mjs";
import { AgentError } from "../agents.mjs";
import { agents, externals } from "../agents-wire.mjs";
import { ExternalError } from "../agents-external.mjs";
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
  const got = Buffer.from(String(req.headers["x-buwiz-host"] || ""));
  const want = Buffer.from(token || "");

  return !!token && got.length === want.length && timingSafeEqual(got, want);
}

const send = (res, status, obj) => {
  res.statusCode = status;
  res.setHeader("content-type", "application/json");
  res.end(JSON.stringify(obj));
};

const STATUS = { disabled: 409, not_found: 404, plugin_unavailable: 503, plugin_restarted: 503, timeout: 504, rate_limited: 429, internal: 500 };

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
  // v1 routines (5.9): the user's one-time approval of what one reads, and
  // the switch that runs it in a chat
  "/v1/host/routines/approve": (b) => routines.approve(b),
  "/v1/host/routines/enable": (b) => routines.enable(b),
  "/v1/host/routines/disable": (b) => routines.disable(b),
  // one run now, out of schedule (Run now)
  "/v1/host/routines/run": (b) => routines.run(b),
  // an instance's params, trigger, interval or label; its removal; a new
  // webhook secret
  "/v1/host/routines/update": (b) => routines.update(b),
  "/v1/host/routines/delete": (b) => routines.remove(b),
  "/v1/host/routines/hook": (b) => routines.hook(b),
  // agents' conversations (agents.mjs): the user archives one, resumes a
  // paused one, and says whom an agent takes messages from
  "/v1/host/agents/archive": agentsCall((b) => agents.archive(b.conversation, b.archived !== false)),
  "/v1/host/agents/resume": agentsCall((b) => agents.resume(b.conversation)),
  "/v1/host/agents/accepts": agentsCall((b) => agents.setAccepts(b.bot, b.accepts)),
  // an agent's outside endpoints (agents-external.mjs): a webhook or a
  // Claude Code bridge; the secrets are answered only when made or rotated
  "/v1/host/agents/endpoints/create": externalCall((b) => externals.create(b)),
  "/v1/host/agents/endpoints/list": externalCall((b) => ({ ok: true, endpoints: externals.list(b.bot) })),
  "/v1/host/agents/endpoints/rotate": externalCall((b) => externals.rotate(b.id)),
  "/v1/host/agents/endpoints/delete": externalCall((b) => (externals.remove(b.id), { ok: true })),
};

/** An endpoints call as a host route, its refusal as a V1Error. */
function externalCall(run) {
  return (b) => {
    try {
      return { ok: true, ...run(b) };
    } catch (e) {
      throw e instanceof ExternalError ? new V1Error(e.code, e.message) : e;
    }
  };
}

/** An agents call as a host route: {ok: true}, or its refusal as a V1Error. */
function agentsCall(run) {
  return (b) => {
    try {
      run(b);

      return { ok: true };
    } catch (e) {
      throw e instanceof AgentError ? new V1Error(e.code, e.message) : e;
    }
  };
}

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

// POST /v1/hooks/<instance>/<secret>: a webhook instance's run (5.9). No
// host token: the secret in the path is the key. 202 once it started.
async function hookRoute(req, res, path) {
  const m = /^\/v1\/hooks\/([^/]+)\/([^/]+)$/.exec(path);

  if (!m || req.method !== "POST") return send(res, 404, { ok: false, code: "not_found", message: "no such hook", retryable: false });

  // an agent's webhook (ah_…): a signed message, read as its raw bytes
  if (m[1].startsWith("ah_")) {
    const raw = await rawBody(req);

    try {
      return send(res, 202, { ok: true, ...externals.incoming(decodeURIComponent(m[1]), decodeURIComponent(m[2]), raw, req.headers["x-buwiz-signature"]) });
    } catch (e) {
      if (!(e instanceof ExternalError)) throw e;

      return send(res, e.status, { ok: false, code: e.code, message: e.message, retryable: e.status === 429 });
    }
  }
  const body = (await readBody(req)) || {};

  try {
    send(res, 202, await routines.hookCall(decodeURIComponent(m[1]), decodeURIComponent(m[2]), body));
  } catch (e) {
    const err = errorBody(e);

    send(res, STATUS[err.code] || 400, err);
  }
}

/** The request body as text; past 512 KB, a string one byte too long. */
function rawBody(req) {
  return new Promise((resolve) => {
    const chunks = [];
    let size = 0;

    req.on("data", (c) => {
      size += c.length;

      if (size <= 512 * 1024) chunks.push(c);
    });
    req.on("end", () => resolve(size > 512 * 1024 ? "x".repeat(512 * 1024 + 1) : Buffer.concat(chunks).toString("utf8")));
  });
}

// /v1/bridge/*: rox-bridge (bridge/rox-bridge.mjs), a Claude Code
// session's way in, with its endpoint's token as `Authorization: Bearer`
async function bridgeRoute(req, res, path) {
  try {
    const r = externals.bridge(String(req.headers.authorization || "").replace(/^Bearer\s+/i, ""));
    const me = agents.party(`ext:${r.id}`);
    const body = req.method === "POST" ? (await readBody(req)) || {} : {};

    if (path === "/v1/bridge/agents") {
      const b = agents.party(r.bot);

      return send(res, 200, { ok: true, agents: b ? [{ id: b.id, name: b.name, description: b.purpose || b.description || "" }] : [] });
    }

    if (path === "/v1/bridge/send" && req.method === "POST") return send(res, 202, { ok: true, ...agents.sendFrom(externals.get(r.id), body, { id: body.id }) });

    if (path === "/v1/bridge/receive" && req.method === "POST") return send(res, 200, { ok: true, ...(await externals.receive(r, body)) });

    if (path === "/v1/bridge/conversations") return send(res, 200, { ok: true, conversations: agents.conversations(me, body) });

    return send(res, 404, { ok: false, code: "not_found", message: `no route ${path}`, retryable: false });
  } catch (e) {
    const status = e instanceof ExternalError ? e.status : e instanceof AgentError ? 400 : 500;

    return send(res, status, { ok: false, code: e.code || "internal", message: String(e.message).slice(0, 300), retryable: false });
  }
}

export default async function handle(req, res) {
  const path = new URL(req.url || "/", "http://x").pathname;

  if (path.startsWith("/v1/hooks/")) {
    await hookRoute(req, res, path);

    return true;
  }

  if (path.startsWith("/v1/bridge/")) {
    await bridgeRoute(req, res, path);

    return true;
  }

  if (!path.startsWith("/v1/host/")) return false;

  if (!authorized(req)) return send(res, 401, { ok: false, code: "unauthorized", message: "X-Buwiz-Host is missing or wrong", retryable: false });
  await answer(req, res, path);

  return true;
}
