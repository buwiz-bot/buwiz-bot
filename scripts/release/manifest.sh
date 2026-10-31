#!/usr/bin/env bash
# Writes dist/latest.json for the dmg, in the filing app's manifest shape
# (its scripts/publish_update_manifest.py and bir-shell's updater.rs):
#
#   {"version": "<semver>",
#    "platforms": {"<key>": {"url", "sha256", "name", "checksum_url"}}}
#
# with one platform, darwin-arm64 (Apple silicon only). The updater
# accepts a download only when its hash, the manifest's sha256 and the
# checksum sidecar's line all agree, so checksum_url is always set.
# It uploads nothing: it prints the commands that would, for the owner.
#
#   scripts/release/manifest.sh [<file>.dmg]
#
#   RELEASE_BASE_URL   where the release's files are served (default
#                      https://downloads.buwiz.com/bot); each release's
#                      files go under <base>/<version>/, latest.json at
#                      <base>/latest.json
#   VERSION            default: the bundle's CFBundleShortVersionString
set -euo pipefail

source "$(dirname "$0")/lib.sh"

BASE="${RELEASE_BASE_URL:-https://downloads.buwiz.com/bot}"
BASE="${BASE%/}"
if [ -n "${1:-}" ]; then
  DMG="$1"
else
  dmgs=("$OUT"/*.dmg)
  [ -f "${dmgs[0]}" ] && [ "${#dmgs[@]}" -eq 1 ] || die "pass the dmg (dist/ has none, or more than one)"
  DMG="${dmgs[0]}"
fi
[ -f "$DMG" ] && [ -f "$DMG.sha256" ] || die "no $DMG and its .sha256 sidecar: run scripts/release/dmg.sh"
NAME="$(basename "$DMG")"
VERSION="${VERSION:-$(plist_get "$(find_app)/Contents/Info.plist" CFBundleShortVersionString)}"
[[ "$VERSION" =~ ^[0-9]+\.[0-9]+\.[0-9]+(-[0-9A-Za-z.-]+)?$ ]] || die "not a semver: '$VERSION'"
case "$NAME" in *"-$VERSION-"*) ;; *) die "$NAME is not version $VERSION's dmg" ;; esac

SHA256="$(shasum -a 256 "$DMG" | cut -d' ' -f1)"
[ "$(cut -d' ' -f1 "$DMG.sha256")" = "$SHA256" ] || die "$DMG.sha256 does not match the dmg"
SIZE="$(stat -f %z "$DMG")"
URL="$BASE/$VERSION/$NAME"

# json.dumps(indent=2), as the filing app's publisher writes it
python3 -I - "$VERSION" "$URL" "$SHA256" "$NAME" "$URL.sha256" > "$OUT/latest.json" <<'PY'
import json, sys
version, url, sha256, name, checksum_url = sys.argv[1:]
manifest = {"version": version, "platforms": {"darwin-arm64": {
    "url": url, "sha256": sha256, "name": name, "checksum_url": checksum_url}}}
sys.stdout.write(json.dumps(manifest, indent=2) + "\n")
PY

cat "$OUT/latest.json"
cat <<EOF
# wrote $OUT/latest.json ($NAME: $SIZE bytes, sha256 $SHA256)
# Not uploaded. To publish (R2 bucket buwiz-releases, under bot/), the dmg
# and its sidecar first and latest.json last, so the manifest never names
# a file that isn't there yet:
#   wrangler r2 object put "buwiz-releases/bot/$VERSION/$NAME" --file "$DMG" --content-type application/x-apple-diskimage --remote
#   wrangler r2 object put "buwiz-releases/bot/$VERSION/$NAME.sha256" --file "$DMG.sha256" --content-type text/plain --remote
#   wrangler r2 object put "buwiz-releases/bot/latest.json" --file "$OUT/latest.json" --content-type application/json --cache-control no-cache --remote
# or with rclone (a remote named r2):
#   rclone copyto "$DMG" "r2:buwiz-releases/bot/$VERSION/$NAME"
#   rclone copyto "$DMG.sha256" "r2:buwiz-releases/bot/$VERSION/$NAME.sha256"
#   rclone copyto "$OUT/latest.json" "r2:buwiz-releases/bot/latest.json" --header-upload "Cache-Control: no-cache"
EOF
