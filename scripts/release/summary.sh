#!/usr/bin/env bash
# The run's summary: what really happened, from the steps' outcomes.
# Environment: VERSION, SIGNED, REPLACE, R2_READY, and R2_OUTCOME and
# GH_OUTCOME (success, failure or skipped).
set -uo pipefail

echo "### Buwiz Bot $VERSION"
echo
if [ "$SIGNED" = true ]; then
  echo "- Signed and notarized."
else
  echo "- **Not signed.** Add the signing secrets and run \`just publish $VERSION\`."
fi

case "$R2_OUTCOME" in
  success) echo "- On R2 under \`bot/releases/v$VERSION/\`." ;;
  failure) echo "- **R2 upload failed.** R2 may hold part of this build; \`just publish $VERSION\` tries again." ;;
  *)
    if [ "$REPLACE" = true ]; then
      echo "- **R2 was not written.** Anything it held for $VERSION, and \`bot/latest.json\`, stay live and don't match the tag."
    else
      echo "- **Not on R2**, so buwiz.com doesn't offer it."
    fi
    ;;
esac

case "$GH_OUTCOME" in
  success)
    if [ "$SIGNED" = true ]; then
      echo "- GitHub release v$VERSION is up."
    else
      echo "- GitHub release v$VERSION is up as a pre-release."
    fi
    ;;
  failure)
    if [ "$REPLACE" = true ]; then
      echo "- **The GitHub release failed.** The existing release stays up, and may hold a mix of old and new files; \`just publish $VERSION\` tries again."
    else
      echo "- **The GitHub release failed**, so there is none. \`just publish $VERSION\` tries again."
    fi
    ;;
  *) echo "- **GitHub release not touched**, because an earlier step failed." ;;
esac

if [ -f out/SHA256SUMS ]; then
  echo
  echo '```'
  cat out/SHA256SUMS
  echo '```'
fi
