#!/bin/sh
# The app-server half of tools/package.sh: node, the bundled server and what
# it reads beside itself, into <Resources dir>/server. Separate so a server
# update (or its rehearsal) can rebuild just this into an existing bundle.
# Usage: tools/package-server.sh <Resources dir>
set -e
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
R="$1"
[ -n "$R" ] || { echo "usage: tools/package-server.sh <Resources dir>" >&2; exit 2; }

mkdir -p "$R/server"
# node runtime — the one on this machine for local builds; CI substitutes
# the official tarball's binary per platform.
# rm first: copying over the node a running server mapped keeps the old
# file's code-signature cache, and macOS kills every later start (SIGKILL)
rm -f "$R/server/node"
cp "$(node -p 'process.execPath')" "$R/server/node"
# Server as a single file: @anthropic-ai/sdk and our own modules inlined;
# node:* builtins stay external. Dynamic ./office and widget imports are
# left as-is and resolve against the shipped dirs below.
# The BetterOffice engines stay external (vendored below): inlined, their
# wasm resolves beside the bundle, where it isn't, and every office_create
# fails with "ooxml-opc wasm is not initialized".
bun build "$ROOT/server/app-server.mjs" --target=node --format=esm \
  --external "@betteroffice/*" --outfile "$R/server/app-server.mjs"
cp -R "$ROOT/widgets" "$R/server/widgets" 2>/dev/null || true
cp -R "$ROOT/server/plugins" "$R/server/plugins" 2>/dev/null || true
# Runtime assets the bundle reads beside itself: TSX templates, the office
# viewer/editor, and mermaid for the diagram web view.
cp -R "$ROOT/server/templates" "$R/server/templates" 2>/dev/null || true
cp -R "$ROOT/server/office" "$R/server/office" 2>/dev/null || true
mkdir -p "$R/node_modules/mermaid/dist"
cp "$ROOT/node_modules/mermaid/dist/mermaid.min.js" "$R/node_modules/mermaid/dist/" 2>/dev/null || true
# The BetterOffice engines are runtime-resolved (import.meta.resolve for the
# caps probe, /office/vendor/ for the viewer's ESM imports, wasm + font
# assets loaded off disk) — they must ship as real packages, not be inlined.
cp -R "$ROOT/node_modules/@betteroffice" "$R/node_modules/@betteroffice" 2>/dev/null || true
# The office MCP child spawns its own node; bundling mcp-main drops its
# @modelcontextprotocol/sdk + zod deps. The engines stay external — they
# resolve against the vendored @betteroffice above. The skia .node asset is
# required relative to the bundle, so it ships beside it.
MCP_OUT="$ROOT/dist/.mcp-$$"
rm -rf "$MCP_OUT"
bun build "$ROOT/server/office/mcp-main.ts" --target=node --format=esm \
  --external "@betteroffice/*" --outdir "$MCP_OUT" && \
  cp "$MCP_OUT"/mcp-main.js "$MCP_OUT"/*.node "$R/server/office/" 2>/dev/null
rm -rf "$MCP_OUT"


# Mini App Protocol v1: the trust store sits beside the bundle (trust.mjs
# reads trust.json next to the module it was bundled into), and each
# first-party v1 plugin ships bundled with its SDK deps, as its own process
# runs it, with its (signed at release) manifest beside it.
cp "$ROOT/server/v1/trust.json" "$R/server/trust.json"
GWS="$R/server/v1-plugins/google-workspace"
mkdir -p "$GWS"
bun build "$ROOT/server/v1/plugins/google-workspace/server.mjs" --target=node --format=esm --outfile "$GWS/server.mjs"
cp "$ROOT/server/v1/plugins/google-workspace/manifest.json" "$GWS/manifest.json"
