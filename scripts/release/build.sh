#!/usr/bin/env bash
# Builds the release bundle, dist/<CFBundleName>.app: release-profile builds
# of the app, the broker workspace (rox-broker, rox-gws) and nsh (nsh,
# nsh-macos-host), assembled as app/bundle-dev.sh assembles the dev bundle
# but without its dev-only parts: the executable is the app itself (no
# launcher, no gpui-agent token or env exports, nothing that points at a
# checkout), and nothing is registered with LaunchServices. The bundle is
# left as the linker signed it; scripts/release/sign.sh signs it.
#
# Every cargo command goes through scripts/wait-build (one heavy build at a
# time) with RUSTC_WRAPPER= empty: a caching wrapper can hand back a stale
# build.
#
#   RELEASE_OUT=<dir>         where the bundle goes (default dist/)
#   CARGO_TARGET_DIR          the app's target dir (default app/target), and
#                             nsh's unless NSH_TARGET_DIR is set
#   BROKER_TARGET_DIR         the broker workspace's (default broker/target)
#   NSH_TARGET_DIR            nsh's (default CARGO_TARGET_DIR, else nsh/target)
#   ROX_BIN, ROX_BROKER_BIN, ROX_GWS_BIN, NSH_BIN, NSH_HOST_BIN
#                             prebuilt binaries to package instead of
#                             building them
#   ROX_GWS_MANIFEST=<path>   rox-gws's signed manifest (default
#                             broker/rox-gws/manifest.signed.json)
#   OFFICE_DIST, OFFICE_FONTS, MATERIAL_SYMBOLS_TTF   as in bundle-dev.sh
#   NODE_MODULES=<dir>        server/'s node_modules (default server/node_modules)
#   APP_ICON=<path>           AppIcon.icns, or a 1024x1024 PNG to make it
#                             from (default app/resources/AppIcon.icns, else
#                             app/resources/AppIcon.png)
#   MIN_MACOS=<x.y>           LSMinimumSystemVersion (default: the highest
#                             minos among the bundled binaries)
#   BUILD_NUMBER=<n>          CFBundleVersion (default: the version)
set -euo pipefail

source "$(dirname "$0")/lib.sh"

# The identity; keep in step with app/bundle-dev.sh.
APP_NAME="Buwiz Bot"
BUNDLE_ID="com.buwiz.bot"
URL_SCHEMES=(buwiz rox)
COPYRIGHT="Copyright © Goldcoders Corp"

[ "$(uname -m)" = arm64 ] || die "release builds are Apple silicon only (arm64)"

