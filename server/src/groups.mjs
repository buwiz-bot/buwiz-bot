// groups.mjs — the sidebar's user-made groups. A group holds any mix of
// bots, chats and artifacts, in the user's order; an item may sit in
// several groups. Rows live in app.db and are written one change at a
// time (persist() rewrites bots/threads, never these):
//   groups      — id, name, color, ord, collapsed
//   group_items — (group_id, kind, ref): kind "bot" | "chat" | "artifact"
//                 | "app", ref the bot id, thread id, artifact id (R<n> |
//                 file:<path> | gdrive:<file id>) or a server's mini app
//                 (mcp:<server>/<app>|<props>, as rox://app/ links open it);
//                 thread is an artifact's origin chat, title an app's.
// Bot sections and the sidebar's pinned Mini Apps became groups once
// (migrateGroups); bot.section and /state/rox-pinned-apps stay readable
// but nothing writes them for the sidebar any more.
import { db, metaGet, metaSet, store } from "./state.mjs";
import { artifactRecords } from "./artifacts.mjs";
import { originOf } from "./appmeta.mjs";

db.exec(`
  CREATE TABLE IF NOT EXISTS groups(
    id TEXT PRIMARY KEY, name TEXT NOT NULL, color TEXT NOT NULL,
    ord INTEGER NOT NULL DEFAULT 0, collapsed INTEGER DEFAULT 0, created_at INTEGER);
  CREATE TABLE IF NOT EXISTS group_items(
    group_id TEXT NOT NULL, kind TEXT NOT NULL, ref TEXT NOT NULL, thread TEXT,
    ord INTEGER NOT NULL DEFAULT 0, PRIMARY KEY(group_id, kind, ref));
`);
// An app member's title (its server can't be asked when the list is drawn).
try { db.exec("ALTER TABLE group_items ADD COLUMN title TEXT"); } catch {}

// The color dots a group picks from (the sidebar's chat hues, then gray).
export const GROUP_COLORS = ["#1884fc", "#32bb78", "#f2994a", "#eb5757", "#9b51e0", "#2d9cdb", "#f2c94c", "#8e8e93"];
const KINDS = new Set(["bot", "chat", "artifact", "app"]);

const newId = () => `g${Date.now().toString(36)}${Math.floor(Math.random() * 46656).toString(36)}`;
const groupRow = (id) => db.prepare("SELECT * FROM groups WHERE id=?").get(String(id || ""));
const groupRows = () => db.prepare("SELECT * FROM groups ORDER BY ord, created_at").all();
const itemRows = (id) =>
  db.prepare("SELECT kind, ref, thread, title FROM group_items WHERE group_id=? ORDER BY ord").all(id);
