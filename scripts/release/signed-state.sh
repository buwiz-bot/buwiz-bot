#!/usr/bin/env bash
# Prints true only if every built target recorded that it was signed, and
# false otherwise: no status, an empty one or a target without one all mean
# not signed. An unsigned build must never reach R2 as a full release.
#
#   scripts/release/signed-state.sh <files-dir> <status-dir>
#
# <files-dir> holds a release-file-* folder per build, <status-dir> a
# release-status-*/signed file per target.
set -uo pipefail

[ "$#" -eq 2 ] || {
  echo "usage: signed-state.sh <files-dir> <status-dir>" >&2
  exit 1
}

builds=0
for dir in "$1"/release-file-*/; do
  [ -d "$dir" ] && builds=$((builds + 1))
done

statuses=0
for file in "$2"/release-status-*/signed; do
  [ -f "$file" ] || continue
  statuses=$((statuses + 1))
  if [ "$(cat "$file")" != true ]; then
    echo false
    exit 0
  fi
done

if [ "$builds" -gt 0 ] && [ "$statuses" -eq "$builds" ]; then
  echo true
else
  echo false
fi
