#!/usr/bin/env bash
# Creates ~/Applications/Buwiz Bot.app (com.buwiz.bot): a dev bundle whose
# executable execs this checkout's debug build (cargo rebuilds are picked up
# without re-bundling) and which declares the `buwiz://` URL scheme, then
# registers it with LaunchServices so `open "buwiz://chat/<thread>?m=<turn>"`
# reaches it. Rox.app, the bundle before the rename, is unregistered (not
# deleted: that is yours to do), so links stop going to it. Launch the app
# through the bundle (`open -a ~/Applications/Buwiz\ Bot.app`) so links go to
# the running copy instead of starting a second one; copy a fresh build in
# first (`rm`, then
# `cp target/debug/rox ~/Applications/Buwiz\ Bot.app/Contents/MacOS/buwiz-bot-bin`).
#
# Its icon is app/resources/AppIcon.icns, the Buwiz logo.
#
# It also packages the broker Rox starts when none answers (plan 7.2) into
# Contents/Resources/broker/: rox-broker (a release build with
# RUSTC_WRAPPER= unless BUWIZ_BROKER_BIN names one) and the static files it
# serves, `server/` being its --assets and `node_modules/` beside it, and
# in bin/: nsh and nsh-macos-host (widget builds and pop-outs; a Finder-
# launched Rox has no nsh on PATH), release builds of this checkout's nsh
# submodule (`cargo build --release -p nsh-cli -p nsh-macos-host` in nsh/);
# and buwiz-gws, the Google Workspace plugin (buwiz-app/google-workspace at
# the commit plugins/google-workspace/rev pins, built by scripts/gws-build),
# with its signed manifest beside it as buwiz-gws.manifest.json when there
# is one (plugins/google-workspace/manifest.signed.json;
# scripts/gws-install-wizard makes it).
#   ROX_APP=<dir>/<name>.app  bundle elsewhere (a dry run); it is not
#                           registered with LaunchServices, and Rox.app is
#                           left registered
#   BUWIZ_BROKER_BIN=<path>   the rox-broker to package
#   BUWIZ_GWS_BIN=<path>    the buwiz-gws to package
#   BUWIZ_GWS_MANIFEST=<path>  its signed manifest
#   NSH_BIN=<path>, NSH_HOST_BIN=<path>   the nsh and nsh-macos-host to
#                           package (both, or neither)
#   OFFICE_DIST=<dir>       the built office editor bundle (default
#                           server/src/office/editor/dist; built by
#                           `npm run build:office-editor` in server/)
#   MATERIAL_SYMBOLS_TTF=<path>  the office views' icon font to package
#                           (default: fetched once into ~/Library/Caches/Buwiz Bot,
#                           or the copy Rox fetched into ~/Library/Caches/Rox)
#   OFFICE_FONTS=<dir>      the @betteroffice/fonts package whose faces the
#                           Word and sheet views draw with (default
#                           server/node_modules/@betteroffice/fonts)
set -euo pipefail

DEFAULT_APP="$HOME/Applications/Buwiz Bot.app"
OLD_APP="$HOME/Applications/Rox.app"
APP="${ROX_APP:-$DEFAULT_APP}"
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
BIN=/Volumes/goldcoders/Projects/rox/app/target/debug/rox
LSREGISTER=/System/Library/Frameworks/CoreServices.framework/Frameworks/LaunchServices.framework/Support/lsregister

mkdir -p "$APP/Contents/MacOS"

# The office views' icon font (app/src/office/ui/icons.rs): Material
# Symbols Outlined, Google Fonts' static instance (weight 400, grade 0,
# optical size 24, unfilled; Apache-2.0), with its licence, in
# Contents/Resources/fonts/. Not in git: fetched once by its pinned URL
# and checked against its hash.
SYMBOLS_URL="https://fonts.gstatic.com/s/materialsymbolsoutlined/v382/kJF1BvYX7BgnkSrUwT8OhrdQw4oELdPIeeII9v6oDMzByHX9rA6RzaxHMPdY43zj-jCxv3fzvRNU22ZXGJpEpjC_1v-p_4MrImHCIJIZrDCvHOem.ttf"
SYMBOLS_SHA256=69b4db8f8971c5206adb2d3891787c33eee828271fa56209e05ee0e3202729dd
SYMBOLS="${MATERIAL_SYMBOLS_TTF:-$HOME/Library/Caches/Buwiz Bot/fonts/MaterialSymbolsOutlined-v382.ttf}"
OLD_SYMBOLS="$HOME/Library/Caches/Rox/fonts/MaterialSymbolsOutlined-v382.ttf"
# the copy Rox fetched serves until Buwiz Bot has its own
if [ ! -f "$SYMBOLS" ] && [ -z "${MATERIAL_SYMBOLS_TTF:-}" ] && [ -f "$OLD_SYMBOLS" ]; then
  SYMBOLS="$OLD_SYMBOLS"
