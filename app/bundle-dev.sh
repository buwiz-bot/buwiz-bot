#!/usr/bin/env bash
# Creates ~/Applications/Rox.app: a dev bundle whose executable execs this
# checkout's debug build (cargo rebuilds are picked up without re-bundling)
# and which declares the `rox://` URL scheme, then registers it with
# LaunchServices so `open "rox://chat/<thread>?m=<turn>"` reaches Rox.
# Launch Rox through it (`open -a ~/Applications/Rox.app`) so links go to the
# running copy instead of starting a second one; copy a fresh build in first
# (`cp target/debug/rox ~/Applications/Rox.app/Contents/MacOS/rox-bin`).
#
# It also packages the broker Rox starts when none answers (plan 7.2) into
# Contents/Resources/broker/: rox-broker (a release build with
# RUSTC_WRAPPER= unless ROX_BROKER_BIN names one) and the static files it
# serves, `server/` being its --assets and `node_modules/` beside it.
#   ROX_APP=<dir>/Rox.app   bundle elsewhere (a dry run); it is not
#                           registered with LaunchServices
#   ROX_BROKER_BIN=<path>   the rox-broker to package
#   OFFICE_DIST=<dir>       the built office editor bundle (default
#                           server/src/office/editor/dist; built by
#                           `npm run build:office-editor` in server/)
set -euo pipefail

DEFAULT_APP="$HOME/Applications/Rox.app"
APP="${ROX_APP:-$DEFAULT_APP}"
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
BIN=/Volumes/goldcoders/Projects/rox/app/target/debug/rox
LSREGISTER=/System/Library/Frameworks/CoreServices.framework/Frameworks/LaunchServices.framework/Support/lsregister

mkdir -p "$APP/Contents/MacOS"

cat > "$APP/Contents/Info.plist" <<PLIST
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>CFBundleIdentifier</key>
  <string>dev.goldcoders.rox</string>
  <key>CFBundleName</key>
  <string>Rox</string>
  <key>CFBundleDisplayName</key>
  <string>Rox</string>
  <key>CFBundleExecutable</key>
  <string>rox</string>
  <key>CFBundlePackageType</key>
  <string>APPL</string>
  <key>CFBundleVersion</key>
  <string>0.1.0</string>
  <key>CFBundleShortVersionString</key>
  <string>0.1.0</string>
  <key>NSHighResolutionCapable</key>
  <true/>
  <key>CFBundleURLTypes</key>
  <array>
    <dict>
      <key>CFBundleURLName</key>
      <string>dev.goldcoders.rox</string>
      <key>CFBundleURLSchemes</key>
      <array>
        <string>rox</string>
      </array>
    </dict>
  </array>
</dict>
</plist>
PLIST

cat > "$APP/Contents/MacOS/rox" <<SH
#!/bin/sh
# Dev launcher: the checkout's debug build, with local automation on. A
# copy inside the bundle (rox-bin, refreshed by whoever relaunches) is
# preferred: started by LaunchServices, loading the binary straight off the
# external volume waits on a files-and-folders permission.
# A new automation token every launch, readable only by this user (the
# per-user temp dir is 0700); \`scripts/rox-agent\` hands it to gpui-agent.
TOKEN_FILE="\$(getconf DARWIN_USER_TEMP_DIR)rox-gpui-agent-token"
( umask 077 && head -c 32 /dev/urandom | od -An -tx1 | tr -d ' \\n' > "\$TOKEN_FILE" )
export GPUI_AGENT=1 GPUI_AGENT_ALLOW_RELEASE=1 ROX_FPS=0
GPUI_AGENT_TOKEN="\$(cat "\$TOKEN_FILE")"
export GPUI_AGENT_TOKEN
BIN="\$(dirname "\$0")/rox-bin"
[ -x "\$BIN" ] || BIN="$BIN"
exec "\$BIN" "\$@"
SH
chmod +x "$APP/Contents/MacOS/rox"

# The broker and its static files: the office viewer and editor pages, the
# editor's bundle and seed files, the plugin catalog and icons, the trust
# store; mermaid.min.js and the BetterOffice packages the office pages
# import, in node_modules/ beside server/ (where the broker looks).
SERVER="$ROOT/server"
B="$APP/Contents/Resources/broker"
if [ -z "${ROX_BROKER_BIN:-}" ]; then
  (cd "$ROOT/broker" && RUSTC_WRAPPER= cargo build --release -p rox-broker)
  ROX_BROKER_BIN="${CARGO_TARGET_DIR:-$ROOT/broker/target}/release/rox-broker"
fi
# rm first: a copy over a binary keeps its old code signature
rm -rf "$B"
mkdir -p "$B/server/office/editor" "$B/node_modules/mermaid/dist"
cp "$ROX_BROKER_BIN" "$B/rox-broker"
cp "$SERVER/src/office/viewer.html" "$B/server/office/viewer.html"
cp "$SERVER/src/office/editor/editor.html" "$B/server/office/editor/editor.html"
cp -R "$SERVER/src/office/seeds" "$B/server/office/seeds"
DIST="${OFFICE_DIST:-$SERVER/src/office/editor/dist}"
if [ -f "$DIST/editor.js" ]; then
  mkdir -p "$B/server/office/editor/dist"
  cp -R "$DIST/." "$B/server/office/editor/dist/"
else
  echo "warning: no office editor bundle in $DIST (run npm run build:office-editor in server/, or set OFFICE_DIST); the editor won't open" >&2
fi
cp -R "$SERVER/src/plugins" "$B/server/plugins"
cp "$SERVER/src/v1/trust.json" "$B/server/trust.json"
cp "$SERVER/node_modules/mermaid/dist/mermaid.min.js" "$B/node_modules/mermaid/dist/mermaid.min.js"
cp -R "$SERVER/node_modules/@betteroffice" "$B/node_modules/@betteroffice"

# only the real bundle answers rox:// links
if [ "$APP" = "$DEFAULT_APP" ]; then
  "$LSREGISTER" -f "$APP"
fi
echo "bundled $APP -> $BIN (broker: $B)"
