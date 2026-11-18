# Sourced first thing by the scripts that once read a variable under an older
# name: with one of those still set, exit 2 and say which, as the app and the
# broker do. Any variable starting with ROX_, or exactly GROK_DATA_DIR,
# GROK_ALLOWED_ORIGINS or GROK_BUNDLE_ID, is one.
__old_names="$(awk 'BEGIN { for (k in ENVIRON) if (k ~ /^ROX_/ || k == "GROK_DATA_DIR" || k == "GROK_ALLOWED_ORIGINS" || k == "GROK_BUNDLE_ID") print k }' </dev/null | sort | paste -sd, - | sed 's/,/, /g')"
if [ -n "$__old_names" ]; then
  echo "$(basename "$0"): these variables are set and no longer read: $__old_names; use the BUWIZ_ name instead and unset them" >&2
  exit 2
fi
unset __old_names
