#!/usr/bin/env bash
# Prints yes or no: whether GitHub has a release at this tag. Only gh's own
# "release not found" means no. Anything else (a network error, a bad token)
# prints nothing and fails, so it can't read as "not released yet".
#
#   scripts/release/release-exists.sh <tag>
#
# The repository is GH_REPO's. PUBLISH_GH is the gh command (default gh; the
# tests use a stand-in).
set -uo pipefail

GH="${PUBLISH_GH:-gh}"
if answer="$("$GH" release view "$1" 2>&1 >/dev/null)"; then
  echo yes
elif [ "$answer" = "release not found" ]; then
  echo no
else
  echo "release-exists: could not read release $1: $answer" >&2
  exit 1
fi
