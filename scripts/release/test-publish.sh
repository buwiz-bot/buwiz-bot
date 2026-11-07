#!/usr/bin/env bash
# Tests the release scripts offline. Nothing here reaches GitHub or R2: gh
# and aws are stand-ins that record their calls, and origin is a bare repo in
# a temp folder.
#
#   scripts/release/test-publish.sh
set -uo pipefail

HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
TMP="$(mktemp -d)"
trap 'rm -rf "$TMP"' EXIT
FAILED=0

ok() { echo "ok    $1"; }
bad() {
  echo "FAIL  $1" >&2
  FAILED=$((FAILED + 1))
}
same() { # <name> <got> <want>
  if [ "$2" = "$3" ]; then ok "$1"; else bad "$1: got '$2', want '$3'"; fi
}
has() { # <name> <text> <part>
  case "$2" in
    *"$3"*) ok "$1" ;;
    *) bad "$1: no '$3' in: $2" ;;
  esac
}
hasnt() { # <name> <text> <part>
  case "$2" in
    *"$3"*) bad "$1: '$3' in: $2" ;;
    *) ok "$1" ;;
  esac
}

# ---- the version logic ----
source "$HERE/publish.sh"
set +e # publish.sh sets it; these tests expect failures
next() { (plan_next "$1" "$2" "$3" && echo "$VERSION") 2>&1; }
exact() { (plan_exact "$1" "$2" "$3" && echo "$VERSION release=$HAS_RELEASE tag=$HAS_TAG") 2>&1; }

same "no release: Cargo.toml's version" "$(next 0.1.0 "" "")" 0.1.0
same "a release: the next patch" "$(next 0.1.0 0.1.0 0.1.0)" 0.1.1
same "the highest release counts, as a version" "$(next 0.1.9 "$(printf '0.1.9\n0.1.10\n0.1.2')" "")" 0.1.11
same "Cargo.toml ahead of the last release wins" "$(next 0.2.0 0.1.4 0.1.4)" 0.2.0
same "Cargo.toml behind the last release: the next patch" "$(next 0.1.0 0.3.0 0.3.0)" 0.3.1
has "a tag with no release stops the plain path" "$(next 0.1.0 "" 0.1.0)" "'just publish 0.1.0' replaces it"
has "a tag at the next patch stops it too" "$(next 0.1.0 0.1.0 "$(printf '0.1.0\n0.1.1')")" "tag v0.1.1 already exists"
same "explicit, nothing there" "$(exact 0.1.0 "" "")" "0.1.0 release=no tag=no"
same "explicit, release and tag there" "$(exact 0.1.0 0.1.0 0.1.0)" "0.1.0 release=yes tag=yes"
same "explicit, only a tag there" "$(exact 0.1.0 "" 0.1.0)" "0.1.0 release=no tag=yes"
same "explicit, a different version is not a match" "$(exact 0.1.0 0.1.01 0.1.00)" "0.1.0 release=no tag=no"
has "explicit, not a version" "$(exact 1.0 "" "")" "MAJOR.MINOR.PATCH"
has "explicit, a v prefix" "$(exact v0.1.0 "" "")" "MAJOR.MINOR.PATCH"

# ---- publish.sh end to end ----
# A checkout with the real script and a gh that answers from $TMP/releases
# and appends its other calls to $TMP/gh.log.
export PUBLISH_GH="$TMP/gh"
cat > "$PUBLISH_GH" <<'GH'
#!/usr/bin/env bash
case "$1 ${2:-}" in
  "api --paginate")
    [ -z "${GH_DOWN:-}" ] || { echo "gh: not signed in" >&2; exit 1; }
    cat "$GH_STATE/releases"
    ;;
  "release view") echo "  GitHub release: stand-in, published today" ;;
  *) echo "$*" >> "$GH_STATE/gh.log" ;;
esac
GH
chmod +x "$PUBLISH_GH"
export GH_STATE="$TMP"