fi
if [ ! -f "$SYMBOLS" ] && [ -z "${MATERIAL_SYMBOLS_TTF:-}" ]; then
  mkdir -p "$(dirname "$SYMBOLS")"
  curl -fsSL -o "$SYMBOLS.part" "$SYMBOLS_URL" && mv "$SYMBOLS.part" "$SYMBOLS" || rm -f "$SYMBOLS.part"
fi
if [ ! -f "$SYMBOLS" ] || [ "$(shasum -a 256 "$SYMBOLS" | cut -d' ' -f1)" != "$SYMBOLS_SHA256" ]; then
  echo "no Material Symbols Outlined font (sha256 $SYMBOLS_SHA256) at $SYMBOLS: download $SYMBOLS_URL there, or set MATERIAL_SYMBOLS_TTF to a copy; the office views' icons need it" >&2
  exit 1
fi
mkdir -p "$APP/Contents/Resources/fonts"
cp "$SYMBOLS" "$APP/Contents/Resources/fonts/MaterialSymbolsOutlined.ttf"
cp "$ROOT/app/fonts/MaterialSymbolsOutlined-LICENSE" "$APP/Contents/Resources/fonts/MaterialSymbolsOutlined-LICENSE"

# The Word and sheet views' fonts (app/src/office/fonts.rs): every face of
# @betteroffice/fonts (OFL; Carlito for Calibri, Liberation for Arial, ...),
# the ones the web editor measures with, with their licences, in
# Contents/Resources/fonts/office/. Not in git: from server/'s node_modules
# (npm ci).
OFFICE_FONTS="${OFFICE_FONTS:-$ROOT/server/node_modules/@betteroffice/fonts}"
if [ ! -f "$OFFICE_FONTS/assets/Carlito-Regular.ttf" ]; then
  echo "no @betteroffice/fonts at $OFFICE_FONTS: run npm ci in server/ (or set OFFICE_FONTS to the package); the Word and sheet views' fonts need it" >&2
  exit 1
