#!/usr/bin/env node
// tools/parity.mjs — do two brokers answer Rox's reads the same? (The Rust
// broker plan, 0.3.) Replays the GET routes Rox calls (from
// route-inventory.mjs) against brokers A and B, each started on a copy of
// the same data (scripts/live-copy), and diffs status and JSON. Fields that
// differ between any two runs (VOLATILE) are left out.
//
//   node server/tools/parity.mjs <url A>[,<data dir A>] <url B>[,<data dir B>]
//
// A data dir supplies that broker's host token for /v1/host/* routes; each
// side's data dir and URL in an answer read as {data} and {broker}.
// Exits non-zero on any difference, listing each with its first JSON path.
import { execFileSync } from "node:child_process";
import { readFileSync, realpathSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

const HERE = dirname(fileURLToPath(import.meta.url));
// streams, calls outside the broker (Reminders, Linear, OAuth), office
// assets and routes that need an argument Rox supplies per call
const SKIP = [/^\/v1\/host\/events$/, /^\/oauth\//, /^\/data\/(reminders|linear)/, /^\/office/, /^\/mermaid/, /^\/module$/, /^\/file\//, /^\/asset\//, /^\/files\//, /^\/app\/icon$/, /^\/artifact\/icon$/, /^\/plugins\/(icon|detail|skill)/, /^\/v1\/(hooks|bridge)/, /^\/v1\/host\/$/];
// Rox's own state keys (app/src: "/state/rox-…")
const STATE = ["rox-drafts", "rox-main-bot", "rox-reactions", "rox-zoom", "rox-pinned-apps"];
const VOLATILE = new Set(["uptime", "pid", "now", "ms", "elapsed", "started_at", "startedAt", "generation", "as_of", "took"]);

const [a, b] = process.argv.slice(2).map((arg) => {
  const [url, data] = String(arg || "").split(",");

  const dirs = data ? [...new Set([data.replace(/\/+$/, ""), realpathSync(data)])] : [];

  return { url: url?.replace(/\/+$/, ""), dirs, token: data ? readFileSync(join(data, "host-token"), "utf8").trim() : "" };
});

if (!a?.url || !b?.url) {
  console.error("usage: parity.mjs <url A>[,<data dir A>] <url B>[,<data dir B>]");
  process.exit(2);
}

const routes = JSON.parse(execFileSync(process.execPath, [join(HERE, "route-inventory.mjs"), "--json"], { encoding: "utf8" }));
const paths = new Set(
  routes
    .filter((r) => r.rox && /^(GET|ANY \(GET)/.test(r.method) && r.match !== "regex" && !SKIP.some((re) => re.test(r.path)))
    .map((r) => r.path.replace(/\?$/, ""))
    .filter((p) => p !== "/state"),
);

for (const k of STATE) paths.add(`/state/${k}`);

async function get(side, path) {
  const res = await fetch(side.url + path, { headers: path.startsWith("/v1/host/") ? { "X-Rox-Host": side.token } : {}, signal: AbortSignal.timeout(20_000) });
  let text = await res.text();

  // where the answer names this broker's own data dir or address
  for (const d of [...side.dirs].sort((x, y) => y.length - x.length)) text = text.split(d).join("{data}");
  text = text.split(side.url).join("{broker}");
  let body;

  try {
    body = JSON.parse(text);
  } catch {
    body = text;
  }

  return { status: res.status, body };
}

const clean = (v) =>
  Array.isArray(v) ? v.map(clean) : v && typeof v === "object" ? Object.fromEntries(Object.entries(v).filter(([k]) => !VOLATILE.has(k)).map(([k, x]) => [k, clean(x)])) : v;

// the first JSON path where x and y differ
function diff(x, y, at = "$") {
  if (typeof x !== typeof y || Array.isArray(x) !== Array.isArray(y)) return at;

  if (x && typeof x === "object") {
    for (const k of new Set([...Object.keys(x), ...Object.keys(y)])) {
      const d = diff(x[k], y[k], Array.isArray(x) ? `${at}[${k}]` : `${at}.${k}`);

      if (d) return d;
    }

    return null;
  }

  return x === y ? null : at;
}

// every thread's turns, as Rox reads a chat that isn't the active one
const threads = (await get(a, "/threads")).body?.list || [];

for (const t of threads) paths.add(`/turns?thread=${t.id}`);
let differ = 0;
const lines = [];

for (const path of [...paths].sort()) {
  const [x, y] = await Promise.all([get(a, path), get(b, path)]);
  const d = x.status !== y.status ? `status ${x.status} vs ${y.status}` : diff(clean(x.body), clean(y.body));

  if (d) {
    differ++;
    lines.push(`DIFF ${path}: ${d}`);
  } else lines.push(`same ${path} (${x.status})`);
}
console.log(lines.join("\n"));
console.log(`\n${paths.size} reads, ${differ} differ`);
process.exit(differ ? 1 : 0);
