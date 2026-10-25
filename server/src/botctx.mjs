// botctx.mjs — a bot's own context: description + instructions, memories
// and integrations, all filled on demand. The bot rows live in store.bots
// (persisted by state.mjs); this module holds the operations on them, the
// system-prompt section and the agent tools that act on the thread's bot.
import { entryOf, iconFile } from "./plugins.mjs";
import { db, persist, store, uuidv7 } from "./state.mjs";

export const botOfThread = (thread) =>
  thread?.id ? store.bots.find((b) => b.threadId === thread.id) : undefined;

const clip = (s, n) => String(s ?? "").trim().slice(0, n);

// Profile fields the bot (bot_profile_update) or the UI (/bot/update) set.
export function setBotProfile(bot, { label, description, instructions } = {}) {
  if (label !== undefined) bot.label = clip(label, 40);
  if (description !== undefined) bot.description = clip(description, 600);
  if (instructions !== undefined) bot.instructions = clip(instructions, 6000);
  if (description !== undefined || instructions !== undefined) touch(bot);
}

// Bot Details' "Last updated": its profile, memories or integrations changed.
export const touch = (bot) => {
  bot.updated_at = Date.now();
};

// ————— memories: [{ id, text, at }] —————
export function memoryAdd(bot, text) {
  const t = clip(text, 1000);
  if (!t) throw new Error("memory needs text");
  bot.memories ??= [];
  const dup = bot.memories.find((m) => m.text.toLowerCase() === t.toLowerCase());
  if (dup) return dup;
  const m = { id: uuidv7(), text: t, at: Date.now() };
  bot.memories.push(m);
  touch(bot);
  if (bot.memories.length > 200) bot.memories.splice(0, bot.memories.length - 200);
  persist();
  return m;
}
/** A memory's new text (Bot Details' edit). */
export function memoryUpdate(bot, memoryId, text) {
  const t = clip(text, 1000);
  if (!t) throw new Error("memory needs text");
  const m = (bot.memories ??= []).find((x) => x.id === memoryId);
  if (!m) throw new Error(`bot has no memory ${memoryId}`);
  m.text = t;
  m.at = Date.now();
  touch(bot);
  persist();
  return m;
}
export function memoryRemove(bot, memoryId) {
  const before = (bot.memories ??= []).length;
  bot.memories = bot.memories.filter((m) => m.id !== memoryId);
  if (bot.memories.length === before) throw new Error(`no memory ${memoryId}`);
  touch(bot);
  persist();
}

// ————— integrations: stored [{ id, kind, at, via }] —————
// kind: "mcp" (mcp_servers id), "plugin" (installed / catalog plugin name),
// "skill" (private skills id). Resolve an id the UI or bot names: an
// explicit "kind:id" prefix or {id, kind} wins, else mcp → plugin → skill.
const KINDS = ["mcp", "plugin", "skill"];
function lookup(kind, id) {
  if (kind === "mcp") {
    const r = db.prepare("SELECT id,name FROM mcp_servers WHERE id=? OR name=?").get(id, id);
    const cat = r && entryOf(r.name || r.id);
    return r && { id: r.id, name: r.name || r.id, kind, description: cat?.description || "", icon: cat && iconFile(cat.key) ? `/plugins/icon/${encodeURIComponent(cat.key)}` : "" };
  }
  if (kind === "plugin") {
    const inst = db.prepare("SELECT name FROM installed_plugins WHERE name=?").get(id);
    const cat = entryOf(id);
    return (inst || cat) && { id, name: cat?.displayName || cat?.name || id, kind, description: cat?.description || "", icon: iconFile(id) ? `/plugins/icon/${encodeURIComponent(id)}` : "" };
  }
  if (kind === "skill") {
    const r = db.prepare("SELECT id,name,description FROM skills WHERE id=? OR name=?").get(id, id);
    return r && { id: r.id, name: r.name || r.id, kind, description: r.description || "", icon: "" };
  }
  return null;
}
export function resolveIntegration(spec) {
  let kind, id;
  if (spec && typeof spec === "object") ({ kind, id } = spec);
  else {
    const m = String(spec || "").match(/^(mcp|plugin|skill):(.+)$/);
    [kind, id] = m ? [m[1], m[2]] : [undefined, String(spec || "")];
  }
  id = String(id || "").trim();
  if (!id) throw new Error("integration needs an id");
  if (kind && !KINDS.includes(kind)) throw new Error(`unknown integration kind ${kind}`);
  for (const k of kind ? [kind] : KINDS) {
    const hit = lookup(k, id);
    if (hit) return hit;
  }
  throw new Error(`no integration matches ${kind ? `${kind}:` : ""}${id}`);
}

// The installed plugin that registered an mcp_servers row, if any.
function pluginOfServer(serverId) {
  for (const p of db.prepare("SELECT name, meta FROM installed_plugins").all()) {
    try {
      if ((JSON.parse(p.meta || "{}").mcp || []).includes(serverId)) return p.name;
    } catch {}
  }
  return null;
}

// The resolved list GET /threads and the tools return: names come live from
// the mcp_servers rows / plugin catalog / skills; a removed one keeps its id.
// A server a listed plugin owns is that plugin — one row per app.
export const integrationsOf = (bot) => {
  const xs = bot.integrations || [];
  const plugins = new Set(xs.filter((x) => x.kind === "plugin").map((x) => x.id));
  return xs
    .filter((x) => !(x.kind === "mcp" && plugins.has(pluginOfServer(x.id))))
    .map((x) => {
      const hit = lookup(x.kind, x.id);
      return hit ? { id: hit.id, name: hit.name, kind: x.kind, description: hit.description, icon: hit.icon } : { id: x.id, name: x.id, kind: x.kind, missing: true };
    });
};

