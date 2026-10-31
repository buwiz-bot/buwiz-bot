# shellcheck shell=bash
# Shared by scripts/release/*.sh (sourced, not run). Everything about the
# bundle the later steps need comes from its Info.plist, so a rename of the
# executable or the bundle id is made in build.sh alone.

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
OUT="${RELEASE_OUT:-$ROOT/dist}"

die() {
  echo "error: $*" >&2
  exit 1
}

# plist_get <Info.plist> <key>: a top-level string value, empty when absent.
plist_get() {
  plutil -extract "$2" raw -o - "$1" 2>/dev/null || true
}

# The bundle the steps after build.sh work on: $1, else the one .app in
# $OUT (build.sh makes exactly one).
find_app() {
  if [ -n "${1:-}" ]; then
    [ -d "$1/Contents" ] || die "$1 is not an app bundle"
    (cd "$1" && pwd)
    return
  fi
  local apps=("$OUT"/*.app)
  [ -d "${apps[0]}" ] || die "no .app in $OUT: run scripts/release/build.sh first, or pass the bundle"
  [ "${#apps[@]}" -eq 1 ] || die "more than one .app in $OUT; pass the one to use"
  echo "${apps[0]}"
}

# The dmg's file name, from the bundle: <name with dashes>-<version>-macos-arm64.dmg.
# "macos" in the name is what the filing app's manifest tooling keys a dmg by.
dmg_name() {
  local plist="$1/Contents/Info.plist" name version
  name="$(plist_get "$plist" CFBundleName)"
  version="$(plist_get "$plist" CFBundleShortVersionString)"
  [ -n "$name" ] && [ -n "$version" ] || die "$plist has no CFBundleName or CFBundleShortVersionString"
  echo "${name// /-}-$version-macos-arm64.dmg"
}

# is_macho <file>: true for a Mach-O (thin or universal) file.
is_macho() {
  case "$(file -b "$1")" in
    Mach-O*) return 0 ;;
    *) return 1 ;;
  esac
}
