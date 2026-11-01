# Google Workspace (buwiz-gws)

Buwiz Bot ships the Google Workspace plugin, `app.buwiz.google-workspace`,
in `Contents/Resources/broker/bin/`:
- `buwiz-gws`, the binary;
- `buwiz-gws.manifest.json`, its signed manifest.

Its code lives in [buwiz-app/google-workspace](https://github.com/buwiz-app/google-workspace).

| File | What it is |
|---|---|
| `rev` | The commit of that repo the Bot builds. `scripts/gws-build` builds it, and `app/bundle-dev.sh`, `scripts/release/build.sh` and `scripts/gws-install-wizard` call that script |
| `manifest.signed.json` | The owner's signed manifest, written by `scripts/gws-install-wizard`. It is not in git; a release takes it from the `GWS_MANIFEST_JSON` secret |

To move the Bot to a newer plugin:
1. Put the new commit in `rev`.
2. Sign the manifest again if it changed (`scripts/gws-install-wizard`, step 2).
3. Check that the Node plugin still matches: run `server/test/v1-gws.test.mjs` with `GWS_CMD` set to the binary `scripts/gws-build` prints. GWS-06 compares the two byte for byte.