# The version and the binary's name, from app/Cargo.toml: the [package]
# name is the binary's unless a [[bin]] names it.
VERSION="$(awk -F'"' '/^\[/{s=$0} s=="[package]" && /^version *=/{print $2; exit}' "$ROOT/app/Cargo.toml")"
BIN_NAME="$(awk -F'"' '/^\[/{s=$0} s=="[[bin]]" && /^name *=/{print $2; exit}' "$ROOT/app/Cargo.toml")"
[ -n "$BIN_NAME" ] || BIN_NAME="$(awk -F'"' '/^\[/{s=$0} s=="[package]" && /^name *=/{print $2; exit}' "$ROOT/app/Cargo.toml")"
[ -n "$VERSION" ] && [ -n "$BIN_NAME" ] || die "no version or name in app/Cargo.toml"

APP_TARGET="${CARGO_TARGET_DIR:-$ROOT/app/target}"
BROKER_TARGET="${BROKER_TARGET_DIR:-$ROOT/broker/target}"
NSH_TARGET="${NSH_TARGET_DIR:-${CARGO_TARGET_DIR:-$ROOT/nsh/target}}"
cargo_release() { # <workspace dir> <target dir> <cargo args...>
  local dir="$1" target="$2"
  shift 2
  (cd "$dir" && RUSTC_WRAPPER='' CARGO_TARGET_DIR="$target" "$ROOT/scripts/wait-build" cargo build --release --locked "$@")
}

# Inputs first, so a missing one fails before the long builds.
SERVER="$ROOT/server"
NODE_MODULES="${NODE_MODULES:-$SERVER/node_modules}"
OFFICE_FONTS="${OFFICE_FONTS:-$NODE_MODULES/@betteroffice/fonts}"
DIST="${OFFICE_DIST:-$SERVER/src/office/editor/dist}"
[ -d "$NODE_MODULES/@betteroffice" ] || die "no @betteroffice packages in $NODE_MODULES: run npm ci in server/ (or set NODE_MODULES)"
[ -f "$OFFICE_FONTS/assets/Carlito-Regular.ttf" ] || die "no @betteroffice/fonts at $OFFICE_FONTS: run npm ci in server/ (or set OFFICE_FONTS)"
# a release without the editor ships a broken office editor: fail, not warn
[ -f "$DIST/editor.js" ] || die "no office editor bundle in $DIST: run npm run build:office-editor in server/ (or set OFFICE_DIST)"

# The office views' icon font, pinned by hash as in bundle-dev.sh.
SYMBOLS_URL="https://fonts.gstatic.com/s/materialsymbolsoutlined/v382/kJF1BvYX7BgnkSrUwT8OhrdQw4oELdPIeeII9v6oDMzByHX9rA6RzaxHMPdY43zj-jCxv3fzvRNU22ZXGJpEpjC_1v-p_4MrImHCIJIZrDCvHOem.ttf"
SYMBOLS_SHA256=69b4db8f8971c5206adb2d3891787c33eee828271fa56209e05ee0e3202729dd
SYMBOLS="${MATERIAL_SYMBOLS_TTF:-$HOME/Library/Caches/Rox/fonts/MaterialSymbolsOutlined-v382.ttf}"
if [ ! -f "$SYMBOLS" ] && [ -z "${MATERIAL_SYMBOLS_TTF:-}" ]; then
  mkdir -p "$(dirname "$SYMBOLS")"
  curl -fsSL -o "$SYMBOLS.part" "$SYMBOLS_URL" && mv "$SYMBOLS.part" "$SYMBOLS" || rm -f "$SYMBOLS.part"
fi
[ -f "$SYMBOLS" ] && [ "$(shasum -a 256 "$SYMBOLS" | cut -d' ' -f1)" = "$SYMBOLS_SHA256" ] \
  || die "no Material Symbols Outlined font (sha256 $SYMBOLS_SHA256) at $SYMBOLS: download $SYMBOLS_URL there, or set MATERIAL_SYMBOLS_TTF"

ICON="${APP_ICON:-}"
if [ -z "$ICON" ]; then
  for c in "$ROOT/app/resources/AppIcon.icns" "$ROOT/app/resources/AppIcon.png"; do
    [ -f "$c" ] && { ICON="$c"; break; }
  done
fi

# The builds.
if [ -z "${ROX_BIN:-}" ]; then
  cargo_release "$ROOT/app" "$APP_TARGET" --bin "$BIN_NAME"
  ROX_BIN="$APP_TARGET/release/$BIN_NAME"
fi
if [ -z "${ROX_BROKER_BIN:-}" ] || [ -z "${ROX_GWS_BIN:-}" ]; then
  PKGS=()
  [ -n "${ROX_BROKER_BIN:-}" ] || PKGS+=(-p rox-broker)
  [ -n "${ROX_GWS_BIN:-}" ] || PKGS+=(-p rox-gws)
  cargo_release "$ROOT/broker" "$BROKER_TARGET" "${PKGS[@]}"
fi
ROX_BROKER_BIN="${ROX_BROKER_BIN:-$BROKER_TARGET/release/rox-broker}"
ROX_GWS_BIN="${ROX_GWS_BIN:-$BROKER_TARGET/release/rox-gws}"
if [ -z "${NSH_BIN:-}${NSH_HOST_BIN:-}" ]; then
  cargo_release "$ROOT/nsh" "$NSH_TARGET" -p nsh-cli -p nsh-macos-host
  NSH_BIN="$NSH_TARGET/release/nsh"
  NSH_HOST_BIN="$NSH_TARGET/release/nsh-macos-host"
fi
for b in "$ROX_BIN" "$ROX_BROKER_BIN" "$ROX_GWS_BIN" "${NSH_BIN:-}" "${NSH_HOST_BIN:-}"; do
  [ -x "$b" ] || die "missing binary: ${b:-nsh or nsh-macos-host (set both NSH_BIN and NSH_HOST_BIN, or neither)}"
done

# The bundle, from scratch: a copy over a binary keeps its old signature.
APP="$OUT/$APP_NAME.app"
C="$APP/Contents"
rm -rf "$APP"
mkdir -p "$C/MacOS" "$C/Resources/fonts/office/LICENSES"
cp "$ROX_BIN" "$C/MacOS/$BIN_NAME"

cp "$SYMBOLS" "$C/Resources/fonts/MaterialSymbolsOutlined.ttf"
cp "$ROOT/app/fonts/MaterialSymbolsOutlined-LICENSE" "$C/Resources/fonts/MaterialSymbolsOutlined-LICENSE"
cp "$OFFICE_FONTS"/assets/*.ttf "$C/Resources/fonts/office/"
cp "$OFFICE_FONTS"/LICENSES/* "$C/Resources/fonts/office/LICENSES/"
cp "$OFFICE_FONTS/LICENSE" "$C/Resources/fonts/office/LICENSE"

# The broker and its static files: the layout app/src/broker.rs expects.
B="$C/Resources/broker"
mkdir -p "$B/bin" "$B/server/office/editor/dist" "$B/node_modules"
cp "$ROX_BROKER_BIN" "$B/rox-broker"
cp "$NSH_BIN" "$B/bin/nsh"
cp "$NSH_HOST_BIN" "$B/bin/nsh-macos-host"
cp "$ROX_GWS_BIN" "$B/bin/rox-gws"
GWS_MANIFEST="${ROX_GWS_MANIFEST:-$ROOT/broker/rox-gws/manifest.signed.json}"
if [ -f "$GWS_MANIFEST" ]; then
  cp "$GWS_MANIFEST" "$B/bin/rox-gws.manifest.json"
else
  echo "warning: no signed rox-gws manifest at $GWS_MANIFEST (set ROX_GWS_MANIFEST); this release can't provision Google Workspace" >&2
fi
cp "$SERVER/src/office/viewer.html" "$B/server/office/viewer.html"
cp "$SERVER/src/office/editor/editor.html" "$B/server/office/editor/editor.html"
cp -R "$SERVER/src/office/seeds" "$B/server/office/seeds"
cp -R "$DIST/." "$B/server/office/editor/dist/"
cp -R "$SERVER/src/plugins" "$B/server/plugins"
cp "$SERVER/src/v1/trust.json" "$B/server/trust.json"
cp -R "$NODE_MODULES/@betteroffice" "$B/node_modules/@betteroffice"
find "$APP" -name .DS_Store -delete

ICON_KEY=""
if [ -n "$ICON" ]; then
  case "$ICON" in
    *.icns) cp "$ICON" "$C/Resources/AppIcon.icns" ;;
    *.png)
      # iconutil wants every size of an iconset; sips scales the 1024 down
      [ "$(sips -g pixelWidth -g pixelHeight "$ICON" | awk '/pixel/{print $2}' | sort -u)" = 1024 ] \
        || die "$ICON is not 1024x1024"
      SET="$(mktemp -d)/AppIcon.iconset"
      mkdir -p "$SET"
      for s in 16 32 128 256 512; do
        sips -z $s $s "$ICON" --out "$SET/icon_${s}x${s}.png" >/dev/null
        sips -z $((s * 2)) $((s * 2)) "$ICON" --out "$SET/icon_${s}x${s}@2x.png" >/dev/null
      done
      iconutil -c icns "$SET" -o "$C/Resources/AppIcon.icns"
      rm -rf "$(dirname "$SET")"
      ;;
    *) die "APP_ICON must be an .icns or a .png: $ICON" ;;
  esac
  ICON_KEY="<key>CFBundleIconFile</key><string>AppIcon</string>"
else
  cat >&2 <<'WARN'

  ************************************************************************
  *  WARNING: NO APP ICON. app/resources/AppIcon.icns (or AppIcon.png,   *
  *  1024x1024) is missing, so this bundle has the generic app icon.     *
  *  Do not ship it.                                                     *
  ************************************************************************

WARN
fi

# LSMinimumSystemVersion: no lower than any bundled binary's minos, which
# is the oldest macOS the binaries load on at all.
minos_of() { vtool -show-build "$1" 2>/dev/null | awk '$1=="minos"{print $2; exit}'; }
HIGHEST=0
while IFS= read -r -d '' f; do
  is_macho "$f" || continue
  v="$(minos_of "$f")"
  [ -n "$v" ] || continue
  [ "$(printf '%s\n%s\n' "$HIGHEST" "$v" | sort -V | tail -1)" = "$v" ] && HIGHEST="$v"
done < <(find "$APP" -type f -perm -u+x -print0)
MIN="${MIN_MACOS:-$HIGHEST}"
[ "$(printf '%s\n%s\n' "$MIN" "$HIGHEST" | sort -V | head -1)" = "$HIGHEST" ] \
  || die "MIN_MACOS=$MIN is below a bundled binary's minos ($HIGHEST)"

SCHEMES=""
for s in "${URL_SCHEMES[@]}"; do SCHEMES+="<string>$s</string>"; done
cat > "$C/Info.plist" <<PLIST
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>CFBundleIdentifier</key><string>$BUNDLE_ID</string>
  <key>CFBundleName</key><string>$APP_NAME</string>
  <key>CFBundleDisplayName</key><string>$APP_NAME</string>
  <key>CFBundleExecutable</key><string>$BIN_NAME</string>
  <key>CFBundlePackageType</key><string>APPL</string>
  <key>CFBundleInfoDictionaryVersion</key><string>6.0</string>
  <key>CFBundleShortVersionString</key><string>$VERSION</string>
  <key>CFBundleVersion</key><string>${BUILD_NUMBER:-$VERSION}</string>
  <key>LSMinimumSystemVersion</key><string>$MIN</string>
  <key>NSHumanReadableCopyright</key><string>$COPYRIGHT</string>
  <key>NSHighResolutionCapable</key><true/>
  $ICON_KEY
  <!-- the broker's Reminders, Finder and file-picker calls go through osascript -->
  <key>NSAppleEventsUsageDescription</key><string>$APP_NAME uses AppleScript for the Mac actions you ask it for, such as adding a reminder.</string>
  <key>CFBundleURLTypes</key>
  <array>
    <dict>
      <key>CFBundleURLName</key><string>$BUNDLE_ID</string>
      <key>CFBundleURLSchemes</key><array>$SCHEMES</array>
    </dict>
  </array>
</dict>
</plist>
PLIST
plutil -lint -s "$C/Info.plist" || die "bad Info.plist"
printf 'APPL????' > "$C/PkgInfo"

echo "built $APP ($APP_NAME $VERSION, $BUNDLE_ID, macOS $MIN+)"
