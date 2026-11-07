#!/usr/bin/env bash
# The command behind `just publish`: picks the version, moves app/Cargo.toml
# to it if needed, tags it and starts .github/workflows/release.yml. It is
# the only thing that starts a release build. Run it on a clean, up-to-date
# main.
#
#   publish.sh next [--dry-run]           `just publish`
#       No release yet: app/Cargo.toml's version. Otherwise the last
#       release's patch + 1, or app/Cargo.toml's version if that is already
#       ahead of it. Never replaces anything: it stops if the tag exists.
#
#   publish.sh exact X.Y.Z [--dry-run]    `just publish X.Y.Z`
#       That version. An existing tag and release of it are replaced: it
#       says what it is replacing and asks first (with no terminal to ask
#       on, PUBLISH_YES=1 goes ahead). If the repo's secrets mean R2 can't be
#       replaced and R2 holds this version, it stops: PUBLISH_KEEP_R2=1 goes
#       ahead and leaves those files live, not matching the tag.
#
# --dry-run prints the version, the tag and whether it would override, and
# changes nothing.
#
#   PUBLISH_GH   the gh command (default gh; the tests use a stand-in)
#   RELEASE_CURL the curl command, likewise
set -euo pipefail

REPO=buwiz-bot/buwiz-bot
WORKFLOW=release.yml
GH="${PUBLISH_GH:-gh}"
SIGNING_SECRETS="MACOS_CERT_P12 MACOS_CERT_PASSWORD NOTARY_KEY_ID NOTARY_ISSUER_ID NOTARY_KEY_P8"
R2_SECRETS="R2_ENDPOINT R2_ACCESS_KEY_ID R2_SECRET_ACCESS_KEY"
ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"

die() {
  echo "publish: $*" >&2
  exit 1
}

is_version() {
  [[ "$1" =~ ^[0-9]+\.[0-9]+\.[0-9]+$ ]]
}

# The highest of a list of versions, one per line; empty for an empty list.
highest() {
  printf '%s\n' "$1" | grep -E '^[0-9]+\.[0-9]+\.[0-9]+$' | sort -V | tail -1 || true
}

# is_newer <a> <b>: whether a is a later version than b.
is_newer() {
  [ "$1" != "$2" ] && [ "$(printf '%s\n%s\n' "$1" "$2" | sort -V | tail -1)" = "$1" ]
}

next_patch() {
  local major minor patch
  IFS=. read -r major minor patch <<<"$1"
  echo "$major.$minor.$((patch + 1))"
}

in_list() { # <version> <list, one per line>
  printf '%s\n' "$2" | grep -Fxq -- "$1"
}

# plan_next <cargo version> <released versions> <tagged versions>
# Sets VERSION and WHY, or stops: this path replaces nothing.
plan_next() {
  local cargo="$1" releases="$2" tags="$3" last
  last="$(highest "$releases")"
  if [ -z "$last" ]; then
    VERSION="$cargo"
    WHY="no release yet: app/Cargo.toml's version"
  elif is_newer "$cargo" "$last"; then
    VERSION="$cargo"
    WHY="app/Cargo.toml is ahead of the last release, $last"
  else
    VERSION="$(next_patch "$last")"
    WHY="the patch after the last release, $last"
  fi
  if in_list "$VERSION" "$tags"; then
    die "tag v$VERSION already exists, with no release of it (an earlier run may have failed). 'just publish $VERSION' replaces it."
  fi
}

# plan_exact <version> <released versions> <tagged versions>
# Sets VERSION, WHY, HAS_RELEASE and HAS_TAG.
plan_exact() {
  local version="$1" releases="$2" tags="$3"
  is_version "$version" || die "version must be MAJOR.MINOR.PATCH, not '$version'"
  VERSION="$version"
  WHY="asked for by name"
  HAS_RELEASE=no
  HAS_TAG=no
  if in_list "$version" "$releases"; then HAS_RELEASE=yes; fi
  if in_list "$version" "$tags"; then HAS_TAG=yes; fi
}

# The [package] version of a Cargo.toml.
cargo_version() {
  awk -F'"' '/^\[/{s=$0} s=="[package]" && /^version *=/{print $2; exit}' "$1"
}

