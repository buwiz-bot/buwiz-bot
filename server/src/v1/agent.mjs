// v1/agent.mjs — what v1 plugins put in front of the model (spec 9.1, 9.6,
// 10). Tools keep their own names (`<short>_<tool>`), descriptions get a
// `[<short>] ` prefix and a 1024-character cap, and only enabled, running,
// approved plugins contribute anything: tools, server instructions and
// scope hints. Disabled tools and reserved `rox.*` names never appear.
// Plugin text is capped at a word boundary with a visible marker and has
// heading markers removed, so it cannot pose as the host's own sections;
// the host writes the labelled headers itself. A tool result's `_meta`
// never reaches the model; a `rox/open` in it becomes a card on the job
// that Rox renders with an Open (or Show) button, and nothing opens by
// itself (9.7).
import { V1, isV1 } from "../grammar.mjs";
import { isObject } from "./jcs.mjs";
import { isApproved } from "./trust.mjs";
import { checkOpen } from "./sessions.mjs";

export const TRUNCATED = "…[truncated]";

// names the agent already has: a v1 tool may not shadow them
const BUILTIN = /^(mcp|office|skill|routine|app|bot)_/;

/** Plugin text for the model: heading markers removed, at most `cap`
 *  characters, cut at a word boundary with the marker (10.2). */
export function approvedText(text, cap) {
  const clean = String(text ?? "").replace(/^[ \t]*#{1,6}[ \t]+/gm, "").trim();
  const cs = [...clean];

  if (cs.length <= cap) return clean;
  const room = cs.slice(0, cap - [...TRUNCATED].length).join("");
  const cut = room.search(/\s\S*$/);

  return `${(cut > 0 ? room.slice(0, cut) : room).trimEnd()}${TRUNCATED}`;
}

// enabled, running and on the approved list as installed (4.1)
function contributing(plugins) {
  const trust = plugins.trust();

  return plugins.rows().filter((row) => row.enabled && plugins.runtime(row.id).state === "running" && isApproved(trust, row.id, row.publisher_key_id));
}

// A routine's turn (5.9) sees its own plugin's tools and only the `reads`
// tools it names; `surface` is {plugin, reads} then, null otherwise.
const inSurface = (surface, plugin, name) => !surface || plugin === surface.plugin || (surface.reads || []).some((x) => x.plugin === plugin && x.tools.includes(name));

/** The model's v1 tools: [{name, description, input_schema, plugin}];
 *  `taken` holds names the agent already has, `surface` narrows them to a
 *  routine's turn. */
export function v1Tools(plugins, taken = new Set(), surface = null) {
  const out = [];

  for (const row of contributing(plugins)) {
    const off = plugins.disabledOf(row);

    for (const t of plugins.runtime(row.id).tools) {
      if (isV1("reservedTool", t.name) || off.has(t.name) || taken.has(t.name) || BUILTIN.test(t.name) || !t.name.startsWith(`${row.short}_`)) continue;

      if (!inSurface(surface, row.id, t.name)) continue;
      out.push({ name: t.name, description: approvedText(`[${row.short}] ${t.description || t.name}`, 1024), input_schema: t.inputSchema || { type: "object", properties: {} }, plugin: row.id });
    }
  }

  return out;
}

/** The plugin id serving v1 tool `name` on the model's surface, or null. */
export const v1ToolPlugin = (plugins, name, surface = null) => v1Tools(plugins, new Set(), surface).find((t) => t.name === name)?.plugin || null;

const fold = (r) =>
  (r.content || []).map((c) => (c.type === "text" ? c.text : c.type === "image" ? `[image: ${c.mimeType || "image/png"}]` : JSON.stringify(c))).join("\n") || "(empty)";

// 9.6: a card for the job, never an open; a bad card is dropped and logged
function noteCard(job, plugins, row, open) {
  let card;

  try {
    card = checkOpen(plugins.manifestOf(row), open);
  } catch (e) {
    console.error(`  v1 ${row.id}: rox/open card dropped: ${String(e?.message || e).slice(0, 200)}`);

    return;
  }

  const query = card.ticket ? `?ticket=${card.ticket}` : "";
  const link = `rox://app/${card.plugin}/${card.app}${query}`;

  job.v1Cards ||= [];
  job.v1Cards.push(card);
  job.linkTurn ||= { links: [] };
  job.linkTurn.links.push({
    provider: "rox-app", id: link, name: card.title, link, action: card.placement === "inline" ? "show" : "open", kind: "app", file_type: "Mini app", card,
  });
}

/** A model tool call to a v1 plugin (origin agent, 8.4): {content, is_error}. */
export async function v1AgentCall(plugins, job, name, input) {
  const plugin = v1ToolPlugin(plugins, name, job?.v1Surface);
  const row = plugin && plugins.rowOf(plugin);

  if (!row) return { content: `${name} is not available: its plugin is off, failed, or the tool is switched off`, is_error: true };
  let r;

  try {
    r = await plugins.callTool(plugin, name, isObject(input) ? input : {}, { meta: { "rox/caller": { origin: "agent", plugin, ts: Date.now() } } });
  } catch (e) {
    return { content: `${name}: ${e.code || "error"}: ${String(e.message || e).slice(0, 300)}`, is_error: true };
  }

  const open = r._meta?.["rox/open"];

  if (open && !r.isError && job) noteCard(job, plugins, row, open);

  return { content: fold(r), is_error: !!r.isError };
}

/** The system-prompt text v1 plugins add for a user message: approved
 *  server instructions, and the scopes whose tokens are in the message. */
export function v1PromptSection(plugins, prompt = "") {
  const parts = [];
  const rows = contributing(plugins);

  for (const row of rows) {
    const instr = plugins.runtime(row.id).instructions;

    if (instr) parts.push(`## Instructions from ${plugins.manifestOf(row).title} (approved)\n${approvedText(instr, 4000)}`);
  }

  const used = new Set();

  for (const m of String(prompt).matchAll(new RegExp(V1.typedToken, "g"))) used.add(`${m[1]}:${m[2]}`);

  for (const key of used) {
    const [short, scope] = key.split(":");
    const row = rows.find((x) => x.short === short);
    const s = row && (plugins.manifestOf(row).scopes || []).find((x) => x.name === scope);

    if (!s) continue;
    const body = [approvedText(s.description, 120), s.hint ? approvedText(s.hint, 1500) : ""].filter(Boolean).join("\n");

    parts.push(`## Scope: ${short}:${scope} (approved plugin ${row.id})\n${body}`);
  }

  return parts.length ? `\n\n${parts.join("\n\n")}` : "";
}

/** A routine's prompt with its label (10.2), or null when the plugin may
 *  not contribute text. */
export function v1RoutinePrompt(plugins, pluginId, routineId) {
  const row = contributing(plugins).find((x) => x.id === pluginId);
  const r = row && (plugins.manifestOf(row).routines || []).find((x) => x.id === routineId);

  return r ? `## Routine: ${approvedText(r.name, 80)} (approved plugin ${row.id})\n${approvedText(r.prompt, 4000)}` : null;
}
