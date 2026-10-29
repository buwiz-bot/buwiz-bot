// agents-external.mjs — agents and the world outside Rox (drafts/agent-
// messaging.md §10). An endpoint belongs to one agent and is a participant
// `ext:<id>` in its conversations:
// - kind "hook": an incoming webhook, POST /v1/hooks/<id>/<secret> with
//   `X-Rox-Signature: t=<unix s>,v1=<hex HMAC-SHA256(secret, "<t>.<body>")>`;
//   what the agent sends back goes to its `url` when it has one (signed
//   with `out_secret`, retried on 5xx for 24 h, dropped on 4xx), else it is
//   held for nobody to read and dropped after 24 h.
// - kind "bridge": a Claude Code session through rox-bridge (bridge/), with
//   the endpoint's secret as its bearer token; what the agent sends back is
//   held until the bridge receives it.
// Secrets live in the broker's database: the host's own store, as the
// host token is.
import { createHash, createHmac, randomBytes, timingSafeEqual } from "node:crypto";

export const HOOK_RATE = 60; // per hour per hook
export const SKEW_S = 300;
export const BODY_MAX = 512 * 1024;
const DAY = 24 * 3600_000;

const sha = (s) => createHash("sha256").update(s).digest("hex");
const same = (a, b) => {
  const x = Buffer.from(String(a)), y = Buffer.from(String(b));

  return x.length === y.length && timingSafeEqual(x, y);
};

/** `X-Rox-Signature` for `body` at unix second `t`. */
export const signature = (secret, body, t = Math.floor(Date.now() / 1000)) => `t=${t},v1=${createHmac("sha256", secret).update(`${t}.${body}`).digest("hex")}`;

export class ExternalError extends Error {
  constructor(status, code, message) {
    super(message);
    this.status = status;
    this.code = code;
  }
}

const refuse = (status, code, message) => {
  throw new ExternalError(status, code, message);
};

