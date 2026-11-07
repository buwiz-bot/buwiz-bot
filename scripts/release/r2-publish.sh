#!/usr/bin/env bash
# Uploads a built release to Cloudflare R2, the last step before the site
# sees it. Everything stays under bot/ in the bucket, which other apps share:
#
#   bot/releases/v<version>/<file>         the installers
#   bot/releases/v<version>/manifest.json  scripts/release/manifest.py's
#   bot/releases/v<version>/SHA256SUMS
#   bot/latest.json                        what buwiz.com reads
#
# The files go first and latest.json last, so it never names a file that
# isn't there yet. A replaced version keeps its URLs, so nothing is cached
# for long, and whatever the new build doesn't have is removed. latest.json
# only ever moves forward: replacing an older version leaves it alone.
#
#   scripts/release/r2-publish.sh <files-dir> <out-dir>
#
# <files-dir> as for manifest.py, <out-dir> holding its manifest.json and
# SHA256SUMS. Environment: VERSION, REPLACE (true or false), R2_BUCKET,
# R2_ENDPOINT, and the aws CLI's credentials.
set -euo pipefail

die() {
  echo "r2-publish: $*" >&2
  exit 1
}

[ "$#" -eq 2 ] || die "usage: r2-publish.sh <files-dir> <out-dir>"
FILES="$1" MANIFEST_DIR="$2"
: "${VERSION:?VERSION is required}"
: "${REPLACE:?REPLACE is required}"
: "${R2_BUCKET:?R2_BUCKET is required}"
: "${R2_ENDPOINT:?R2_ENDPOINT is required}"

PREFIX="bot/releases/v$VERSION"
CACHE="public, max-age=300"

s3api() { aws s3api --endpoint-url "$R2_ENDPOINT" "$@"; }
s3cp() { aws s3 cp --endpoint-url "$R2_ENDPOINT" --no-progress "$@"; }

keys() { # <prefix>: every key under it, one per line
  s3api list-objects-v2 --bucket "$R2_BUCKET" --prefix "$1" --query 'Contents[].Key' --output text |
    tr '\t' '\n' | { grep -vx 'None' || true; }
}

content_type() {
  case "$1" in
    *.dmg) echo application/x-apple-diskimage ;;
    *.json) echo application/json ;;
    SHA256SUMS) echo "text/plain; charset=utf-8" ;;
    *) echo application/octet-stream ;;
  esac
}

put() { # <file>: into the version's folder, then its size is checked
  local path="$1" name size remote
  name="$(basename "$path")"
  size="$(wc -c < "$path" | tr -d '[:space:]')"
  s3cp --cache-control "$CACHE" --content-type "$(content_type "$name")" "$path" "s3://$R2_BUCKET/$PREFIX/$name"
  remote="$(s3api head-object --bucket "$R2_BUCKET" --key "$PREFIX/$name" --query ContentLength --output text)"
  [ "$remote" = "$size" ] || die "$PREFIX/$name is $remote bytes in R2, the local file is $size"
}

if [ "$REPLACE" != true ] && [ -n "$(keys "$PREFIX/")" ]; then
  die "$PREFIX/ already holds files; 'just publish $VERSION' replaces them"
fi

WANTED=(manifest.json SHA256SUMS)
for path in "$FILES"/release-file-*/*; do
  put "$path"
  WANTED+=("$(basename "$path")")
done
put "$MANIFEST_DIR/manifest.json"
put "$MANIFEST_DIR/SHA256SUMS"

# what an earlier build of this version left that this one doesn't have
for key in $(keys "$PREFIX/"); do
  name="${key#"$PREFIX/"}"
  printf '%s\n' "${WANTED[@]}" | grep -Fxq -- "$name" && continue
  echo "removing $key, which this build doesn't have"
  s3api delete-object --bucket "$R2_BUCKET" --key "$key"
done

CURRENT="$(s3cp "s3://$R2_BUCKET/bot/latest.json" - 2>/dev/null |
  python3 -c 'import json, sys; print(json.load(sys.stdin)["version"])' 2>/dev/null || true)"
if [ -n "$CURRENT" ] && [ "$(printf '%s\n%s\n' "$CURRENT" "$VERSION" | sort -V | tail -1)" != "$VERSION" ]; then
  echo "bot/latest.json stays at $CURRENT, newer than $VERSION"
else
  s3cp --cache-control "public, max-age=60" --content-type application/json \
    "$MANIFEST_DIR/manifest.json" "s3://$R2_BUCKET/bot/latest.json"
  echo "bot/latest.json -> $VERSION (was ${CURRENT:-nothing})"
fi
