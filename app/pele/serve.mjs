// Rox's Mermaid renderer: Pele (pele.js, MIT, vendored) behind a tiny
// localhost HTTP server, so ```mermaid fences draw as native SVG images
// instead of one WKWebView + mermaid.js per diagram.
//
//   node serve.mjs            → prints `PORT <n>` on its first stdout line
//   POST /render {code, theme: "dark"|"light"}
//        → {uri: "data:image/png;base64,…", width, height} | {error}
//
// Pele themes through CSS variables (`var(--pele-fg,#1f1f1f)`), which a
// native SVG renderer does not evaluate; they are resolved here to Rox's
// palette. nsh's own SVG renderer draws no `<text>` (no font database by
// design), so resvg rasterizes here at 2× with two system fonts loaded
// explicitly (~15 ms, off Rox's threads; scanning every system font took
// ~270 ms a diagram). The process exits when its stdin closes (Rox quit).
import { createServer } from "node:http";
import { createRequire } from "node:module";
import { render } from "./pele.js";

const { Resvg } = createRequire(import.meta.url)("@resvg/resvg-js");
const SCALE = 2;
const FONTS = {
  loadSystemFonts: false,
  fontFiles: ["/System/Library/Fonts/HelveticaNeue.ttc", "/System/Library/Fonts/Menlo.ttc"],
  defaultFontFamily: "Helvetica Neue",
  sansSerifFamily: "Helvetica Neue",
  monospaceFamily: "Menlo",
};

// Rox's agent-bubble surface and inks (theme.rs: AGENT_BUBBLE, TEXT, DIM…).
const PALETTES = {
  dark: {
    bg: "#1f1f1f", fg: "#f5f5f5", muted: "#a8a8a8", line: "#c8c8c8",
    surface: "#2a2a2a", "surface-alt": "#333333", border: "#5a5a5a",
    accent: "#4d9bff", font: "Helvetica Neue, sans-serif",
    "font-mono": "Menlo, monospace",
  },
  light: {
    bg: "#f0f0f0", fg: "#1c1c1c", muted: "#6e6e6e", line: "#3a3a3a",
    surface: "#ffffff", "surface-alt": "#e6e6e6", border: "#a0a0a0",
    accent: "#1884fc", font: "Helvetica Neue, sans-serif",
    "font-mono": "Menlo, monospace",
  },
};

// A fallback may hold one level of parentheses (`rgba(0,0,0,.5)`).
const PELE_VAR = /var\(--pele-([a-z0-9-]+)\s*(?:,\s*((?:[^()]|\([^()]*\))*))?\)/g;
const LOCAL_DECL = /--(_[a-z0-9-]+)\s*:\s*((?:[^;"()]|\([^()]*\))+);?/g;
const LOCAL_VAR = /var\(--(_[a-z0-9-]+)\s*(?:,\s*((?:[^()]|\([^()]*\))*))?\)/g;

function resolve(svg, palette) {
  svg = svg.replace(PELE_VAR, (_m, name, fb) => palette[name] ?? (fb ?? "").trim());
  const local = {};
  for (const [, k, v] of svg.matchAll(LOCAL_DECL)) local[k] = v.trim();
  for (let i = 0; i < 4 && LOCAL_VAR.test(svg); i++) {
    svg = svg.replace(LOCAL_VAR, (m, k, fb) => local[k] ?? (fb ?? "").trim() ?? m);
  }
  return svg.replace(LOCAL_DECL, "");
}

const cache = new Map();

function draw(code, theme) {
  const key = theme + "\0" + code;
  let hit = cache.get(key);
  if (!hit) {
    const out = render(code);
    const svg = resolve(out.svg, PALETTES[theme] ?? PALETTES.dark);
    const png = new Resvg(svg, { fitTo: { mode: "zoom", value: SCALE }, font: FONTS })
      .render()
      .asPng();
    hit = {
      uri: "data:image/png;base64," + Buffer.from(png).toString("base64"),
      width: out.width,
      height: out.height,
    };
    cache.set(key, hit);
  }
  return hit;
}

const server = createServer((req, res) => {
  if (req.method !== "POST" || req.url !== "/render") {
    res.writeHead(404).end();
    return;
  }
  let body = "";
  req.on("data", (c) => (body += c));
  req.on("end", () => {
    let reply;
    try {
      const { code, theme } = JSON.parse(body);
      reply = draw(String(code ?? ""), theme === "light" ? "light" : "dark");
    } catch (e) {
      reply = { error: String(e?.message ?? e) };
    }
    res.writeHead(200, { "content-type": "application/json" }).end(JSON.stringify(reply));
  });
});

server.listen(0, "127.0.0.1", () => {
  process.stdout.write(`PORT ${server.address().port}\n`);
});
process.stdin.on("end", () => process.exit(0));
process.stdin.resume();
