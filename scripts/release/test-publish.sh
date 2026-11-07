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
# shellcheck disable=SC1091 # checked on its own
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
# no_tty.py runs a command with no controlling terminal; on_tty.py on a new
# one, typing an answer at the [y/N] question if it comes.
cat > "$TMP/no_tty.py" <<'PY'
import os, sys
if os.fork():
    sys.exit(os.waitstatus_to_exitcode(os.wait()[1]))
os.setsid()
os.execvp(sys.argv[1], sys.argv[1:])
PY
cat > "$TMP/on_tty.py" <<'PY'
import os, pty, sys
answer, command = sys.argv[1], sys.argv[2:]
pid, fd = pty.fork()
if pid == 0:
    os.execvp(command[0], command)
out, sent = b"", False
while True:
    try:
        data = os.read(fd, 4096)
    except OSError:
        break
    if not data:
        break
    out += data
    if not sent and b"[y/N]" in out:
        os.write(fd, answer.encode() + b"\n")
        sent = True
sys.stdout.write(out.decode(errors="replace"))
sys.exit(os.waitstatus_to_exitcode(os.wait()[1]))
PY

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
  "secret list")
    [ -z "${GH_SECRETS_DOWN:-}" ] || { echo "gh: HTTP 403" >&2; exit 1; }
    cat "$GH_STATE/secrets"
    ;;
  "release view")
    case " $* " in
      *" --template "*) echo "  GitHub release: stand-in, published today" ;;
      *" --jq "*) cat "$GH_STATE/assets" ;;
      *)
        case "$(cat "$GH_STATE/view")" in
          present) echo "title: stand-in" ;;
          absent) echo "release not found" >&2; exit 1 ;;
          *) echo "HTTP 502: Bad Gateway" >&2; exit 1 ;;
        esac
        ;;
    esac
    ;;
  "api --method") echo "generated notes" ;;
  "release upload")
    echo "$*" >> "$GH_STATE/gh.log"
    [ -z "${GH_UPLOAD_FAILS:-}" ] || { echo "upload failed" >&2; exit 1; }
    ;;
  *) echo "$*" >> "$GH_STATE/gh.log" ;;
esac
GH
chmod +x "$PUBLISH_GH"
export GH_STATE="$TMP"
SIGNING_SECRETS="MACOS_CERT_P12 MACOS_CERT_PASSWORD NOTARY_KEY_ID NOTARY_ISSUER_ID NOTARY_KEY_P8"
R2_SECRETS="R2_ENDPOINT R2_ACCESS_KEY_ID R2_SECRET_ACCESS_KEY"
secrets() { printf '%s\n' "$@" > "$TMP/secrets"; }
# shellcheck disable=SC2086 # the lists split on purpose
all_secrets() { secrets $SIGNING_SECRETS $R2_SECRETS; }
# the public manifest's status code, as the curl stand-in answers it
export RELEASE_CURL="$TMP/curl"
cat > "$RELEASE_CURL" <<'CURL'
#!/usr/bin/env bash
cat "$GH_STATE/http"
CURL
chmod +x "$RELEASE_CURL"

