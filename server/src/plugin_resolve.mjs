// plugin_resolve.mjs — locate a Grok Bot catalog entry's plugin inside its
// repository and read what it ships (MCP servers, skills).
//
// Works over an abstract repo view — `files` (every path at HEAD) and
// `read(path) → string|null` — so the catalog sync tool (blobless clones in
// a scratch dir) and the server's installer (blobless clones under
// PLUGINS_DIR/.repos) share one resolver.
import { posix } from "node:path";

const { dirname, join, normalize } = posix;

// Manifest dirs plugin repos use: .cursor-plugin, .claude-plugin,
// .codex-plugin, .grok-plugin, .plugin, … — plus a bare plugin.json.
const MANIFEST_RE = /(^|\/)(?:\.[a-z0-9]+-plugin\/|\.plugin\/)?plugin\.json$/;
const MARKET_RE = /(^|\/)(?:\.[a-z0-9]+-plugin|\.agents\/plugins|\.github\/plugin|\.plugin)\/marketplace\.json$/;
// Preference when one plugin dir carries several manifests.
const MANIFEST_RANK = [".cursor-plugin/", ".claude-plugin/", ".grok-plugin/", ".codex-plugin/", ".plugin/"];
const rankOf = (f) => {
  const i = MANIFEST_RANK.findIndex((m) => f.includes(m));
  return i < 0 ? MANIFEST_RANK.length : i;
};
const skip = (f) => /(^|\/)(node_modules|test|tests|fixtures|examples?)\//.test(f);
const clean = (p) => {
  const n = normalize(String(p || "").replace(/^\.\//, "")).replace(/^\.$/, "").replace(/\/+$/, "");
  return n.startsWith("..") ? null : n;
};
const pluginDirOf = (manifest) => {
  const d = dirname(manifest);
  return /(^|\/)\.(?:[a-z0-9]+-)?plugin$/.test(d) ? clean(dirname(d)) ?? "" : d === "." ? "" : d;
};
const parse = (s) => { try { return JSON.parse(s); } catch { return null; } };

// The plugin path a skill's catalog sourceUrl points into
// (…/blob/<sha>/<dir>/skills/<skill>/SKILL.md → <dir>).
export function sourceUrlInfo(url) {
  const m = String(url || "").match(/^https:\/\/github\.com\/([^/]+\/[^/]+)\/blob\/([^/]+)\/(.+)$/);
  if (!m) return null;
  const file = m[3];
  const dir = file.match(/^(?:(.*)\/)?skills\/[^/]+\/SKILL\.md$/i);
  return { repo: `https://github.com/${m[1]}`, sha: m[2], file, pluginDir: dir ? dir[1] || "" : null };
}

export function repoOf(entry) {
  if (entry.repositoryUrl) return entry.repositoryUrl.replace(/\/+$/, "").replace(/\.git$/, "");
  for (const s of entry.skills || []) {
    const i = sourceUrlInfo(s.sourceUrl);
    if (i) return i.repo;
  }
  return null;
}

// MCP servers a plugin dir declares: plugin.json `mcpServers` (inline
// object, or a path / list of paths to JSON files), else mcp.json /
// .mcp.json beside it.
async function mcpOf(dir, manifest, read, inline) {
  const servers = {};
  const looksLike = (v) => v && typeof v === "object" && (v.url || v.command);
  const take = (j) => {
    let s = j && (j.mcpServers || (j.mcp && typeof j.mcp === "object" ? j.mcp : null));
    // Some plugins drop the mcpServers wrapper: {"notion": {"url": …}}.
    if (!s && j && typeof j === "object" && Object.values(j).some(looksLike)) s = j;
    if (s && typeof s === "object" && !Array.isArray(s))
      for (const [k, v] of Object.entries(s)) if (v && typeof v === "object" && (v.url || v.command)) servers[k] = v;
  };
  if (inline) take({ mcpServers: inline });
  const decl = manifest?.mcpServers;
  if (decl && typeof decl === "object" && !Array.isArray(decl)) take(manifest);
  else
    for (const p of [decl].flat().filter((x) => typeof x === "string")) {
      const f = clean(join(dir, p));
      if (f != null) take(parse(await read(f)));
    }
  if (!Object.keys(servers).length)
    for (const f of ["mcp.json", ".mcp.json"]) {
      take(parse(await read(dir ? `${dir}/${f}` : f)));
      if (Object.keys(servers).length) break;
    }
  return servers;
}

// Every SKILL.md under the plugin's declared skill dirs (plugin.json
// `skills`, default skills/).
function skillsOf(dir, manifest, files) {
  const roots = [manifest?.skills].flat().filter((x) => typeof x === "string").map((p) => clean(join(dir, p)));
  if (!roots.length) roots.push(dir ? `${dir}/skills` : "skills");
  const out = [];
  for (const r of roots.filter((x) => x != null)) {
    const pre = r ? `${r}/` : "";
    for (const f of files) {
      if (!f.startsWith(pre) || !/(^|\/)SKILL\.md$/i.test(f)) continue;
      const rest = f.slice(pre.length).split("/");
      if (rest.length === 2) out.push({ name: rest[0], file: f });
      else if (rest.length === 1 && r) out.push({ name: r.split("/").pop(), file: f });
    }
  }
  return out;
}

// → { path, via, manifestFile, mcpServers, skills:[{name,file}], missing:[connector names not declared] }
//   or { error } when nothing in the repo matches.
export async function resolveEntry(entry, { files, read }) {
  const names = [entry.pluginName, entry.name].filter(Boolean).map((s) => s.toLowerCase());
  const manifests = files.filter((f) => MANIFEST_RE.test(f) && !skip(f));
  const manifestIn = (dir) =>
    manifests.filter((m) => pluginDirOf(m) === dir).sort((a, b) => rankOf(a) - rankOf(b))[0] || null;
  let path = null, via = null, inline = null;

  // 1. Marketplace entry named like the plugin (cursor/plugins and every
  //    multi-plugin repo list {name, source}).
  for (const mf of files.filter((f) => MARKET_RE.test(f) && !skip(f)).sort((a, b) => rankOf(a) - rankOf(b))) {
    const m = parse(await read(mf));
    const root = clean(m?.metadata?.pluginRoot || "") || "";
    for (const n of names) {
      const hit = (m?.plugins || []).find((p) => String(p?.name || "").toLowerCase() === n);
      if (!hit) continue;
      const src = typeof hit.source === "string" ? hit.source : hit.source?.path ?? (hit.source?.source === "url" || hit.source?.source === "github" ? null : "");
      if (src == null) continue; // points at another repo
      const p = clean(join(root, src));
      if (p == null || (p && !files.some((f) => f.startsWith(p + "/")))) continue;
      path = p; via = "marketplace"; inline = hit.mcpServers && typeof hit.mcpServers === "object" ? hit.mcpServers : null;
      break;
    }
    if (path != null) break;
  }
  // 2. A manifest whose name matches.
  if (path == null) {
    for (const n of names) {
      for (const mf of [...manifests].sort((a, b) => rankOf(a) - rankOf(b))) {
        const m = parse(await read(mf));
        if (String(m?.name || "").toLowerCase() === n) { path = pluginDirOf(mf); via = "manifest"; break; }
      }
      if (path != null) break;
    }
  }
  // 3. The directory the catalog's skill sourceUrls point into.
  if (path == null) {
    const dirs = new Set((entry.skills || []).map((s) => sourceUrlInfo(s.sourceUrl)?.pluginDir).filter((d) => d != null));
    if (dirs.size === 1) { path = [...dirs][0]; via = "skill-source"; }
  }
  // 4. A root plugin (manifest, mcp config or skills at the top level).
  if (path == null && (manifestIn("") || files.some((f) => f === "mcp.json" || f === ".mcp.json" || /^skills\/[^/]+\/SKILL\.md$/i.test(f)))) {
    path = ""; via = "root";
  }
  if (path == null) return { error: "no plugin manifest in the repository matches this entry" };

  const manifestFile = manifestIn(path);
  const manifest = manifestFile ? parse(await read(manifestFile)) : null;
  const mcpServers = await mcpOf(path, manifest, read, inline);
  let skills = skillsOf(path, manifest, files);
  // Catalog skills that live outside the resolved dir (shared skill trees).
  const have = new Set(skills.flatMap((s) => [s.name, s.file]));
  for (const s of entry.skills || []) {
    const i = sourceUrlInfo(s.sourceUrl);
    if (i && !have.has(s.name) && !have.has(i.file) && files.includes(i.file)) {
      skills.push({ name: i.file.split("/").slice(-2, -1)[0], file: i.file });
      have.add(i.file);
    }
  }
  // Catalog connector names are slugs of the declared keys
  // ("Mintlify Search" → mintlify-search); re-key to the catalog's names.
  const slug = (n) => String(n).toLowerCase().replace(/[^a-z0-9]+/g, "");
  const wanted = (entry.connectors || []).map((c) => c.name);
  for (const n of wanted) {
    if (mcpServers[n]) continue;
    const k = Object.keys(mcpServers).find((k) => !wanted.includes(k) && slug(k) === slug(n));
    if (k) { mcpServers[n] = mcpServers[k]; delete mcpServers[k]; }
  }
  // One declared server, one catalog connector, different names.
  const extra = Object.keys(mcpServers).filter((k) => !wanted.includes(k));
  const lacking = wanted.filter((n) => !mcpServers[n]);
  if (extra.length === 1 && lacking.length === 1 && wanted.length === 1) {
    mcpServers[lacking[0]] = mcpServers[extra[0]];
    delete mcpServers[extra[0]];
  }
  const missing = (entry.connectors || []).map((c) => c.name).filter((n) => !mcpServers[n]);
  return { path, via, manifestFile, mcpServers, skills, missing };
}

// Cursor routes its Google Drive/Docs/Sheets/Slides plugins through its
// own REST proxy; Google serves the same MCP servers directly
// (<product>mcp.googleapis.com), so those go straight to Google. Other
// connectors hosted behind Cursor's or Grok's account gateways can't be
// reached without those accounts.
const GOOGLE_PROXY = /^https:\/\/api\.cursor\.com\/rest-mcp\/google-(drive|sheets|docs|slides)\/mcp\/?$/;
const HOSTED = [
  [/^https:\/\/api\.cursor\.com\//, "Hosted by Cursor — it signs in through a Cursor account, which Grok Shell doesn't have"],
  [/^https:\/\/connectors-gateway\.grok\.com\//, "Hosted by Grok's connector gateway — it needs a Grok account session"],
];
export function adjustServers(servers) {
  const out = {};
  for (const [n, cfg] of Object.entries(servers)) {
    let c = cfg;
    const g = typeof c?.url === "string" && c.url.match(GOOGLE_PROXY);
    if (g) c = { ...c, url: `https://${g[1]}mcp.googleapis.com/mcp/v1` };
    const hosted = typeof c?.url === "string" && HOSTED.find(([re]) => re.test(c.url));
    if (hosted) c = { ...c, unavailable: hosted[1] };
    out[n] = c;
  }
  return out;
}