reset_repo() {
  rm -rf "$TMP/origin.git" "$TMP/work" "$TMP/gh.log"
  : > "$TMP/gh.log"
  : > "$TMP/releases"
  git init --quiet --bare --initial-branch=main "$TMP/origin.git"
  git init --quiet --initial-branch=main "$TMP/work"
  (
    cd "$TMP/work" || exit 1
    git config user.name test
    git config user.email test@example.invalid
    git config core.hooksPath /dev/null
    mkdir -p app broker/buwiz-broker scripts/release
    cp "$HERE/publish.sh" scripts/release/
    printf '[package]\nname = "buwiz-bot"\nversion = "0.1.0"\n\n[dependencies]\nfoo = { version = "9.9.9" }\n' > app/Cargo.toml
    printf '[package]\nname = "buwiz-broker"\nversion = "0.1.0"\n' > broker/buwiz-broker/Cargo.toml
    printf '[[package]]\nname = "a"\nversion = "0.1.0"\n\n[[package]]\nname = "buwiz-bot"\nversion = "0.1.0"\n' > app/Cargo.lock
    printf '[[package]]\nname = "buwiz-broker"\nversion = "0.1.0"\n' > broker/Cargo.lock
    git add -A
    git commit --quiet -m start
    git remote add origin "$TMP/origin.git"
    git push --quiet origin main
  )
}
publish() { (cd "$TMP/work" && scripts/release/publish.sh "$@") 2>&1; }
origin() { git --git-dir="$TMP/origin.git" "$@"; }
snapshot() { (origin for-each-ref; cd "$TMP/work" && git for-each-ref && git status --short && cat "$TMP/gh.log") 2>&1; }

reset_repo
before="$(snapshot)"
out="$(publish next --dry-run)"
has "dry run, first: the version" "$out" "version:  0.1.0 (no release yet"
has "dry run, first: the tag" "$out" "tag:      v0.1.0"
has "dry run, first: no override" "$out" "override: no"
same "dry run changes nothing" "$(snapshot)" "$before"

out="$(GH_DOWN=1 publish next --dry-run; echo "exit=$?")"
has "gh down is an error, not 'no release yet'" "$out" "could not list the releases"
has "gh down exits non-zero" "$out" "exit=1"

out="$(publish next)"
has "first release: starts" "$out" "started the release of v0.1.0"
same "first release: tag on origin" "$(origin tag)" v0.1.0
same "first release: tag is main's head" "$(origin rev-parse 'v0.1.0^{commit}')" "$(origin rev-parse main)"
same "first release: no version commit" "$(origin log --format=%s main)" start
same "first release: the workflow call" "$(cat "$TMP/gh.log")" \
  "workflow run release.yml -R buwiz-bot/buwiz-bot --ref v0.1.0 -f version=0.1.0 -f replace=false"

echo v0.1.0 > "$TMP/releases"
: > "$TMP/gh.log"
out="$(publish next --dry-run)"
has "dry run, existing release: the bump" "$out" "version:  0.1.1 (the patch after the last release, 0.1.0)"
has "dry run, existing release: the commit" "$out" "app 0.1.0, broker 0.1.0 -> 0.1.1"
same "dry run leaves Cargo.toml alone" "$(sed -n 3p "$TMP/work/app/Cargo.toml")" 'version = "0.1.0"'

out="$(publish next)"
has "bump: starts" "$out" "started the release of v0.1.1"
same "bump: main has the release commit" "$(origin log -1 --format=%s main)" "Release v0.1.1"
same "bump: the tag is on it" "$(origin rev-parse 'v0.1.1^{commit}')" "$(origin rev-parse main)"
same "bump: app/Cargo.toml" "$(origin show main:app/Cargo.toml | sed -n 3p)" 'version = "0.1.1"'
same "bump: a dependency's version is left alone" "$(origin show main:app/Cargo.toml | sed -n 6p)" 'foo = { version = "9.9.9" }'
same "bump: the broker" "$(origin show main:broker/buwiz-broker/Cargo.toml | sed -n 3p)" 'version = "0.1.1"'
same "bump: app/Cargo.lock's package" "$(origin show main:app/Cargo.lock | sed -n 7p)" 'version = "0.1.1"'
same "bump: another package in the lock is left alone" "$(origin show main:app/Cargo.lock | sed -n 3p)" 'version = "0.1.0"'
same "bump: broker/Cargo.lock" "$(origin show main:broker/Cargo.lock | sed -n 3p)" 'version = "0.1.1"'