reset_repo() {
  rm -rf "$TMP/origin.git" "$TMP/work" "$TMP/gh.log"
  : > "$TMP/gh.log"
  : > "$TMP/releases"
  all_secrets
  echo 404 > "$TMP/http"
  git init --quiet --bare --initial-branch=main "$TMP/origin.git"
  git init --quiet --initial-branch=main "$TMP/work"
  (
    cd "$TMP/work" || exit 1
    git config user.name test
    git config user.email test@example.invalid
    git config core.hooksPath /dev/null
    mkdir -p app broker/buwiz-broker scripts/release
    cp "$HERE/publish.sh" "$HERE/override-check.sh" scripts/release/
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
publish() { (cd "$TMP/work" && python3 -I "$TMP/no_tty.py" scripts/release/publish.sh "$@") 2>&1; }
publish_tty() { # <answer> <args...>: on a terminal, answering its question
  local answer="$1"
  shift
  (cd "$TMP/work" && python3 -I "$TMP/on_tty.py" "$answer" scripts/release/publish.sh "$@") 2>&1
}
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
  "workflow run release.yml -R buwiz-bot/buwiz-bot --ref v0.1.0 -f version=0.1.0 -f replace=false -f keep_r2=false"

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

out="$(publish exact 0.1.1)"
has "override with no terminal stops" "$out" "set PUBLISH_YES=1"
same "override stopped: the tag stays" "$(origin rev-parse 'v0.1.1^{commit}')" "$old_tag"

# PUBLISH_YES is for a run with no terminal: on one, the question is asked.
out="$(PUBLISH_YES=1 publish_tty n exact 0.1.1)"
has "a terminal asks, even with PUBLISH_YES" "$out" "[y/N]"
has "a terminal's no stops it" "$out" "stopped; nothing changed"
same "that no leaves the tag" "$(origin rev-parse 'v0.1.1^{commit}')" "$old_tag"
same "that no starts nothing" "$(cat "$TMP/gh.log")" ""

out="$(PUBLISH_YES=1 publish exact 0.1.1)"
has "override: says what it replaces" "$out" "This REPLACES what is published as 0.1.1"
has "override: starts" "$out" "started the release of v0.1.1"
same "override: the tag moved to main" "$(origin rev-parse 'v0.1.1^{commit}')" "$(origin rev-parse main)"
same "override: the workflow call" "$(cat "$TMP/gh.log")" \
  "workflow run release.yml -R buwiz-bot/buwiz-bot --ref v0.1.1 -f version=0.1.1 -f replace=true -f keep_r2=false"

: > "$TMP/gh.log"
out="$(publish exact 0.3.0)"
has "explicit new version: no override" "$out" "override: no"
same "explicit new version: main moves to it" "$(origin log -1 --format=%s main)" "Release v0.3.0"
same "explicit new version: the tag" "$(origin rev-parse 'v0.3.0^{commit}')" "$(origin rev-parse main)"

# an override that can't replace what is on R2
printf 'v0.1.0\nv0.1.1\nv0.3.0\n' > "$TMP/releases"
override_setup() { # <public status>
  : > "$TMP/gh.log"
  (cd "$TMP/work" && git commit --quiet --allow-empty -m another && git push --quiet origin main)
  old_tag="$(origin rev-parse 'v0.3.0^{commit}')"
  echo "$1" > "$TMP/http"
}
# shellcheck disable=SC2086
secrets $SIGNING_SECRETS
override_setup 200
out="$(PUBLISH_YES=1 publish exact 0.3.0; echo "exit=$?")"
has "no R2 secrets, files on R2: refused" "$out" "PUBLISH_KEEP_R2=1"
has "that refusal exits non-zero" "$out" "exit=1"
same "that refusal leaves the tag" "$(origin rev-parse 'v0.3.0^{commit}')" "$old_tag"
same "that refusal starts nothing" "$(cat "$TMP/gh.log")" ""
out="$(publish exact 0.3.0 --dry-run)"
has "a dry run shows that refusal" "$out" "PUBLISH_KEEP_R2=1"
has "a dry run says R2 isn't replaced" "$out" "NOT replaced"

# shellcheck disable=SC2086
secrets $R2_SECRETS
has "no signing secrets, files on R2: refused" "$(PUBLISH_YES=1 publish exact 0.3.0)" "PUBLISH_KEEP_R2=1"
override_setup 503
has "can't tell what is on R2: refused" "$(PUBLISH_YES=1 publish exact 0.3.0)" "PUBLISH_KEEP_R2=1"

override_setup 200
out="$(PUBLISH_YES=1 PUBLISH_KEEP_R2=1 publish exact 0.3.0)"
has "kept by choice: it says what really happens" "$out" "NOT replaced"
hasnt "kept by choice: no promise to replace R2" "$out" "and bot/latest.json if"
same "kept by choice: the workflow is told" "$(cat "$TMP/gh.log")" \
  "workflow run release.yml -R buwiz-bot/buwiz-bot --ref v0.3.0 -f version=0.3.0 -f replace=true -f keep_r2=true"

override_setup 404
out="$(PUBLISH_YES=1 publish exact 0.3.0)"
has "no secrets but nothing on R2: goes ahead" "$out" "started the release of v0.3.0"
has "and says R2 isn't touched" "$out" "NOT replaced"

all_secrets
override_setup 200
out="$(PUBLISH_YES=1 publish exact 0.3.0)"
has "all secrets: R2 is replaced" "$out" "R2: bot/releases/v0.3.0/ replaced"
out="$(GH_SECRETS_DOWN=1 PUBLISH_YES=1 publish exact 0.3.0; echo "exit=$?")"
has "unreadable secrets stop an override" "$out" "could not list the secrets"
has "and exit non-zero" "$out" "exit=1"

# A local tag of the version, not on origin: left by a failed push, or one of
# nsh upstream's, which this repo's clones hold. Neither gets in the way.
: > "$TMP/gh.log"
git_work() { git -C "$TMP/work" "$@"; }
git_work commit --quiet --allow-empty -m "after the tag"
git_work push --quiet origin main
git_work tag -a v0.3.1 -m "someone else's" HEAD~1
foreign="$(git_work rev-parse v0.3.1)"
out="$(publish next; echo "exit=$?")"
has "a foreign local tag doesn't stop a release" "$out" "started the release of v0.3.1"
has "and that run succeeds" "$out" "exit=0"
hasnt "and there is no raw git error" "$out" "fatal:"
same "the tag on origin is this commit" "$(origin rev-parse 'v0.3.1^{commit}')" "$(origin rev-parse main)"
same "the local tag is left alone" "$(git_work rev-parse v0.3.1)" "$foreign"
git_work fetch --quiet origin main
git_work reset --quiet --hard FETCH_HEAD

# origin refuses the tag after the bump commit went out: the retry just works
printf 'v0.1.0\nv0.1.1\nv0.3.0\nv0.3.1\n' > "$TMP/releases"
origin config core.hooksPath "$TMP/origin.git/hooks"
printf '#!/bin/sh\ncase "$1" in refs/tags/*) exit 1 ;; esac\n' > "$TMP/origin.git/hooks/update"
chmod +x "$TMP/origin.git/hooks/update"
out="$(publish next; echo "exit=$?")"
has "a refused tag push fails" "$out" "exit=1"
same "the bump commit did go out" "$(origin log -1 --format=%s main)" "Release v0.3.2"
same "the tag didn't" "$(origin tag | grep -c v0.3.2)" 0
rm -f "$TMP/origin.git/hooks/update"
out="$(publish next; echo "exit=$?")"
has "the retry starts the same version" "$out" "started the release of v0.3.2"
has "and succeeds" "$out" "exit=0"
same "with the tag on main" "$(origin rev-parse 'v0.3.2^{commit}')" "$(origin rev-parse main)"

printf 'v0.1.0\nv0.1.1\nv0.3.0\nv0.3.1\nv0.3.2\n' > "$TMP/releases"
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

NEAR="$TMP/near"
near() { # <file name>: manifest.py's verdict on that name for 0.1.1
  rm -rf "$NEAR"
  mkdir -p "$NEAR/release-file-macos-arm64-dmg"
  printf 'x' > "$NEAR/release-file-macos-arm64-dmg/$1"
  if python3 -I "$HERE/manifest.py" "$NEAR" 0.1.1 "$TMP/o3" > /dev/null 2>&1; then echo accepted; else echo refused; fi
}
same "manifest: 0.1.10 is not 0.1.1" "$(near Buwiz-Bot-0.1.10-macos-arm64.dmg)" refused
same "manifest: 10.1.1 is not 0.1.1" "$(near Buwiz-Bot-10.1.1-macos-arm64.dmg)" refused
same "manifest: 0.1.1.5 is not 0.1.1" "$(near Buwiz-Bot-0.1.1.5-macos-arm64.dmg)" refused
same "manifest: the right version" "$(near Buwiz-Bot-0.1.1-macos-arm64.dmg)" accepted
same "manifest: right before the extension" "$(near Buwiz-Bot-0.1.1.dmg)" accepted

# ---- signed-state.sh ----
ST="$TMP/st"
state() { "$HERE/signed-state.sh" "$ST/files" "$ST/status" 2>/dev/null; }
rm -rf "$ST"
mkdir -p "$ST/files/release-file-macos-arm64-dmg"
same "signed: a missing status folder is not signed" "$(state)" false
mkdir -p "$ST/status"
same "signed: an empty status folder is not signed" "$(state)" false
mkdir -p "$ST/status/release-status-macos-arm64"
: > "$ST/status/release-status-macos-arm64/signed"
same "signed: an empty status file is not signed" "$(state)" false
echo false > "$ST/status/release-status-macos-arm64/signed"
same "signed: false is not signed" "$(state)" false
echo true > "$ST/status/release-status-macos-arm64/signed"
same "signed: true is signed" "$(state)" true
mkdir -p "$ST/files/release-file-linux-x64-deb"
same "signed: a build with no status is not signed" "$(state)" false

# ---- release-exists.sh ----
exists() { echo "$1" > "$TMP/view"; "$HERE/release-exists.sh" v0.1.0 2>/dev/null; echo "exit=$?"; }
same "exists: found" "$(exists present)" "yes
exit=0"
same "exists: a real not-found" "$(exists absent)" "no
exit=0"
same "exists: any other failure stops, with no answer" "$(exists down)" "exit=1"

# ---- gh-publish.sh ----
ghp() { # <signed> <replace>
  : > "$TMP/gh.log"
  (VERSION=0.1.0 SIGNED="$1" REPLACE="$2" GH_REPO=test/none "$HERE/gh-publish.sh" "$FILES" "$TMP/out") 2>&1
  echo "exit=$?"
}
echo absent > "$TMP/view"
: > "$TMP/assets"
ghp true false > "$TMP/ghp.out"
has "gh: a new release is created" "$(cat "$TMP/gh.log")" "release create v0.1.0"
hasnt "gh: signed is not a pre-release" "$(cat "$TMP/gh.log")" "--prerelease"
ghp false false > "$TMP/ghp.out"
has "gh: unsigned is a pre-release" "$(cat "$TMP/gh.log")" "--prerelease"

echo present > "$TMP/view"
printf 'Buwiz-Bot-0.1.0-macos-arm64.dmg\nold-file.dmg\nmanifest.json\nSHA256SUMS\n' > "$TMP/assets"
ghp true true > "$TMP/ghp.out"
has "gh replace: succeeds" "$(cat "$TMP/ghp.out")" "exit=0"
log="$(cat "$TMP/gh.log")"
hasnt "gh replace: the release is never deleted" "$log" "release delete v0.1.0"
hasnt "gh replace: nor created again" "$log" "release create"
has "gh replace: files go up over the old ones" "$log" "--clobber"
has "gh replace: a leftover asset goes" "$log" "release delete-asset v0.1.0 old-file.dmg"
same "gh replace: only the leftover goes" "$(grep -c delete-asset "$TMP/gh.log")" 1
has "gh replace: it stops being a pre-release" "$log" "--prerelease=false"
ghp false true > "$TMP/ghp.out"
has "gh replace: unsigned becomes a pre-release" "$(cat "$TMP/gh.log")" "--prerelease=true"

GH_UPLOAD_FAILS=1 ghp true true > "$TMP/ghp.out"
has "gh replace: a failed upload fails the step" "$(cat "$TMP/ghp.out")" "exit=1"
hasnt "gh replace: and leaves the release" "$(cat "$TMP/gh.log")" "delete"
ghp true false > "$TMP/ghp.out"
has "gh: an existing release, unasked, is refused" "$(cat "$TMP/ghp.out")" "exit=1"
hasnt "gh: and is left alone" "$(cat "$TMP/gh.log")" "release upload"
echo down > "$TMP/view"
ghp true true > "$TMP/ghp.out"
has "gh: an unreadable release stops" "$(cat "$TMP/ghp.out")" "exit=1"
same "gh: and writes nothing" "$(cat "$TMP/gh.log")" ""

# ---- summary.sh ----
sum() { # <signed> <replace> <r2 ready> <r2 outcome> <gh outcome>
  VERSION=0.1.0 SIGNED="$1" REPLACE="$2" R2_READY="$3" R2_OUTCOME="$4" GH_OUTCOME="$5" "$HERE/summary.sh" 2>&1
}
# shellcheck disable=SC2016 # literal backticks
has "summary: uploaded" "$(sum true false true success success)" 'On R2 under `bot/releases/v0.1.0/`'
has "summary: a new version off R2" "$(sum true false false skipped success)" "Not on R2"
out="$(sum false true true skipped success)"
has "summary: an override off R2 says the old files stay" "$out" "stay live"
hasnt "summary: and doesn't say it isn't on R2" "$out" "Not on R2"
has "summary: a failed upload is not 'on R2'" "$(sum true false true failure skipped)" "R2 upload failed"
has "summary: GitHub left alone after that" "$(sum true true true failure skipped)" "GitHub release not touched"
has "summary: a failed replace says the release is still up" "$(sum true true true success failure)" "stays up"

# ---- the workflow calls these ----
wf="$(cat "$HERE/../../.github/workflows/release.yml")"
hasnt "workflow: no inline signed check" "$wf" "grep -rqvx"
hasnt "workflow: no inline release delete" "$wf" "gh release delete"
has "workflow: signed-state.sh" "$wf" "scripts/release/signed-state.sh files status"
# shellcheck disable=SC2016 # a literal $VERSION
has "workflow: release-exists.sh in the preflight" "$wf" 'scripts/release/release-exists.sh "v$VERSION"'
has "workflow: gh-publish.sh" "$wf" "scripts/release/gh-publish.sh files out"
has "workflow: override-check.sh in the preflight" "$wf" "scripts/release/override-check.sh"
has "workflow: summary.sh" "$wf" "scripts/release/summary.sh"

# ---- r2-publish.sh ----
BIN="$TMP/bin"
mkdir -p "$BIN" "$TMP/sizes"
cat > "$BIN/aws" <<'AWS'
#!/usr/bin/env bash
echo "$*" >> "$R2_LOG"
args=" $* "
case "$args" in
  *" list-objects-v2 "*)
    [ -z "${R2_LIST_FAILS:-}" ] || { echo "An error occurred (AccessDenied)" >&2; exit 254; }
    cat "$R2_STATE/keys"
    ;;
  *" get-object "*)
    [ -z "${R2_READ_FAILS:-}" ] || { echo "An error occurred (InternalError) when calling the GetObject operation" >&2; exit 254; }
    [ -f "$R2_STATE/latest" ] || { echo "An error occurred (NoSuchKey) when calling the GetObject operation: The specified key does not exist." >&2; exit 254; }
    cp "$R2_STATE/latest" "${@:$#:1}"
    ;;
  *" head-object "*)
    key="${args##*--key }"
    key="${key%% *}"
    echo "${R2_SIZE:-$(cat "$R2_STATE/sizes/$(basename "$key")")}"
    ;;
  *" cp "*"s3://buwiz-releases/bot/latest.json - "*)
    [ -z "${R2_READ_FAILS:-}" ] || exit 1
    cat "$R2_STATE/latest" 2>/dev/null || exit 1
    ;;
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

