# Releases. docs/release.md has the whole story and the secrets to add first.
# Nothing else starts a release build: not a push, a PR or a merge.

# Lists the recipes.
default:
    @just --list

# Publish: `just publish` is the next version, `just publish 0.1.0` is that one, replacing it if it exists.
publish version="":
    #!/usr/bin/env bash
    set -euo pipefail
    if [ -z {{ quote(version) }} ]; then
        # No version given: Cargo.toml's for the first release, then the next
        # patch. Never replaces anything.
        exec scripts/release/publish.sh next
    else
        # An exact version: replaces its tag and release if they exist, after
        # saying what it is replacing and asking.
        exec scripts/release/publish.sh exact {{ quote(version) }}
    fi

# Prints the version, the tag and whether `just publish` would override; changes nothing.
publish-dry-run version="":
    #!/usr/bin/env bash
    set -euo pipefail
    if [ -z {{ quote(version) }} ]; then
        exec scripts/release/publish.sh next --dry-run
    else
        exec scripts/release/publish.sh exact {{ quote(version) }} --dry-run
    fi

# Tests the publish, manifest and R2 scripts, offline, against stand-ins.
publish-test:
    scripts/release/test-publish.sh
