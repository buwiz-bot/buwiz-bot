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
# no Keychain, whatever a test's broker is asked
BUWIZ_NO_KEYCHAIN=1
export BUWIZ_NO_KEYCHAIN
# the Mac tools the broker shells out to (osascript, open, secret, nsh, ...)
# are the stubs in tests/fixtures/bin, ahead of the real ones
PATH="$(cd "$HERE/../fixtures/bin" && pwd):$PATH"
export PATH
cd "$HERE" || exit 2
[ $# -gt 0 ] || set -- ./*.test.mjs
node --test --test-concurrency=1 "$@"