set_cargo_version() { # <Cargo.toml> <version>
  local tmp
  tmp="$(mktemp)"
  awk -v v="$2" '
    /^\[/ { s = $0 }
    s == "[package]" && !done && /^version *=/ { print "version = \"" v "\""; done = 1; next }
    { print }' "$1" > "$tmp"
  cat "$tmp" > "$1"
  rm -f "$tmp"
}

set_lock_version() { # <Cargo.lock> <package> <version>
  local tmp
  tmp="$(mktemp)"
  awk -v name="name = \"$2\"" -v v="$3" '
    $0 == name { hit = 1; print; next }
    hit && /^version = / { print "version = \"" v "\""; hit = 0; next }
    { print }' "$1" > "$tmp"
  cat "$tmp" > "$1"
  rm -f "$tmp"
}

# Every published or pre-release version on GitHub. A failure stops the run:
# it must never read as "no release yet".
released_versions() {
  local tags
  tags="$("$GH" api --paginate "repos/$REPO/releases" --jq '.[] | select(.draft | not) | .tag_name')" \
    || die "could not list the releases of $REPO (is gh signed in?)"
  printf '%s\n' "$tags" | sed -n 's/^v//p'
}

tagged_versions() {
  local refs
  refs="$(git ls-remote --tags --refs origin 'v*')" || die "could not read origin's tags"
  printf '%s\n' "$refs" | sed -n 's|.*refs/tags/v||p'
}

# The names of the repo's secrets; never their values.
secret_names() {
  "$GH" secret list -R "$REPO" --json name --jq '.[].name' || die "could not list the secrets of $REPO"
}

has_all() { # <names, one per line> <wanted, space-separated>: yes or no
  local name
  for name in $2; do
    in_list "$name" "$1" || {
      echo no
      return 0
    }
  done
  echo yes
}

# What stops a real run, one line each; a dry run only reports it.
checkout_problems() {
  local branch
  branch="$(git branch --show-current)"
  [ "$branch" = main ] || echo "not on main (on ${branch:-a detached HEAD})"
  [ -z "$(git status --porcelain --untracked-files=no)" ] || echo "the working tree has uncommitted changes"
  git fetch --quiet origin main || { echo "could not fetch origin main"; return 0; }
  [ "$(git rev-parse HEAD)" = "$(git rev-parse FETCH_HEAD)" ] || echo "HEAD is not origin/main"
}

# Says what the override is about to replace.
describe_replaced() {
  local version="$1" tag="v$1" commit
  echo
  echo "This REPLACES what is published as $version:"
  if [ "$HAS_RELEASE" = yes ]; then
    "$GH" release view "$tag" -R "$REPO" --json name,publishedAt,isPrerelease,assets \
      --template '  GitHub release: {{.name}}, published {{.publishedAt}}{{if .isPrerelease}} (pre-release){{end}}{{"\n"}}{{range .assets}}    {{.name}}{{"\n"}}{{end}}' \
      || die "could not read release $tag"
  fi
  if [ "$HAS_TAG" = yes ]; then
    commit="$(git ls-remote --tags origin "refs/tags/$tag^{}" "refs/tags/$tag" | tail -1 | cut -f1)"
    echo "  tag $tag: $commit -> $(git rev-parse HEAD)"
  fi
  echo "  $R2_PLAN"
  echo "The tag moves now. The rest happens at the end of the run, once the new build has passed; the GitHub release is updated in place."
}

confirm() {
  if ! { : < /dev/tty; } 2>/dev/null; then
    [ "${PUBLISH_YES:-}" = 1 ] && return 0
    die "no terminal to ask on: set PUBLISH_YES=1 to go ahead"
  fi
  local answer
  printf 'Replace it? [y/N] ' > /dev/tty
  read -r answer < /dev/tty
  case "$answer" in
    y | Y | yes) ;;
    *) die "stopped; nothing changed" ;;
  esac
}