fi
rm -rf "$APP/Contents/Resources/fonts/office"
mkdir -p "$APP/Contents/Resources/fonts/office/LICENSES"
cp "$OFFICE_FONTS"/assets/*.ttf "$APP/Contents/Resources/fonts/office/"
cp "$OFFICE_FONTS"/LICENSES/* "$APP/Contents/Resources/fonts/office/LICENSES/"
cp "$OFFICE_FONTS/LICENSE" "$APP/Contents/Resources/fonts/office/LICENSE"

# The Buwiz logo (app/resources/README.md)
cp "$ROOT/app/resources/AppIcon.icns" "$APP/Contents/Resources/AppIcon.icns"

cat > "$APP/Contents/Info.plist" <<PLIST
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>CFBundleIdentifier</key>
  <string>com.buwiz.bot</string>
  <key>CFBundleName</key>
  <string>Buwiz Bot</string>
  <key>CFBundleDisplayName</key>
  <string>Buwiz Bot</string>
  <key>CFBundleExecutable</key>
  <string>buwiz-bot</string>
  <key>CFBundleIconFile</key>
  <string>AppIcon</string>
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
      <string>com.buwiz.bot</string>
      <key>CFBundleURLSchemes</key>
      <array>
        <string>buwiz</string>
      </array>
    </dict>
  </array>
</dict>
</plist>
PLIST

cat > "$APP/Contents/MacOS/buwiz-bot" <<SH
#!/bin/sh
# Dev launcher: the checkout's debug build, with local automation on. A
# copy inside the bundle (buwiz-bot-bin, refreshed by whoever relaunches) is
# preferred: started by LaunchServices, loading the binary straight off the
# external volume waits on a files-and-folders permission.
# A new automation token every launch, readable only by this user (the
# per-user temp dir is 0700); \`scripts/rox-agent\` hands it to gpui-agent.
TOKEN_FILE="\$(getconf DARWIN_USER_TEMP_DIR)rox-gpui-agent-token"
( umask 077 && head -c 32 /dev/urandom | od -An -tx1 | tr -d ' \\n' > "\$TOKEN_FILE" )
export GPUI_AGENT=1 GPUI_AGENT_ALLOW_RELEASE=1 ROX_FPS=0
GPUI_AGENT_TOKEN="\$(cat "\$TOKEN_FILE")"
export GPUI_AGENT_TOKEN
BIN="\$(dirname "\$0")/buwiz-bot-bin"
[ -x "\$BIN" ] || BIN="$BIN"
exec "\$BIN" "\$@"
SH
chmod +x "$APP/Contents/MacOS/buwiz-bot"

# The broker and its static files: the office viewer and editor pages, the
# editor's bundle and seed files, the plugin catalog and icons, the trust
# store; the BetterOffice packages the office pages import, in
# node_modules/ beside server/ (where the broker looks).
SERVER="$ROOT/server"
B="$APP/Contents/Resources/broker"
BROKER_TARGET="${CARGO_TARGET_DIR:-$ROOT/broker/target}/release"
if [ -z "${BUWIZ_BROKER_BIN:-}" ]; then
  (cd "$ROOT/broker" && RUSTC_WRAPPER= cargo build --release -p rox-broker)
fi
BUWIZ_BROKER_BIN="${BUWIZ_BROKER_BIN:-$BROKER_TARGET/rox-broker}"
BUWIZ_GWS_BIN="${BUWIZ_GWS_BIN:-$("$ROOT/scripts/gws-build")}"
GWS_MANIFEST="${BUWIZ_GWS_MANIFEST:-$ROOT/plugins/google-workspace/manifest.signed.json}"
if [ -z "${NSH_BIN:-}${NSH_HOST_BIN:-}" ]; then
  (cd "$ROOT/nsh" && RUSTC_WRAPPER= cargo build --release -p nsh-cli -p nsh-macos-host)
  NSH_BIN="${CARGO_TARGET_DIR:-$ROOT/nsh/target}/release/nsh"
  NSH_HOST_BIN="${CARGO_TARGET_DIR:-$ROOT/nsh/target}/release/nsh-macos-host"
fi
if [ ! -x "${NSH_BIN:-}" ] || [ ! -x "${NSH_HOST_BIN:-}" ]; then
  echo "no nsh and nsh-macos-host: build the nsh submodule (cargo build --release -p nsh-cli -p nsh-macos-host in nsh/), or set NSH_BIN and NSH_HOST_BIN" >&2
  exit 1
fi
# rm first: a copy over a binary keeps its old code signature
rm -rf "$B"
mkdir -p "$B/bin" "$B/server/office/editor" "$B/node_modules"
cp "$BUWIZ_BROKER_BIN" "$B/rox-broker"
cp "$NSH_BIN" "$B/bin/nsh"
cp "$NSH_HOST_BIN" "$B/bin/nsh-macos-host"
cp "$BUWIZ_GWS_BIN" "$B/bin/buwiz-gws"
if [ -f "$GWS_MANIFEST" ]; then
  cp "$GWS_MANIFEST" "$B/bin/buwiz-gws.manifest.json"
else
  echo "warning: no signed buwiz-gws manifest at $GWS_MANIFEST (scripts/gws-install-wizard signs one, or set BUWIZ_GWS_MANIFEST); Google Workspace won't provision without GWS_MANIFEST" >&2
fi
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
cp -R "$SERVER/node_modules/@betteroffice" "$B/node_modules/@betteroffice"

# only the real bundle answers buwiz:// links; Rox.app (rox://, withdrawn)
# is unregistered so nothing opens it any more
if [ "$APP" = "$DEFAULT_APP" ]; then
  "$LSREGISTER" -f "$APP"
  if [ -d "$OLD_APP" ]; then
    "$LSREGISTER" -u "$OLD_APP" || true
    echo "unregistered $OLD_APP: it is no longer used, and you can delete it (it is not deleted for you)"
  fi
fi
echo "bundled $APP -> $BIN (broker: $B; nsh: $NSH_BIN)"
