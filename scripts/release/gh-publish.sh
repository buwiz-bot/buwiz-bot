#!/usr/bin/env bash
# Creates the GitHub release, or updates an existing one in place when
# REPLACE is true. It never deletes the release: a failure partway leaves it
# up, though it can then hold a mix of old and new files.
#
#   scripts/release/gh-publish.sh <files-dir> <out-dir>
#
# <files-dir> and <out-dir> as for r2-publish.sh. Environment: VERSION,
# SIGNED and REPLACE (true or false), GH_REPO, and gh's token. PUBLISH_GH is
# the gh command (default gh; the tests use a stand-in).
set -euo pipefail

GH="${PUBLISH_GH:-gh}"
HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"

die() {
  echo "gh-publish: $*" >&2
  exit 1
}

[ "$#" -eq 2 ] || die "usage: gh-publish.sh <files-dir> <out-dir>"
: "${VERSION:?VERSION is required}"
: "${SIGNED:?SIGNED is required}"
: "${REPLACE:?REPLACE is required}"
: "${GH_REPO:?GH_REPO is required}"

TAG="v$VERSION"
TITLE="Buwiz Bot $VERSION"
NOTE=""
PRERELEASE=false
if [ "$SIGNED" != true ]; then
  NOTE="Unsigned build: macOS blocks it until you allow it in System Settings."
  PRERELEASE=true
fi
ASSETS=("$1"/release-file-*/* "$2/manifest.json" "$2/SHA256SUMS")

EXISTS="$("$HERE/release-exists.sh" "$TAG")"

if [ "$EXISTS" = no ]; then
  ARGS=(--verify-tag --title "$TITLE" --generate-notes)
  if [ "$PRERELEASE" = true ]; then ARGS+=(--prerelease --notes "$NOTE"); fi
  "$GH" release create "$TAG" "${ARGS[@]}" "${ASSETS[@]}"
  exit 0
fi

[ "$REPLACE" = true ] || die "$TAG is already released; 'just publish $VERSION' replaces it"

"$GH" release upload "$TAG" --clobber "${ASSETS[@]}"

# what an earlier build left that this one doesn't have
WANTED=""
for path in "${ASSETS[@]}"; do
  WANTED="$WANTED$(basename "$path")"$'\n'
done
for name in $("$GH" release view "$TAG" --json assets --jq '.assets[].name'); do
  printf '%s' "$WANTED" | grep -Fxq -- "$name" && continue
  "$GH" release delete-asset "$TAG" "$name" --yes
done

NOTES="$("$GH" api --method POST "repos/$GH_REPO/releases/generate-notes" -f tag_name="$TAG" --jq .body)"
"$GH" release edit "$TAG" --title "$TITLE" --prerelease="$PRERELEASE" --notes "${NOTE:+$NOTE$'\n\n'}$NOTES"