# a failed listing is not an empty folder
echo None > "$TMP/keys"
out="$(R2_LIST_FAILS=1 r2 0.1.0 false; echo "exit=$?")"
has "r2: a failed listing stops the guard" "$out" "could not list"
has "r2: exits non-zero" "$out" "exit=1"
same "r2: and uploads nothing" "$(puts)" ""
out="$(R2_LIST_FAILS=1 r2 0.1.0 true; echo "exit=$?")"
has "r2: on a replace too" "$out" "could not list"

# only a real not-found means there is no latest.json
echo '{"version": "0.2.0"}' > "$TMP/latest"
out="$(R2_READ_FAILS=1 r2 0.1.0 true; echo "exit=$?")"
has "r2: an unreadable latest.json stops" "$out" "could not read bot/latest.json"
has "r2: that exits non-zero" "$out" "exit=1"
same "r2: before anything is written" "$(puts)" ""
echo 'not json' > "$TMP/latest"
out="$(r2 0.1.0 true; echo "exit=$?")"
has "r2: a broken latest.json stops" "$out" "bot/latest.json has no version"
same "r2: and writes nothing" "$(puts)" ""
rm -f "$TMP/latest"
has "r2: a real not-found is nothing there" "$(r2 0.1.0 true)" "(was nothing)"

echo
if [ "$FAILED" -eq 0 ]; then
  echo "all passed"
else
  echo "$FAILED failed" >&2
  exit 1
fi
