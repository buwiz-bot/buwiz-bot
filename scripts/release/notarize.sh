#!/usr/bin/env bash
# Notarizes a signed .app or .dmg with notarytool (an App Store Connect API
# key), waits for the verdict, then staples the ticket to it.
#
#   scripts/release/notarize.sh <bundle>.app | <file>.dmg
#
#   NOTARY_KEY_ID      the API key's Key ID
#   NOTARY_ISSUER_ID   its Issuer ID
#   NOTARY_KEY_P8      the AuthKey_<id>.p8 file's contents, base64-encoded
# Without all three it says so and skips (exit 0), so local dry runs go on;
# REQUIRE_NOTARIZATION=1 (as CI sets) makes that an error instead.
set -euo pipefail

source "$(dirname "$0")/lib.sh"

TARGET="${1:-}"
[ -e "$TARGET" ] || die "usage: notarize.sh <bundle>.app | <file>.dmg"

missing=()
for v in NOTARY_KEY_ID NOTARY_ISSUER_ID NOTARY_KEY_P8; do
  [ -n "${!v:-}" ] || missing+=("$v")
done
if [ "${#missing[@]}" -gt 0 ]; then
  [ "${REQUIRE_NOTARIZATION:-0}" = 1 ] && die "notarization required, but ${missing[*]} unset"
  echo "SKIPPING NOTARIZATION of $(basename "$TARGET"): ${missing[*]} unset. Not distributable as is." >&2
  exit 0
fi

WORK="$(mktemp -d)"
trap 'rm -rf "$WORK"' EXIT
KEY="$WORK/AuthKey_$NOTARY_KEY_ID.p8"
(umask 077 && printf '%s' "$NOTARY_KEY_P8" | base64 --decode > "$KEY")
grep -q "BEGIN PRIVATE KEY" "$KEY" || die "NOTARY_KEY_P8 is not a base64-encoded .p8 key"
AUTH=(--key "$KEY" --key-id "$NOTARY_KEY_ID" --issuer "$NOTARY_ISSUER_ID")

case "$TARGET" in
  *.app)
    # notarytool takes a zip, dmg or pkg; the ticket is stapled to the app
    UPLOAD="$WORK/$(basename "$TARGET" .app).zip"
    ditto -c -k --sequesterRsrc --keepParent "$TARGET" "$UPLOAD"
    ;;
  *.dmg) UPLOAD="$TARGET" ;;
  *) die "not an .app or .dmg: $TARGET" ;;
esac

echo "notarizing $(basename "$TARGET") (this waits for Apple)"
xcrun notarytool submit "$UPLOAD" "${AUTH[@]}" --wait --timeout 2h --output-format json > "$WORK/result.json" \
  || true
STATUS="$(plutil -extract status raw -o - "$WORK/result.json" 2>/dev/null || true)"
ID="$(plutil -extract id raw -o - "$WORK/result.json" 2>/dev/null || true)"
if [ "$STATUS" != Accepted ]; then
  cat "$WORK/result.json" >&2
  [ -n "$ID" ] && xcrun notarytool log "$ID" "${AUTH[@]}" >&2 || true
  die "notarization of $(basename "$TARGET") ended ${STATUS:-without a status}"
fi

xcrun stapler staple "$TARGET"
xcrun stapler validate "$TARGET"
echo "notarized and stapled $TARGET (submission $ID)"
