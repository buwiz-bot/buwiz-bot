// attachments.mjs — `@` tokens attached to a chat or a bot, and what they
// scope there. An attachment is a token a server's manifest lets attach
// (`scopes[].attach`), resolved once by the scope's `resolve` tool:
//   attachments(id, server, token, scope, title, open, key, details, thread,
//               bot, turn, created_at)
// `turn` is the id of the card message posted for it (an `mcp-app` link,
// the card `rox/open` replies carry), `key` the mini app its `open` link
// names (`mcp:<server>/<app>|<props>`, as pins store it). A rename is a
// display name per token (attachment_names), shared by every attachment
// and pin of it. While a chat has attachments, every resolve made for it
// carries `context: {attached}`, and the agent's system prompt lists them
// with the hints of the scopes in play (scopeSection).
import { db, findThread, store, threadBot, uuidv7, writeIndex } from "./state.mjs";
import { appKey, callToolJson, readManifest } from "./mcp.mjs";
import { parseToken, tokensIn } from "./grammar.mjs";

const parseJson = (s, d) => {
  try { return JSON.parse(s); } catch { return d; }
};
const botOf = (threadId) => threadBot(threadId) || null;
const serverRow = (id) => db.prepare("SELECT * FROM mcp_servers WHERE id=? AND enabled=1").get(String(id || ""));
const scopesOf = (m) => (Array.isArray(m?.scopes) ? m.scopes.filter((x) => x && typeof x.name === "string") : []);

// `rox://app/<server>/<app>?props=<JSON>` → the mini app's key, or "".
function keyOfOpen(open) {
  const m = String(open || "").match(/^rox:\/\/app\/([a-z0-9-]+)\/([A-Za-z0-9._-]+)(?:\?props=([^&\s]*))?/);
  if (!m) return "";
  const props = m[3] ? parseJson(decodeURIComponent(m[3]), {}) : {};
  return appKey(m[1], m[2], props && typeof props === "object" ? props : {});
}

const names = () => new Map(db.prepare("SELECT token,title FROM attachment_names").all().map((r) => [r.token, r.title]));

// A row as the endpoints hand it out: `title` is the display name (the
// rename, else what the resolver said), `resolved_title` the latter.
function json(r, renamed = names()) {
  return {
    id: r.id,
    server: r.server,
    token: r.token,
    scope: r.scope,
    title: renamed.get(r.token) || r.title,
    resolved_title: r.title,
    open: r.open,
    key: r.key || "",
    details: parseJson(r.details, null),
    thread: r.thread,
    bot: r.bot || null,
    turn: r.turn || "",
    created_at: r.created_at,
  };
}

/** The attachments of chat `threadId` (a bot's are in its chat). */
export function attachmentsOf(threadId) {
  if (!threadId) return [];
  const renamed = names();
  return db.prepare("SELECT * FROM attachments WHERE thread=? ORDER BY created_at").all(String(threadId)).map((r) => json(r, renamed));
}

/** GET /mcp/attachments?thread=|bot=|token= — each with where it is. */
export function listAttachments({ thread, bot, token } = {}) {
  const where = [], args = [];
  if (thread) where.push("thread=?"), args.push(String(thread));
  if (bot) where.push("bot=?"), args.push(String(bot));
  if (token) where.push("token=?"), args.push(String(token));
  const rows = db.prepare(`SELECT * FROM attachments${where.length ? ` WHERE ${where.join(" AND ")}` : ""} ORDER BY created_at`).all(...args);
  const renamed = names();
  return rows.map((r) => {
    const a = json(r, renamed);
    const t = store.list.find((x) => x.id === a.thread);
    return { ...a, threadTitle: t?.title || "", botName: store.bots.find((b) => b.id === a.bot)?.name || "" };
  });
}

/** `POST /mcp/resolve {server, name, args, complete, thread?}` — a command
 *  or `@` scope resolved by the tool the server's manifest names for it;
 *  the tool's result as is, or {ok:false, error}. A chat with this
 *  server's tokens attached passes them as `context.attached`. */
export async function mcpResolve({ server, name, args, complete, thread }) {
  const row = serverRow(server);
  if (!row) return { ok: false, error: "no such server" };
  const m = await readManifest(row);
  const all = [...(Array.isArray(m.commands) ? m.commands : []), ...scopesOf(m)];
  const tool = all.find((x) => x?.name === name && x.resolve)?.resolve;
  if (!tool) return { ok: false, error: `${row.id} has no /${name}` };
  const t = thread ? findThread(String(thread)) : null;
  const attached = t ? attachmentsOf(t.id).filter((a) => a.server === row.id).map((a) => a.token) : [];
  const r = await callToolJson(row, String(tool), {
    name: String(name),
    args: String(args ?? ""),
    complete: !!complete,
    ...(attached.length ? { context: { attached } } : {}),
  });
  return r.ok ? r.result : r;
}

/** POST /mcp/attach {server, token, thread | bot} — resolves the token
 *  (with the chat's context), stores it and posts its card into the chat
 *  (a bot's: its own chat). Attaching a token the chat already has returns
 *  that attachment. */
