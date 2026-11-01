#!/usr/bin/env bash
# Signs the release bundle with the hardened runtime, inside out: every
# loose Mach-O (the broker, nsh, nsh-macos-host, buwiz-gws, anything under
# Resources/), deepest first; then any nested bundle (frameworks, plugins,
# helper apps), deepest first; then the app. Only the app's executable and
# buwiz-broker get app.entitlements; the rest are signed with none.
#
#   scripts/release/sign.sh [identity] [<bundle>.app]
#
# identity: "Developer ID Application: <Team> (<TEAMID>)" or its hash,
# default $DEVELOPER_ID_APPLICATION; "-" signs ad hoc (a dry run: no
# timestamp, not notarizable). The bundle defaults to the one in dist/.
#   SIGN_KEYCHAIN=<path>   the keychain holding the identity (CI's temp one)
set -euo pipefail

source "$(dirname "$0")/lib.sh"

ID="${1:-${DEVELOPER_ID_APPLICATION:-}}"
[ -n "$ID" ] || die "no identity: pass it, set DEVELOPER_ID_APPLICATION, or pass - for ad hoc"
APP="$(find_app "${2:-}")"
PLIST="$APP/Contents/Info.plist"
BUNDLE_ID="$(plist_get "$PLIST" CFBundleIdentifier)"
EXE="$APP/Contents/MacOS/$(plist_get "$PLIST" CFBundleExecutable)"
[ -n "$BUNDLE_ID" ] && [ -f "$EXE" ] || die "$PLIST names no bundle id or no executable that exists"
ENTITLEMENTS="$(dirname "$0")/app.entitlements"
# the broker spawns osascript on the app's behalf (app/src/broker.rs places it)
ENTITLED=("$APP/Contents/Resources/broker/buwiz-broker")

FLAGS=(--force --options runtime --sign "$ID")
if [ "$ID" = - ]; then
  FLAGS+=(--timestamp=none)
else
  FLAGS+=(--timestamp)
fi
[ -n "${SIGN_KEYCHAIN:-}" ] && FLAGS+=(--keychain "$SIGN_KEYCHAIN")

entitled() {
  local e
  for e in "${ENTITLED[@]}"; do [ "$1" = "$e" ] && return 0; done
  return 1
}

# deepest first: a path's depth is its count of slashes
by_depth() { awk -F/ '{print NF "\t" $0}' | sort -rn -k1,1 | cut -f2-; }

NESTED='.*\.(framework|app|appex|xpc|bundle|plugin)$'
n=0
while IFS= read -r f; do
  [ "$f" = "$EXE" ] && continue
  is_macho "$f" || continue
  name="$(basename "$f")"
  if entitled "$f"; then
    codesign "${FLAGS[@]}" --identifier "$BUNDLE_ID.$name" --entitlements "$ENTITLEMENTS" "$f"
  else
    codesign "${FLAGS[@]}" --identifier "$BUNDLE_ID.$name" "$f"
  fi
  n=$((n + 1))
done < <(find "$APP/Contents" -type f ! -path "$APP/Contents/_CodeSignature/*" | by_depth)

while IFS= read -r b; do
  codesign "${FLAGS[@]}" "$b"
  n=$((n + 1))
done < <(find -E "$APP/Contents" -depth -type d -regex "$NESTED" | by_depth)

codesign "${FLAGS[@]}" --entitlements "$ENTITLEMENTS" "$APP"
codesign --verify --deep --strict --verbose=2 "$APP"
echo "signed $APP and $n nested item(s) as $ID"