export function createExternals({ db, agents, now = () => Date.now(), post = defaultPost, log = () => {}, setTimer = setTimeout, save = () => {} }) {
  db.exec(`
    CREATE TABLE IF NOT EXISTS agent_endpoints(
      id TEXT PRIMARY KEY, bot TEXT NOT NULL, kind TEXT NOT NULL, name TEXT NOT NULL,
      secret TEXT NOT NULL, url TEXT, out_secret TEXT, created_at INTEGER NOT NULL);
    CREATE TABLE IF NOT EXISTS agent_hook_seen(
      endpoint TEXT NOT NULL, id TEXT NOT NULL, hash TEXT NOT NULL, answer TEXT NOT NULL, at INTEGER NOT NULL,
      PRIMARY KEY(endpoint, id));
    CREATE TABLE IF NOT EXISTS agent_outbox(
      seq INTEGER PRIMARY KEY AUTOINCREMENT, endpoint TEXT NOT NULL, msg TEXT NOT NULL, at INTEGER NOT NULL,
      state TEXT NOT NULL, attempts INTEGER NOT NULL DEFAULT 0, next_at INTEGER);
  `);
  const hits = new Map(); // hook id → call times, for the rate limit
  const waiting = new Set(); // bridge long-polls: {endpoint, wake}
  const row = (id) => db.prepare("SELECT * FROM agent_endpoints WHERE id=?").get(String(id || ""));
  const pub = (r) => ({ id: r.id, bot: r.bot, kind: r.kind, name: r.name, ...(r.url ? { url: r.url } : {}), created_at: r.created_at });

  /** For agents.mjs: the endpoint as a participant. */
  const get = (id) => {
    const r = row(id);

    return r ? { id: r.id, name: r.name, bot: r.bot } : null;
  };

  /** A new endpoint for agent `bot`; its secrets are answered only here. */
  function create({ bot, kind, name, url }) {
    if (!["hook", "bridge"].includes(kind)) refuse(400, "invalid_args", 'kind is "hook" or "bridge"');
    const label = String(name || "").trim().slice(0, 60);

    if (!label) refuse(400, "invalid_args", "name is required");

    if (!agents.party(String(bot || ""))) refuse(400, "invalid_args", "no such agent");

    if (url !== undefined && !/^https?:\/\/[^\s]+$/.test(String(url))) refuse(400, "invalid_args", "url is an http(s) URL");
    const id = `${kind === "hook" ? "ah" : "ab"}_${randomBytes(9).toString("base64url")}`;
    const secret = randomBytes(24).toString("base64url");
    const outSecret = url ? randomBytes(24).toString("base64url") : null;

    db.prepare("INSERT INTO agent_endpoints(id, bot, kind, name, secret, url, out_secret, created_at) VALUES(?,?,?,?,?,?,?,?)").run(id, bot, kind, label, secret, url || null, outSecret, now());

    return { ...pub(row(id)), secret, ...(outSecret ? { out_secret: outSecret } : {}), ...(kind === "hook" ? { path: `/v1/hooks/${id}/${secret}` } : {}) };
  }

  const list = (bot) => db.prepare("SELECT * FROM agent_endpoints" + (bot ? " WHERE bot=?" : "") + " ORDER BY created_at").all(...(bot ? [bot] : [])).map(pub);

  function rotate(id) {
    if (!row(id)) refuse(400, "invalid_args", "no such endpoint");
    const secret = randomBytes(24).toString("base64url");

    db.prepare("UPDATE agent_endpoints SET secret=? WHERE id=?").run(secret, id);

    return { id, secret };
  }

  function remove(id) {
    db.prepare("DELETE FROM agent_endpoints WHERE id=?").run(String(id || ""));
    db.prepare("DELETE FROM agent_outbox WHERE endpoint=?").run(String(id || ""));
  }

  /** POST /v1/hooks/<id>/<secret>: a signed envelope from outside (§10.1)
   *  → {id, conversation}. Refusals are ExternalErrors with their status. */
  function incoming(id, secret, raw, header) {
    const r = row(id);

    if (!r || r.kind !== "hook" || !same(r.secret, secret)) refuse(404, "not_found", "no such hook");

    if (raw.length > BODY_MAX) refuse(413, "too_large", "the body is over 512 KB");
    const m = /^t=(\d+),v1=([0-9a-f]{64})$/.exec(String(header || ""));

    if (!m || Math.abs(now() / 1000 - Number(m[1])) > SKEW_S || !same(signature(r.secret, raw, m[1]), `t=${m[1]},v1=${m[2]}`))
      refuse(401, "bad_signature", "X-Rox-Signature is missing, wrong or older than 5 minutes");
    const times = (hits.get(id) || []).filter((x) => x > now() - 3600_000);

    hits.set(id, times);

    if (times.length >= HOOK_RATE) refuse(429, "rate_limited", `at most ${HOOK_RATE} calls an hour`);
    times.push(now());
    let env;

    try {
      env = JSON.parse(raw);
    } catch {
      refuse(400, "invalid_args", "the body is not JSON");
    }

    if (!env || typeof env !== "object" || Array.isArray(env)) refuse(400, "invalid_args", "the body is an envelope object");

    if (env.v !== 1) refuse(400, "unsupported_version", "v must be 1");

    if (env.id !== undefined && !/^msg_[A-Za-z0-9_-]{8,80}$/.test(String(env.id))) refuse(400, "invalid_args", "id is msg_ and 8 to 80 letters, digits, _ or -");
    db.prepare("DELETE FROM agent_hook_seen WHERE at < ?").run(now() - DAY);
    const hash = sha(raw);
    const seen = env.id && db.prepare("SELECT * FROM agent_hook_seen WHERE endpoint=? AND id=?").get(id, env.id);

    if (seen) {
      if (seen.hash !== hash) refuse(409, "replayed", "this id came with another body in the last 24 hours");

      return JSON.parse(seen.answer);
    }
    let answer;

    try {
      const sent = agents.sendFrom(get(id), {
        to: env.to, conversation: env.conversation, in_reply_to: env.in_reply_to, subject: env.subject, text: env.text,
        data: env.data, attachments: env.attachments, expects_reply: env.expects_reply, deadline: env.deadline,
      }, { id: env.id });

      answer = { id: sent.id, conversation: sent.conversation };
    } catch (e) {
      refuse(e.code === "rate_limited" ? 429 : 400, e.code || "invalid_args", e.message);
    }

    if (env.id) db.prepare("INSERT INTO agent_hook_seen(endpoint, id, hash, answer, at) VALUES(?,?,?,?,?)").run(id, env.id, hash, JSON.stringify(answer), now());

    return answer;
  }

  /** A message an agent sent to endpoint `p` (its participant): to its url,
   *  or held for the bridge (§10.2, §10.3). */
  function deliver(p, msg) {
    const r = row(String(p.id).replace(/^ext:/, ""));

    if (!r) return log(`agents: ${msg.id} to a removed endpoint ${p.id}`);
    const res = db.prepare("INSERT INTO agent_outbox(endpoint, msg, at, state, next_at) VALUES(?,?,?,?,?)").run(r.id, JSON.stringify(msg), now(), r.url ? "queued" : "held", r.url ? now() : null);

    if (r.url) send(Number(res.lastInsertRowid));
    else for (const w of [...waiting]) if (w.endpoint === r.id) w.wake();
  }

  // one outgoing attempt: 2xx delivered, 4xx dropped (its sender told), 5xx
  // or no answer retried with backoff until a day has passed
  async function send(seq) {
    const o = db.prepare("SELECT * FROM agent_outbox WHERE seq=?").get(seq);
    const r = o && row(o.endpoint);

    if (!o || o.state !== "queued" || !r?.url) return;
    let status = 0;

    try {
      status = await post(r.url, o.msg, { "content-type": "application/json", "x-rox-signature": signature(r.out_secret, o.msg, Math.floor(now() / 1000)) });
    } catch {}
    const attempts = o.attempts + 1;

    if (status >= 200 && status < 300) return db.prepare("UPDATE agent_outbox SET state='delivered', attempts=? WHERE seq=?").run(attempts, seq);

    if ((status >= 400 && status < 500) || now() - o.at >= DAY) {
      db.prepare("UPDATE agent_outbox SET state='dropped', attempts=? WHERE seq=?").run(attempts, seq);

      return dropped(r, JSON.parse(o.msg), status);
    }
    const wait = Math.min(3600_000, 30_000 * 2 ** (attempts - 1));

    db.prepare("UPDATE agent_outbox SET attempts=?, next_at=? WHERE seq=?").run(attempts, now() + wait, seq);
    setTimer(() => send(seq).catch(() => {}), wait);
  }

  // the agent that sent it learns, in a notice in the conversation
  function dropped(r, msg, status) {
    const t = agents.conversation(msg.conversation);

    if (t) t.turns.push({ id: `drop-${msg.id}`, role: "notice", text: `Not delivered to ${r.name}${status ? ` (HTTP ${status})` : ""}`, notice: { kind: "agents_undelivered", endpoint: r.id, message: msg.id, at: now() } }), save();
    log(`agents: ${msg.id} to ${r.id} dropped (${status || "no answer"})`);
  }

  /** Outgoing messages a restart left queued, tried again. */
  function resume() {
    for (const o of db.prepare("SELECT seq FROM agent_outbox WHERE state='queued'").all()) send(o.seq).catch(() => {});
  }

  /** The bridge with token `token`, or a 401. */
  function bridge(token) {
    const r = token && db.prepare("SELECT * FROM agent_endpoints WHERE kind='bridge'").all().find((x) => same(x.secret, token));

    if (!r) refuse(401, "unauthorized", "the bridge token is missing or wrong");

    return r;
  }

  /** `rox_receive`: what the bridge's agent sent it, oldest first, waiting
   *  up to `wait_s` (at most 30) for something when there is nothing yet. */
  async function receive(r, { wait_s = 0, conversation } = {}) {
    const take = () => {
      const rows = db.prepare("SELECT * FROM agent_outbox WHERE endpoint=? AND state='held' ORDER BY seq").all(r.id)
        .filter((o) => !conversation || JSON.parse(o.msg).conversation === conversation);

      for (const o of rows) db.prepare("UPDATE agent_outbox SET state='delivered' WHERE seq=?").run(o.seq);

      return rows.map((o) => JSON.parse(o.msg));
    };
    let got = take();
    const wait = Math.max(0, Math.min(30, Number(wait_s) || 0)) * 1000;

    if (!got.length && wait) {
      await new Promise((resolve) => {
        const w = { endpoint: r.id, wake: () => (waiting.delete(w), clearTimeout(w.timer), resolve()) };

        w.timer = setTimeout(w.wake, wait);
        waiting.add(w);
      });
      got = take();
    }

    return { messages: got };
  }

  return { get, create, list, rotate, remove, incoming, deliver, resume, bridge, receive };
}

async function defaultPost(url, body, headers) {
  const res = await fetch(url, { method: "POST", body, headers, signal: AbortSignal.timeout(15_000) });

  return res.status;
}