const cleanName = (name) => String(name ?? "").trim().slice(0, 60);
const cleanColor = (c) => (/^#[0-9a-f]{6}$/i.test(String(c || "")) ? String(c).toLowerCase() : "");
// An artifact's id without a card's version suffix ("R18@v1" → "R18").
const artId = (ref) => String(ref || "").replace(/^(R\d+)@v\d+$/, "$1");

function tx(fn) {
  db.exec("BEGIN");
  try {
    const r = fn();
    db.exec("COMMIT");
    return r;
  } catch (e) {
    db.exec("ROLLBACK");
    throw e;
  }
}

// Renumber the rows `ids` in that order (groups or one group's items).
function renumberGroups(ids) {
  const st = db.prepare("UPDATE groups SET ord=? WHERE id=?");
  ids.forEach((id, k) => st.run(k, id));
}
function renumberItems(gid, items) {
  const st = db.prepare("UPDATE group_items SET ord=? WHERE group_id=? AND kind=? AND ref=?");
  items.forEach((it, k) => st.run(k, gid, it.kind, it.ref));
}

function insertGroup(name, color) {
  const n = db.prepare("SELECT COUNT(*) AS n FROM groups").get().n;
  const id = newId();
  db.prepare("INSERT INTO groups(id,name,color,ord,collapsed,created_at) VALUES(?,?,?,?,0,?)")
    .run(id, name, cleanColor(color) || GROUP_COLORS[n % GROUP_COLORS.length], n, Date.now());
  return id;
}

// Adds (or, already there, moves) one member; index = its new position.
function putItem(gid, kind, ref, thread, index, title) {
  const cur = itemRows(gid);
  const items = cur.filter((it) => !(it.kind === kind && it.ref === ref));
  if (items.length === cur.length)
    db.prepare("INSERT INTO group_items(group_id,kind,ref,thread,ord,title) VALUES(?,?,?,?,?,?)")
      .run(gid, kind, ref, thread || null, items.length, title || null);
  else if (thread) db.prepare("UPDATE group_items SET thread=? WHERE group_id=? AND kind=? AND ref=?").run(thread, gid, kind, ref);
  if (title && items.length !== cur.length)
    db.prepare("UPDATE group_items SET title=? WHERE group_id=? AND kind=? AND ref=?").run(title, gid, kind, ref);
  const at = Number.isInteger(index) ? Math.max(0, Math.min(index, items.length)) : items.length;
  items.splice(at, 0, { kind, ref });
  renumberItems(gid, items);
}

/** First boot with groups: every bot section becomes a group of the same
 *  name holding its bots in catalog order, and the sidebar's pinned Mini
 *  Apps become a "Mini Apps" group. Runs once (meta groups_migrated);
 *  reads bot.section and /state/rox-pinned-apps, never changes them. */
export function migrateGroups() {
  if (metaGet("groups_migrated")) return false;
  tx(() => {
    const names = [...(store.sections || [])];
    for (const b of store.bots) if (b.section && !names.includes(b.section)) names.push(b.section);
    for (const name of names) {
      const bots = store.bots.filter((b) => b.section === name);
      if (!bots.length || !cleanName(name)) continue;
      const gid = insertGroup(cleanName(name));
      for (const b of bots) putItem(gid, "bot", b.id, null);
    }
    let pinned = [];
    try {
      const row = db.prepare("SELECT data FROM kv WHERE name='rox-pinned-apps'").get();
      pinned = JSON.parse(row?.data || "{}")?.apps || [];
    } catch {}
    pinned = pinned.filter((p) => p?.app);
    if (pinned.length) {
      const gid = insertGroup("Mini Apps");
      for (const p of pinned) putItem(gid, "artifact", artId(p.app), p.thread || null);
    }
    metaSet("groups_migrated", String(Date.now()));
  });
  return true;
}

/** GET /groups — every group in order, its members resolved: a bot's and
 *  a chat's title (Rox draws them from its own lists), an artifact's
 *  Library record. A member whose bot or chat is gone is dropped for
 *  good; an artifact that can't be found is skipped (and kept). */
export async function listGroups(origin = originOf()) {
  const groups = groupRows();
  const all = groups.map((g) => itemRows(g.id));
  const arts = await artifactRecords(
    all.flat().filter((it) => it.kind === "artifact").map((it) => ({ id: it.ref, thread: it.thread })),
    origin,
  );
  let a = 0;
  const gone = [];
  const out = groups.map((g, k) => ({
    id: g.id,
    name: g.name,
    color: g.color,
    order: k,
    collapsed: !!g.collapsed,
    items: all[k].flatMap((it) => {
      if (it.kind === "bot") {
        const b = store.bots.find((x) => x.id === it.ref);
        if (!b) return gone.push([g.id, it]), [];
        return [{ kind: "bot", ref: it.ref, title: b.name, hidden: !!b.hidden }];
      }
      if (it.kind === "app") return [{ kind: "app", ref: it.ref, title: it.title || it.ref.replace(/^mcp:|\|.*$/g, "") }];
      if (it.kind === "chat") {
        const t = store.list.find((x) => x.id === it.ref);
        if (!t) return gone.push([g.id, it]), [];
        return [{ kind: "chat", ref: it.ref, title: t.title, hidden: !!(t.hidden || t.archived) }];
      }
      const rec = arts[a++];
      if (!rec) return [];
      return [{ kind: "artifact", ref: it.ref, thread: it.thread || rec.origin.thread, title: rec.title, artifact: rec }];
    }),
  }));
  if (gone.length)
    tx(() => {
      for (const [gid, it] of gone)
        db.prepare("DELETE FROM group_items WHERE group_id=? AND kind=? AND ref=?").run(gid, it.kind, it.ref);
    });
  return { groups: out, colors: GROUP_COLORS };
}

/** POST /group/new {name, color?, item?} — item ({kind, ref, thread?})
 *  goes in as its first member (Add to Group › New Group…). */
export function newGroup({ name, color, item } = {}) {
  name = cleanName(name);
  if (!name) throw new Error("a group needs a name");
  const it = item ? checkItem(item) : null;
  return tx(() => {
    const id = insertGroup(name, color);
    if (it) putItem(id, it.kind, it.ref, it.thread, undefined, it.title);
    return id;
  });
}

/** POST /group/update {id, name?, color?, collapsed?, order?} — order is
 *  the group's new position among the groups. */
export function updateGroup({ id, name, color, collapsed, order } = {}) {
  const g = groupRow(id);
  if (!g) throw new Error(`unknown group ${id}`);
  tx(() => {
    if (name !== undefined) {
      const n = cleanName(name);
      if (!n) throw new Error("a group needs a name");
      db.prepare("UPDATE groups SET name=? WHERE id=?").run(n, g.id);
    }
    if (color !== undefined) {
      const c = cleanColor(color);
      if (!c) throw new Error(`bad color ${color}`);
      db.prepare("UPDATE groups SET color=? WHERE id=?").run(c, g.id);
    }
    if (collapsed !== undefined) db.prepare("UPDATE groups SET collapsed=? WHERE id=?").run(collapsed ? 1 : 0, g.id);
    if (order !== undefined) {
      const ids = groupRows().map((r) => r.id).filter((x) => x !== g.id);
      ids.splice(Math.max(0, Math.min(Number(order) || 0, ids.length)), 0, g.id);
      renumberGroups(ids);
    }
  });
}

/** POST /group/delete {id} — the group goes; its bots, chats and
 *  artifacts stay where they are (and in any other group). */
export function deleteGroup({ id } = {}) {
  const g = groupRow(id);
  if (!g) throw new Error(`unknown group ${id}`);
  tx(() => {
    db.prepare("DELETE FROM group_items WHERE group_id=?").run(g.id);
    db.prepare("DELETE FROM groups WHERE id=?").run(g.id);
    renumberGroups(groupRows().map((r) => r.id));
  });
}

/** POST /group/item {id, kind, ref, thread?, op: "add"|"remove", index?}
 *  — add puts the item at index (the end without one); adding one that's
 *  already there moves it (reordering). Other groups are never touched. */
export function groupItem({ id, kind, ref, thread, title, op, index } = {}) {
  const g = groupRow(id);
  if (!g) throw new Error(`unknown group ${id}`);
  if (op === "remove") {
    if (!KINDS.has(kind)) throw new Error(`bad kind ${kind}`);
    ref = kind === "artifact" ? artId(ref) : String(ref || "");
    tx(() => {
      db.prepare("DELETE FROM group_items WHERE group_id=? AND kind=? AND ref=?").run(g.id, kind, ref);
      renumberItems(g.id, itemRows(g.id));
    });
    return;
  }
  if (op !== "add") throw new Error(`unknown op ${op}`);
  const it = checkItem({ kind, ref, thread, title });
  tx(() => putItem(g.id, it.kind, it.ref, it.thread, index == null ? undefined : Number(index), it.title));
}

// A member to add, checked: its bot or chat must exist; an artifact's
// origin chat is dropped when it doesn't.
function checkItem({ kind, ref, thread, title }) {
  if (!KINDS.has(kind)) throw new Error(`bad kind ${kind}`);
  if (kind === "app") {
    ref = String(ref || "");
    if (!/^mcp:[a-z0-9-]+\/[A-Za-z0-9._-]+(\|\{.*\})?$/s.test(ref)) throw new Error(`bad app ${ref}`);
    return { kind, ref, thread: null, title: String(title || "").trim().slice(0, 80) };
  }
  ref = kind === "artifact" ? artId(ref) : String(ref || "");
  if (kind === "bot" && !store.bots.some((b) => b.id === ref)) throw new Error(`unknown bot ${ref}`);
  if (kind === "chat" && !store.list.some((t) => t.id === ref)) throw new Error(`unknown chat ${ref}`);
  if (kind === "artifact" && !/^R\d+$/.test(ref) && !/^(file|gdrive):./.test(ref)) throw new Error(`bad artifact id ${ref}`);
  thread = kind === "artifact" && store.list.some((t) => t.id === thread) ? String(thread) : null;
  return { kind, ref, thread };
}
