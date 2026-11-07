#!/usr/bin/env bash
# Stops an override that can't replace what is on R2, so the public download
# can't silently stop matching the tag. An unsigned build or missing R2
# secrets mean R2 won't be written; if R2 holds this version, or that can't
# be told, this fails unless keep-r2 is yes.
#
#   scripts/release/override-check.sh <version> <signing yes|no> <r2 yes|no> <keep-r2 yes|no>
#
# It reads the public address, so it needs no secrets. RELEASE_CURL is the
# curl command (default curl; the tests use a stand-in).
set -uo pipefail

[ "$#" -eq 4 ] || {
  echo "usage: override-check.sh <version> <signing> <r2> <keep-r2>" >&2
  exit 1
}
VERSION="$1" SIGNING="$2" R2="$3" KEEP="$4"
CURL="${RELEASE_CURL:-curl}"
URL="https://downloads.buwiz.com/bot/releases/v$VERSION/manifest.json"

if [ "$SIGNING" = yes ] && [ "$R2" = yes ]; then
  exit 0
fi

CODE="$("$CURL" -s -o /dev/null -w '%{http_code}' --max-time 20 "$URL" || true)"
case "$CODE" in
  404) exit 0 ;;
  200) HELD="R2 holds $VERSION" ;;
  *) HELD="R2 may hold $VERSION (its manifest answered ${CODE:-nothing})" ;;
esac

if [ "$KEEP" = yes ]; then
  echo "override-check: $HELD; it stays live, by choice, and won't match the tag"
  exit 0
fi
echo "override-check: $HELD, and this run can't replace it (signing secrets: $SIGNING, R2 secrets: $R2). Add the secrets, or set PUBLISH_KEEP_R2=1 to leave those files live though they won't match the tag." >&2
exit 1
