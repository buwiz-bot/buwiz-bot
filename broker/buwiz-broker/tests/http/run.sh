#!/bin/sh
# The HTTP tests: each starts the given buwiz-broker on its own data dir and
# speaks HTTP to it. Plain `node` (24 or later), no packages.
#
#   tests/http/run.sh <buwiz-broker binary> [test file ...]
#
# TEST_PORT (default 8797) is the port each test's broker takes, one at a
# time; move it when another broker holds it. Exits non-zero if any test
# fails, and prints the counts.
set -u
BIN="${1:-}"
[ -x "$BIN" ] || { echo "usage: $0 <buwiz-broker binary> [test file ...]" >&2; exit 2; }
shift
HERE="$(cd "$(dirname "$0")" && pwd)"
BUWIZ_BROKER_BIN="$(cd "$(dirname "$BIN")" && pwd)/$(basename "$BIN")"
export BUWIZ_BROKER_BIN
# Everything below runs from a clean environment: nothing of the caller's
# (a model provider's URL or key, a proxy, a token) reaches a test, its broker
# or the stand-ins it starts. Only what is named here is passed.
if [ -z "${BUWIZ_HTTP_CLEAN:-}" ]; then
  NODE="$(node -p process.execPath)" || exit 2
  GIT="$(command -v git || true)"
  WORK="$(mktemp -d "${TMPDIR:-/tmp}/buwiz-http-run.XXXXXX")" || exit 2
  trap 'rm -rf "$WORK"' EXIT
  # the stubs (osascript, open, secret, nsh, ...) come first, ahead of any real tool
  TOOLS="$(cd "$HERE/../fixtures/bin" && pwd):$(dirname "$NODE")${GIT:+:$(dirname "$GIT")}:/usr/bin:/bin:/usr/sbin:/sbin"
  mkdir -p "$WORK/home" "$WORK/tmp"
  env -i PATH="$TOOLS" HOME="$WORK/home" TMPDIR="$WORK/tmp" LANG=C.UTF-8 \
    TEST_PORT="${TEST_PORT:-8797}" ${TEST_SINK_PORT:+TEST_SINK_PORT="$TEST_SINK_PORT"} \
    BUWIZ_NO_KEYCHAIN=1 BUWIZ_HTTP_CLEAN=1 BUWIZ_BROKER_BIN="$BUWIZ_BROKER_BIN" \
    sh "$0" "$BIN" "$@"
  exit $?
fi
cd "$HERE" || exit 2
[ $# -gt 0 ] || set -- ./*.test.mjs
node --test --test-concurrency=1 "$@"
