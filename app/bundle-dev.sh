#!/usr/bin/env bash
# Creates ~/Applications/Rox.app: a dev bundle whose executable execs this
# checkout's debug build (cargo rebuilds are picked up without re-bundling)
# and which declares the `rox://` URL scheme, then registers it with
# LaunchServices so `open "rox://chat/<thread>?m=<turn>"` reaches Rox.
# Launch Rox through it (`open -a ~/Applications/Rox.app`) so links go to the
# running copy instead of starting a second one.
set -euo pipefail

APP="$HOME/Applications/Rox.app"
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
# Dev launcher: the checkout's debug build, with local automation on.
export GPUI_AGENT=1 GPUI_AGENT_ALLOW_RELEASE=1 GPUI_AGENT_TOKEN=dev-secret ROX_FPS=0
exec "$BIN" "\$@"
SH
chmod +x "$APP/Contents/MacOS/rox"

"$LSREGISTER" -f "$APP"
echo "bundled $APP -> $BIN"