main() {
  local mode="${1:-}" exact="" dry=no
  shift || true
  case "$mode" in
    next) ;;
    exact)
      exact="${1:-}"
      shift || true
      [ -n "$exact" ] || die "exact needs a version"
      ;;
    *) die "usage: publish.sh next|exact X.Y.Z [--dry-run]" ;;
  esac
  case "${1:-}" in
    "") ;;
    --dry-run) dry=yes ;;
    *) die "unknown argument '$1'" ;;
  esac

  cd "$ROOT"
  local cargo broker releases tags
  cargo="$(cargo_version app/Cargo.toml)"
  broker="$(cargo_version broker/buwiz-broker/Cargo.toml)"
  is_version "$cargo" || die "app/Cargo.toml has no MAJOR.MINOR.PATCH version"
  releases="$(released_versions)"
  tags="$(tagged_versions)"

  HAS_RELEASE=no
  HAS_TAG=no
  if [ "$mode" = next ]; then
    plan_next "$cargo" "$releases" "$tags"
  else
    plan_exact "$exact" "$releases" "$tags"
  fi

  local tag="v$VERSION" override=no bump=no problems
  if [ "$HAS_RELEASE" = yes ] || [ "$HAS_TAG" = yes ]; then override=yes; fi
  if [ "$cargo" != "$VERSION" ] || [ "$broker" != "$VERSION" ]; then bump=yes; fi

  echo "version:  $VERSION ($WHY)"
  echo "tag:      $tag"
  echo "override: $override (release: $HAS_RELEASE, tag: $HAS_TAG)"
  if [ "$bump" = yes ]; then
    echo "cargo:    app $cargo, broker $broker -> $VERSION, committed to main"
    if is_newer "$cargo" "$VERSION"; then
      echo "          note: this lowers main's version"
    fi
  else
    echo "cargo:    already $VERSION"
  fi

  problems="$(checkout_problems)"

  # What R2 will really get; an exact run replaces, so it can leave R2 behind.
  local keep_r2=false
  R2_PLAN="R2: bot/releases/$tag/ replaced, and bot/latest.json if $VERSION is the newest"
  if [ "$mode" = exact ]; then
    local names signing r2 keep=no refusal
    names="$(secret_names)"
    signing="$(has_all "$names" "$SIGNING_SECRETS")"
    r2="$(has_all "$names" "$R2_SECRETS")"
    if [ "${PUBLISH_KEEP_R2:-}" = 1 ]; then
      keep=yes
      keep_r2=true
    fi
    if [ "$signing" != yes ] || [ "$r2" != yes ]; then
      R2_PLAN="R2: NOT replaced (signing secrets: $signing, R2 secrets: $r2); whatever it holds for $VERSION stays live and won't match the tag"
      echo "$R2_PLAN"
    fi
    refusal="$("$ROOT/scripts/release/override-check.sh" "$VERSION" "$signing" "$r2" "$keep" 2>&1)" \
      || problems="${problems:+$problems$'\n'}$refusal"
  fi

  if [ "$dry" = yes ]; then
    if [ "$override" = yes ]; then describe_replaced "$VERSION"; fi
    if [ -n "$problems" ]; then
      echo
      echo "A real run would stop here:"
      printf '%s\n' "$problems" | sed 's/^/  /'
    fi
    echo
    echo "dry run: nothing changed"
    return 0
  fi
  [ -z "$problems" ] || die "$(printf 'not publishing:\n%s' "$problems")"

  if [ "$override" = yes ]; then
    describe_replaced "$VERSION"
    confirm
  fi

  if [ "$bump" = yes ]; then
    set_cargo_version app/Cargo.toml "$VERSION"
    set_cargo_version broker/buwiz-broker/Cargo.toml "$VERSION"
    set_lock_version app/Cargo.lock buwiz-bot "$VERSION"
    set_lock_version broker/Cargo.lock buwiz-broker "$VERSION"
    git add app/Cargo.toml app/Cargo.lock broker/buwiz-broker/Cargo.toml broker/Cargo.lock
    git commit --quiet -m "Release $tag"
    git push --quiet origin HEAD:main
  fi

  # Straight to origin, with no local tag: this clone also holds nsh upstream's
  # v* tags, and a failed push then leaves nothing behind to trip over.
  if [ "$override" = yes ]; then
    git push --quiet --force origin "HEAD:refs/tags/$tag"
  else
    git push --quiet origin "HEAD:refs/tags/$tag"
  fi

  local replace=false
  if [ "$mode" = exact ]; then replace=true; fi
  "$GH" workflow run "$WORKFLOW" -R "$REPO" --ref "$tag" -f version="$VERSION" -f replace="$replace" -f keep_r2="$keep_r2" \
    || die "$tag is pushed but the release did not start; 'just publish $VERSION' tries again"
  echo
  echo "started the release of $tag: https://github.com/$REPO/actions/workflows/$WORKFLOW"
}

# test-publish.sh sources this file for the functions above.
if [ "${BASH_SOURCE[0]}" = "$0" ]; then
  main "$@"
fi