printf 'v0.1.0\nv0.1.1\n' > "$TMP/releases"
: > "$TMP/gh.log"
old_tag="$(origin rev-parse 'v0.1.1^{commit}')"
(cd "$TMP/work" && git commit --quiet --allow-empty -m fix && git push --quiet origin main)
before="$(snapshot)"
out="$(publish exact 0.1.1 --dry-run)"
has "dry run, override: says so" "$out" "override: yes (release: yes, tag: yes)"
has "dry run, override: says what it replaces" "$out" "This REPLACES what is published as 0.1.1"
has "dry run, override: names the old tag commit" "$out" "tag v0.1.1: $old_tag"
same "dry run, override: changes nothing" "$(snapshot)" "$before"

if { : < /dev/tty; } 2>/dev/null; then
  ok "override question skipped: this run has a terminal"
else
  out="$(publish exact 0.1.1)"
  has "override with no terminal stops" "$out" "set PUBLISH_YES=1"
  same "override stopped: the tag stays" "$(origin rev-parse 'v0.1.1^{commit}')" "$old_tag"
fi

out="$(PUBLISH_YES=1 publish exact 0.1.1)"
has "override: says what it replaces" "$out" "This REPLACES what is published as 0.1.1"
has "override: starts" "$out" "started the release of v0.1.1"
same "override: the tag moved to main" "$(origin rev-parse 'v0.1.1^{commit}')" "$(origin rev-parse main)"
same "override: the workflow call" "$(cat "$TMP/gh.log")" \
  "workflow run release.yml -R buwiz-bot/buwiz-bot --ref v0.1.1 -f version=0.1.1 -f replace=true"

: > "$TMP/gh.log"
out="$(publish exact 0.3.0)"
has "explicit new version: no override" "$out" "override: no"
same "explicit new version: main moves to it" "$(origin log -1 --format=%s main)" "Release v0.3.0"
same "explicit new version: the tag" "$(origin rev-parse 'v0.3.0^{commit}')" "$(origin rev-parse main)"

printf 'v0.1.0\nv0.1.1\nv0.3.0\n' > "$TMP/releases"
(cd "$TMP/work" && git checkout --quiet -b side)
out="$(publish next; echo "exit=$?")"
has "off main: refuses" "$out" "not on main"
has "off main: exits non-zero" "$out" "exit=1"
out="$(publish next --dry-run; echo "exit=$?")"
has "off main: a dry run reports it" "$out" "A real run would stop here"
has "off main: a dry run still succeeds" "$out" "exit=0"
(cd "$TMP/work" && git checkout --quiet main && echo x >> app/Cargo.toml)
has "a dirty tree refuses" "$(publish next)" "uncommitted changes"
(cd "$TMP/work" && git checkout --quiet -- app/Cargo.toml && git commit --quiet --allow-empty -m local)
has "unpushed commits refuse" "$(publish next)" "HEAD is not origin/main"

# ---- manifest.py ----
FILES="$TMP/files"
mkdir -p "$FILES/release-file-macos-arm64-dmg"
printf 'not really a dmg' > "$FILES/release-file-macos-arm64-dmg/Buwiz-Bot-0.1.0-macos-arm64.dmg"
PUBLISHED_AT=2026-11-07T00:00:00Z python3 -I "$HERE/manifest.py" "$FILES" 0.1.0 "$TMP/out" > /dev/null
same "manifest: the site's shape" "$(python3 -I -c '
import json, sys
m = json.load(open(sys.argv[1]))
f = m["files"][0]
print(m["version"], m["published_at"], sorted(f), f["platform"], f["arch"], f["kind"], f["size"])' "$TMP/out/manifest.json")" \
  "0.1.0 2026-11-07T00:00:00Z ['arch', 'file', 'kind', 'platform', 'sha256', 'size'] macos arm64 dmg 16"
same "SHA256SUMS checks out" "$(cd "$FILES/release-file-macos-arm64-dmg" && shasum -a 256 -c "$TMP/out/SHA256SUMS")" \
  "Buwiz-Bot-0.1.0-macos-arm64.dmg: OK"
has "manifest: a file of another version" "$(python3 -I "$HERE/manifest.py" "$FILES" 0.2.0 "$TMP/o2" 2>&1)" "carrying 0.2.0"