export function integrationAdd(bot, spec, via = "user") {
  const hit = resolveIntegration(spec);
  bot.integrations ??= [];
  if (!bot.integrations.some((x) => x.kind === hit.kind && x.id === hit.id)) {
    bot.integrations.push({ id: hit.id, kind: hit.kind, at: Date.now(), via });
    touch(bot);
    persist();
  }
  return hit;
}
export function integrationRemove(bot, spec) {
  let kind, id;
  if (spec && typeof spec === "object") ({ kind, id } = spec);
  else {
    const m = String(spec || "").match(/^(mcp|plugin|skill):(.+)$/);
    [kind, id] = m ? [m[1], m[2]] : [undefined, String(spec || "")];
  }
  const before = (bot.integrations ??= []).length;
  // Removing a plugin removes the servers it owns, which it stood in for.
  bot.integrations = bot.integrations.filter(
    (x) => !(x.id === id && (!kind || x.kind === kind)) && !(kind === "plugin" && x.kind === "mcp" && pluginOfServer(x.id) === id),
  );
  if (bot.integrations.length === before) throw new Error(`bot has no integration ${spec?.id ?? spec}`);
  touch(bot);
  persist();
}

// A tool call from the thread's bot: an mcp_<server>_<tool> call records the
// plugin that registered the server, else the server itself.
export function recordToolUse(thread, serverId) {
  const bot = botOfThread(thread);
  if (!bot) return;
  bot.integrations ??= [];
  let changed = false;
  const add = (kind, id) => {
    if (bot.integrations.some((x) => x.kind === kind && x.id === id)) return;
    bot.integrations.push({ id, kind, at: Date.now(), via: "use" });
    changed = true;
  };
  const plugin = pluginOfServer(serverId);
  if (plugin) add("plugin", plugin);
  else add("mcp", serverId);
  if (changed) persist();
}

// ————— system prompt section (only for a bot's thread) —————
export function botSection(thread) {
  const bot = botOfThread(thread);
  if (!bot) return "";
  const who = `You are ${bot.name}${bot.label ? `, ${bot.label}` : ""}.${bot.description ? ` ${bot.description}` : ""}`;
  const parts = [`\n\n## Who you are\n${who}`];
  if (bot.instructions) parts.push(`Instructions:\n${bot.instructions}`);
  if (bot.memories?.length)
    parts.push(`Memories (things the user asked you to keep):\n${bot.memories.map((m) => `- ${m.text}`).join("\n")}`);
  parts.push(
    "Your profile and memories are yours to maintain: memory_add / memory_list / memory_remove, bot_profile_update, integrations_list.",
  );
  return parts.join("\n\n");
}

// ————— agent tools (offered only when the thread has a bot) —————
export const BOT_TOOLS = [
  {
    name: "bot_profile_update",
    description:
      "Change your own profile. Use when the user asks you to change how you behave, your persona, tone or rules (instructions), what you are (description), or your tagline (label). Pass only the fields that change; instructions replaces the whole text, so include what should stay.",
    input_schema: {
      type: "object",
      properties: {
        label: { type: "string", description: "short tagline, ≤40 chars" },
        description: { type: "string", description: "one or two sentences: who you are" },
        instructions: { type: "string", description: "the full persona/behavior instructions" },
      },
    },
  },
  {
    name: "memory_add",
    description:
      "Save a durable fact. Use when the user says remember…, keep in mind…, from now on…, or shares a lasting preference or fact about themselves. One fact per call, written as a short standalone sentence.",
    input_schema: {
      type: "object",
      properties: { text: { type: "string" } },
      required: ["text"],
    },
  },
  {
    name: "memory_list",
    description: "List your saved memories with their ids. Use before removing one, or when asked what you remember.",
    input_schema: { type: "object", properties: {} },
  },
  {
    name: "memory_remove",
    description: "Delete a saved memory by id. Use when the user says forget…, or a memory is wrong or outdated (memory_list shows ids).",
    input_schema: {
      type: "object",
      properties: { id: { type: "string" } },
      required: ["id"],
    },
  },
  {
    name: "integrations_list",
    description: "List the integrations (connected MCP servers, plugins, skills) recorded as yours. Use when asked what you are connected to.",
    input_schema: { type: "object", properties: {} },
  },
];
export const isBotTool = (name) => BOT_TOOLS.some((t) => t.name === name);

export function toolBot(thread, name, input = {}) {
  const bot = botOfThread(thread);
  if (!bot) return { content: `${name} only works in a bot's thread`, is_error: true };
  if (name === "bot_profile_update") {
    const patch = {};
    for (const k of ["label", "description", "instructions"]) if (input[k] !== undefined) patch[k] = input[k];
    if (!Object.keys(patch).length) return { content: "nothing to update — pass label, description or instructions", is_error: true };
    setBotProfile(bot, patch);
    persist();
    return { content: `updated ${Object.keys(patch).join(", ")} — applies from your next step` };
  }
  if (name === "memory_add") {
    const m = memoryAdd(bot, input.text);
    return { content: `remembered (${m.id})` };
  }
  if (name === "memory_list") {
    const ms = bot.memories || [];
    return { content: ms.length ? ms.map((m) => `${m.id} — ${m.text}`).join("\n") : "no memories yet" };
  }
  if (name === "memory_remove") {
    memoryRemove(bot, String(input.id || ""));
    return { content: `forgot ${input.id}` };
  }
  if (name === "integrations_list") {
    const xs = integrationsOf(bot);
    return {
      content: xs.length
        ? xs.map((x) => `${x.kind}:${x.id} — ${x.name}${x.missing ? " (removed)" : ""}`).join("\n")
        : "no integrations recorded yet (the global catalog is still available)",
    };
  }
  return { content: `unknown bot tool ${name}`, is_error: true };
}