export async function attach({ server, token, thread, bot }) {
  let t = thread ? findThread(String(thread)) : null;
  if (!t && bot) {
    const b = store.bots.find((x) => x.id === bot);
    if (!b) throw new Error(`unknown bot ${bot}`);
    t = findThread(b.threadId);
    if (!t) throw new Error(`${b.name} has no chat`);
  }
  if (!t) throw new Error(`unknown chat ${thread ?? ""}`);
  const tok = parseToken(token);
  if (!tok) throw new Error(`not a token: ${token}`);
  server = String(server || tok.server);
  const same = (tk) => db.prepare("SELECT * FROM attachments WHERE server=? AND token=? AND thread=?").get(server, tk, t.id);
  const had = same(String(token).trim());
  if (had) return { attachment: json(had), existing: true };
  const row = serverRow(server);
  if (!row) throw new Error(`no such server ${server}`);
  const scope = scopesOf(await readManifest(row)).find((s) => s.name === tok.scope);
  if (!scope?.attach) throw new Error(`@${server}:${tok.scope} tokens can't be attached`);
  const r = await mcpResolve({ server, name: tok.scope, args: tok.value, thread: t.id });
  if (!r?.ok) throw new Error(r?.error || "couldn't resolve the token");
  const canonical = String(r.token || token).trim();
  // the resolver's canonical form may be one the chat already has
  const dup = same(canonical);
  if (dup) return { attachment: json(dup), existing: true };
  const a = {
    id: uuidv7(),
    server,
    token: canonical,
    scope: tok.scope,
    title: String(r.title || canonical).slice(0, 120),
    open: String(r.open || ""),
    key: keyOfOpen(r.open),
    details: r.details && typeof r.details === "object" ? JSON.stringify(r.details) : null,
    thread: t.id,
    bot: botOf(t.id)?.id || null,
    turn: uuidv7(),
    created_at: Date.now(),
  };
  // the card: its own message, the link card a `rox/open` reply carries
  t.turns.push({
    id: a.turn,
    role: "ai",
    text: "",
    label: "",
    links: [{
      provider: "mcp-app", id: a.key || a.open, name: a.title, link: a.open, action: "attached",
      kind: "app", file_type: scope.description || "Attached", token: a.token, attachment: a.id, at: a.created_at,
    }],
  });
  db.prepare(
    "INSERT INTO attachments(id,server,token,scope,title,open,key,details,thread,bot,turn,created_at) VALUES(?,?,?,?,?,?,?,?,?,?,?,?)",
  ).run(a.id, a.server, a.token, a.scope, a.title, a.open, a.key, a.details, a.thread, a.bot, a.turn, a.created_at);
  writeIndex();
  return { attachment: json(db.prepare("SELECT * FROM attachments WHERE id=?").get(a.id)), existing: false };
}

/** POST /mcp/detach {id} — the scope leaves the chat; its card stays in
 *  the history. */
export function detach(id) {
  // the Library item's id (`attach:<id>`) or the attachment's own
  id = String(id || "").replace(/^attach:/, "");
  const r = db.prepare("DELETE FROM attachments WHERE id=?").run(id);
  if (!r.changes) throw new Error(`no attachment ${id}`);
}

/** POST /mcp/attachment/rename {token, title} — every attachment and pin
 *  of the token shows `title`; "" goes back to the resolver's. */
export function renameToken(token, title) {
  token = String(token || "").trim();
  if (!token) throw new Error("rename needs a token");
  title = String(title ?? "").trim().slice(0, 120);
  if (title) db.prepare("INSERT OR REPLACE INTO attachment_names(token,title,updated_at) VALUES(?,?,?)").run(token, title, Date.now());
  else db.prepare("DELETE FROM attachment_names WHERE token=?").run(token);
}

/** A pin's display name: the rename of the token whose app it opens. */
export function renamedKeys() {
  return new Map(
    db.prepare("SELECT a.key AS key, n.title AS title FROM attachments a JOIN attachment_names n ON n.token=a.token WHERE a.key<>''")
      .all().map((r) => [r.key, r.title]),
  );
}

/** The agent's system prompt for chat `thread` with user message `prompt`:
 *  a line per attachment, then the `hint` of every scope that is attached
 *  or has a token in the message. "" when neither. */
export async function scopeSection(thread, prompt = "") {
  const atts = attachmentsOf(thread?.id);
  const used = new Map(); // server → Set(scope)
  const note = (server, scope) => (used.get(server) || used.set(server, new Set()).get(server)).add(scope);
  for (const a of atts) note(a.server, a.scope);
  for (const t of tokensIn(prompt)) note(t.server, t.scope);
  if (!used.size) return "";
  const lines = [], hints = [];
  for (const [server, scopes] of used) {
    const row = serverRow(server);
    const m = row ? await readManifest(row).catch(() => ({})) : {};
    const title = String(m.title || row?.name || server);
    const defs = scopesOf(m);
    for (const a of atts.filter((x) => x.server === server)) {
      // "Taxpayer by TIN" → "this taxpayer"
      const noun = String(defs.find((s) => s.name === a.scope)?.description || "").trim().split(/\s+/)[0]?.toLowerCase() || "item";
      lines.push(`Attached: ${a.token} (${a.title}) — ${title} tools default to this ${noun} here`);
    }
    for (const s of defs) if (scopes.has(s.name) && typeof s.hint === "string" && s.hint.trim()) hints.push(s.hint.trim());
  }
  return `\n\n## Scope of this chat\n${[...lines, ...hints].join("\n")}`;
}