# ---- r2-publish.sh ----
BIN="$TMP/bin"
mkdir -p "$BIN" "$TMP/sizes"
cat > "$BIN/aws" <<'AWS'
#!/usr/bin/env bash
echo "$*" >> "$R2_LOG"
args=" $* "
case "$args" in
  *" list-objects-v2 "*) cat "$R2_STATE/keys" ;;
  *" head-object "*)
    key="${args##*--key }"
    key="${key%% *}"
    echo "${R2_SIZE:-$(cat "$R2_STATE/sizes/$(basename "$key")")}"
    ;;
  *" cp "*"s3://buwiz-releases/bot/latest.json - "*) cat "$R2_STATE/latest" 2>/dev/null || exit 1 ;;
  *" cp "*)
    n=$#
    wc -c < "${@:$((n - 1)):1}" | tr -d ' ' > "$R2_STATE/sizes/$(basename "${@:$n:1}")"
    ;;
esac
AWS
chmod +x "$BIN/aws"
r2() { # <version> <replace>
  : > "$TMP/r2.log"
  (PATH="$BIN:$PATH" VERSION="$1" REPLACE="$2" R2_BUCKET=buwiz-releases R2_ENDPOINT=http://127.0.0.1:1 \
    R2_LOG="$TMP/r2.log" R2_STATE="$TMP" "$HERE/r2-publish.sh" "$FILES" "$TMP/out") 2>&1
}
puts() { sed -n 's/.*\(s3:\/\/[^ ]*\)$/\1/p' "$TMP/r2.log" | grep -v ' - ' | sed 's|s3://buwiz-releases/||'; }

echo None > "$TMP/keys"
rm -f "$TMP/latest"
out="$(r2 0.1.0 false)"
has "r2: latest moves" "$out" "bot/latest.json -> 0.1.0 (was nothing)"
same "r2: the installer, the manifest, the sums, then latest" "$(puts | grep -v '^bot/latest.json$' | tr '\n' ' ')$(puts | tail -1)" \
  "bot/releases/v0.1.0/Buwiz-Bot-0.1.0-macos-arm64.dmg bot/releases/v0.1.0/manifest.json bot/releases/v0.1.0/SHA256SUMS bot/latest.json"
hasnt "r2: nothing outside bot/" "$(grep -E 's3://|--key|--prefix' "$TMP/r2.log" | grep -v 'bot/')" "s3"

echo "bot/releases/v0.1.0/Buwiz-Bot-0.1.0-macos-arm64.dmg" > "$TMP/keys"
out="$(r2 0.1.0 false; echo "exit=$?")"
has "r2: a version is not overwritten unasked" "$out" "already holds files"
has "r2: that fails the run" "$out" "exit=1"
same "r2: and uploads nothing" "$(puts)" ""

printf 'bot/releases/v0.1.0/Buwiz-Bot-0.1.0-macos-arm64.dmg\tbot/releases/v0.1.0/old-file.dmg\n' > "$TMP/keys"
echo '{"version": "0.1.0"}' > "$TMP/latest"
out="$(r2 0.1.0 true)"
has "r2 replace: latest rewritten" "$out" "bot/latest.json -> 0.1.0 (was 0.1.0)"
has "r2 replace: a leftover goes" "$(grep delete-object "$TMP/r2.log")" "--key bot/releases/v0.1.0/old-file.dmg"
same "r2 replace: only the leftover goes" "$(grep -c delete-object "$TMP/r2.log")" 1

echo '{"version": "0.2.0"}' > "$TMP/latest"
out="$(r2 0.1.0 true)"
has "r2: replacing an older version leaves latest" "$out" "bot/latest.json stays at 0.2.0"
hasnt "r2: and does not write it" "$(puts)" "bot/latest.json"
echo '{"version": "0.1.9"}' > "$TMP/latest"
has "r2: 0.1.10 is newer than 0.1.9" "$(cp "$TMP/out/manifest.json" "$TMP/m.json"; r2 0.1.10 true)" "bot/latest.json -> 0.1.10"

out="$(R2_SIZE=1 r2 0.1.0 true; echo "exit=$?")"
has "r2: a short upload fails" "$out" "the local file is 16"
has "r2: exits non-zero" "$out" "exit=1"

echo
if [ "$FAILED" -eq 0 ]; then
  echo "all passed"
else
  echo "$FAILED failed" >&2
  exit 1
fi
