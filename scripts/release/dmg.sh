#!/usr/bin/env bash
# Packs the signed (and, for a release, notarized and stapled) bundle into
# a drag-to-Applications dmg with hdiutil, signs the dmg, notarizes and
# staples it (scripts/release/notarize.sh), and writes its checksum sidecar
# beside it. Prints the dmg's sha256 and size.
#
#   scripts/release/dmg.sh [identity] [<bundle>.app]
#
# identity as for sign.sh (default $DEVELOPER_ID_APPLICATION; "-" is an
# ad-hoc dry run, which is not notarized).
#   SIGN_KEYCHAIN=<path>   as for sign.sh
# Writes dist/<Name>-<version>-macos-arm64.dmg and <that>.sha256.
set -euo pipefail

source "$(dirname "$0")/lib.sh"

ID="${1:-${DEVELOPER_ID_APPLICATION:-}}"
[ -n "$ID" ] || die "no identity: pass it, set DEVELOPER_ID_APPLICATION, or pass - for ad hoc"
APP="$(find_app "${2:-}")"
codesign --verify --deep --strict "$APP" || die "$APP is not signed: run scripts/release/sign.sh first"
if [ "$ID" != - ] && ! xcrun stapler validate -q "$APP" 2>/dev/null; then
  echo "warning: $APP has no stapled ticket (scripts/release/notarize.sh it first); Gatekeeper must reach Apple to check it" >&2
fi

VOLNAME="$(plist_get "$APP/Contents/Info.plist" CFBundleName)"
NAME="$(dmg_name "$APP")"
DMG="$OUT/$NAME"
mkdir -p "$OUT"

STAGE="$(mktemp -d)"
trap 'rm -rf "$STAGE"' EXIT
# ditto keeps the signatures, extended attributes and the stapled ticket
ditto "$APP" "$STAGE/$(basename "$APP")"
ln -s /Applications "$STAGE/Applications"
rm -f "$DMG" "$DMG.sha256"
hdiutil create -quiet -volname "$VOLNAME" -srcfolder "$STAGE" -fs HFS+ -format UDZO -imagekey zlib-level=9 -ov "$DMG"

FLAGS=(--force --sign "$ID")
[ "$ID" = - ] && FLAGS+=(--timestamp=none) || FLAGS+=(--timestamp)
[ -n "${SIGN_KEYCHAIN:-}" ] && FLAGS+=(--keychain "$SIGN_KEYCHAIN")
codesign "${FLAGS[@]}" "$DMG"
codesign --verify --strict "$DMG"

if [ "$ID" = - ]; then
  echo "ad-hoc dmg: not notarized" >&2
else
  "$(dirname "$0")/notarize.sh" "$DMG"
fi

# stapling changes the dmg, so the checksum comes last; the sidecar is in
# shasum's format, which the filing app's updater reads (checksum_url)
SHA256="$(shasum -a 256 "$DMG" | cut -d' ' -f1)"
SIZE="$(stat -f %z "$DMG")"
(cd "$OUT" && printf '%s  %s\n' "$SHA256" "$NAME" > "$NAME.sha256")
echo "dmg:    $DMG"
echo "sha256: $SHA256"
echo "size:   $SIZE"
