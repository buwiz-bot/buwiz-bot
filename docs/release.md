# Releasing Buwiz Bot

A release is one Apple silicon (arm64) dmg, signed with the Goldcoders
Corp Developer ID, notarized and stapled, plus its checksum sidecar and
`latest.json`. They are served from `https://downloads.buwiz.com/bot/`
(the R2 bucket `buwiz-releases`, shared with the filing and books apps;
the bot's files are under `bot/`):

```
bot/latest.json
bot/<version>/Buwiz-Bot-<version>-macos-arm64.dmg
bot/<version>/Buwiz-Bot-<version>-macos-arm64.dmg.sha256
```

Nothing here uploads: the scripts and CI stop at files on disk (or
workflow artifacts), and `manifest.sh` prints the upload commands.

## The steps

All in `scripts/release/`; each later step reads names, ids and the
version from the bundle's `Info.plist`, so they follow `build.sh`.

| Script | Does |
| --- | --- |
| `build.sh` | Release builds of the app, the broker workspace (`rox-broker`, `rox-gws`) and nsh (`nsh`, `nsh-macos-host`), through `scripts/wait-build`, with `RUSTC_WRAPPER` empty; assembles `dist/Buwiz Bot.app` (the layout of `app/bundle-dev.sh`, without its launcher, gpui-agent token or env exports). |
| `sign.sh [identity]` | Developer ID signing with the hardened runtime, inside out: every Mach-O, then nested bundles, then the app. `-` signs ad hoc (a dry run). |
| `notarize.sh <app or dmg>` | `notarytool submit --wait`, then `stapler staple` and `stapler validate`. Skips, saying so, without the `NOTARY_*` env (`REQUIRE_NOTARIZATION=1` makes that an error). |
| `dmg.sh [identity]` | A drag-to-Applications dmg (hdiutil, no new tools), signed, notarized and stapled; writes `<dmg>.sha256` and prints the sha256 and size. |
| `manifest.sh [dmg]` | `dist/latest.json`, and the upload commands as comments. |

### Info.plist

The name, bundle id, executable name and URL schemes come from the
Info.plist `app/bundle-dev.sh` writes (`Buwiz Bot`, `com.buwiz.bot`,
`buwiz-bot`, `buwiz`), so the release and dev bundles agree. In the
release bundle `MacOS/buwiz-bot` is the app itself (the cargo `rox`
binary), not the dev launcher. On top of those:

- `CFBundleShortVersionString`: `app/Cargo.toml`'s `[package] version`.
  The tag must be `v` plus that version (CI checks). `CFBundleVersion` is
  the same, or `BUILD_NUMBER` (CI passes the run number).
- `LSMinimumSystemVersion`: 13.0 (`MIN_MACOS`), as the filing app, until
  older versions are tested. `build.sh` refuses a value below any bundled
  binary's `minos` (11.0 today, rustc's default for arm64): the oldest
  macOS the binaries load on at all.
- `CFBundleURLTypes`: the `buwiz` scheme only.
- `NSHumanReadableCopyright`: "Copyright © Goldcoders Corp".
- `NSAppleEventsUsageDescription`: the broker drives Reminders and Finder
  through `osascript`; macOS shows this text when it asks.
- `CFBundleIconFile`: only with an icon (below); `build.sh` warns loudly
  without one.

### The icon

Put one of these in `app/resources/` (or point `APP_ICON` at it):

- `AppIcon.icns`, used as is; or
- `AppIcon.png`, 1024x1024, from which `build.sh` makes the `.icns`
  (`sips` for the 16 to 1024 px sizes, then `iconutil`).

### Entitlements

`scripts/release/app.entitlements` goes on the app's executable and on
`rox-broker`; `nsh`, `nsh-macos-host` and `rox-gws` get none.

| Entitlement | Why |
| --- | --- |
| `com.apple.security.automation.apple-events` | The broker runs `osascript` (Reminders, Finder's desktop bounds, the save-as picker). The hardened runtime refuses Apple Events without it. |

Left out on purpose, after inspecting the bundle:

- `cs.allow-jit` and `cs.allow-unsigned-executable-memory`: nothing in
  the bundle JITs. nsh's QuickJS (`quickjs-jit`) is the interpreter (no
  `MAP_JIT` or executable mappings in it); WKWebView's JIT runs in
  WebKit's own processes, which carry Apple's entitlements; the bundled
  broker runs with `--no-node`, so no Node is spawned. ra-jit-live (live
  patching) is linked in but only started in dev builds
  (`app/src/live.rs`), so a release reserves no patch area and writes no
  `~/.cache/ra-jit`.
- `cs.disable-library-validation`: only system libraries and frameworks
  are loaded (`dlopen` of AVFoundation and AVKit included).
- No App Sandbox: the app runs local tools and reads the user's files.

## Locally

```sh
export CARGO_TARGET_DIR=...   # optional; and BROKER_TARGET_DIR, NSH_TARGET_DIR
(cd server && npm ci && npm run build:office-editor)
scripts/release/build.sh

# a dry run, signed ad hoc
scripts/release/sign.sh -
scripts/release/dmg.sh -
scripts/release/manifest.sh

# a real release
export DEVELOPER_ID_APPLICATION="Developer ID Application: Goldcoders Corp (5KZ8MD34QW)"  # confirm with owner
export NOTARY_KEY_ID=... NOTARY_ISSUER_ID=... NOTARY_KEY_P8="$(base64 -i AuthKey_XXXX.p8)"
scripts/release/sign.sh
scripts/release/notarize.sh "dist/Buwiz Bot.app"
scripts/release/dmg.sh
scripts/release/manifest.sh
```

`build.sh` takes prebuilt binaries (`ROX_BIN`, `ROX_BROKER_BIN`,
`ROX_GWS_BIN`, `NSH_BIN` with `NSH_HOST_BIN`) and other input paths
(`NODE_MODULES`, `OFFICE_DIST`, `OFFICE_FONTS`, `MATERIAL_SYMBOLS_TTF`,
`ROX_GWS_MANIFEST`); its header lists them. Output goes to `dist/`
(`RELEASE_OUT`), which git ignores.

## CI

`.github/workflows/release.yml` runs on a `v*` tag and on
`workflow_dispatch`, on `macos-14` (Apple silicon). It checks the tag
against the version, builds the office editor and the bundle, imports the
certificate into a temporary keychain, signs, notarizes and staples the
app, makes, signs, notarizes and staples the dmg, runs the Gatekeeper
checks, writes `latest.json`, and keeps the dmg, its `.sha256` and
`latest.json` as a workflow artifact. A tag without every signing secret
fails; a manual run without them is an ad-hoc dry run, its artifact named
`-unsigned`.

There is no upload step yet. To publish, download the artifact and run
the commands `manifest.sh` prints (dmg and sidecar first, `latest.json`
last).

`.github/workflows/ci.yml` runs, on every PR, `cargo test -p rox -p
buwiz-office-kit -p buwiz-word -p buwiz-spreadsheet` (the app workspace;
the office crates take their fonts from `server/`'s `npm ci`) and the
broker workspace's `cargo test` (whose diff tests run `server/`'s Node
code).

GitHub Actions has been blocked on billing on these accounts before; a red
or never-started run may be that and not the code. The scripts run the
same way on a Mac.

### Secrets

Repository secrets (Settings › Secrets and variables › Actions):

| Secret | Value |
| --- | --- |
| `MACOS_CERT_P12` | The Developer ID Application certificate and its private key, exported as `.p12`, base64-encoded: `base64 -i cert.p12` |
| `MACOS_CERT_PASSWORD` | The `.p12`'s export password |
| `DEVELOPER_ID_APPLICATION` | Optional. The identity's full name, `Developer ID Application: Goldcoders Corp (5KZ8MD34QW)` (the filing app's team; confirm with owner); the run fails if the `.p12` holds another |
| `NOTARY_KEY_ID` | The App Store Connect API key's Key ID |
| `NOTARY_ISSUER_ID` | The API keys page's Issuer ID |
| `NOTARY_KEY_P8` | The key's `AuthKey_<KeyID>.p8`, base64-encoded: `base64 -i AuthKey_XXXX.p8` |
| `GWS_MANIFEST_JSON` | Optional. The signed `rox-gws` manifest (`scripts/gws-install-wizard`'s `broker/rox-gws/manifest.signed.json`); without it the release can't provision Google Workspace |

Getting them:

- **Certificate.** developer.apple.com › Certificates › + › *Developer ID
  Application* (only the Account Holder can create one). Make the CSR in
  Keychain Access, install the certificate, then in *My Certificates*
  export it together with its private key as `.p12` with a password.
- **API key.** App Store Connect › Users and Access › Integrations › App
  Store Connect API › Team Keys › +, access *Developer*. Download the
  `.p8` (once only); note its Key ID and the Issuer ID above the list.

## Known limitation: spaces in plugin command paths

The app installs as `/Applications/Buwiz Bot.app`, so its bundled tools
(`Contents/Resources/broker/bin/rox-gws`, `nsh`, ...) have a space in
their paths. Connect Apps splits a plugin's command at spaces, so a
plugin command naming one of them breaks until that is fixed (a fix is
coming separately). Check Google Workspace provisioning on an installed
release before announcing it.

## Checking a release

On a Mac other than the build machine, after downloading the dmg:

```sh
shasum -a 256 -c Buwiz-Bot-<version>-macos-arm64.dmg.sha256
xcrun stapler validate Buwiz-Bot-<version>-macos-arm64.dmg
spctl -a -vvv -t open --context context:primary-signature Buwiz-Bot-<version>-macos-arm64.dmg
hdiutil attach Buwiz-Bot-<version>-macos-arm64.dmg
codesign --verify --deep --strict --verbose=2 "/Volumes/Buwiz Bot/Buwiz Bot.app"
codesign -dv --verbose=4 "/Volumes/Buwiz Bot/Buwiz Bot.app"   # Authority=Developer ID Application: …, flags=0x10000(runtime), TeamIdentifier
spctl -a -vvv -t exec "/Volumes/Buwiz Bot/Buwiz Bot.app"      # accepted, source=Notarized Developer ID
xcrun stapler validate "/Volumes/Buwiz Bot/Buwiz Bot.app"
```

## latest.json

The filing app's shape (its `scripts/publish_update_manifest.py`, read by
its updater): a version and one entry per platform, whose `name` is the
file name the checksum sidecar's line is matched by. The updater takes a
download only when its hash, `sha256` and the sidecar all agree.

```json
{
  "version": "0.1.0",
  "platforms": {
    "darwin-arm64": {
      "url": "https://downloads.buwiz.com/bot/0.1.0/Buwiz-Bot-0.1.0-macos-arm64.dmg",
      "sha256": "<hex>",
      "name": "Buwiz-Bot-0.1.0-macos-arm64.dmg",
      "checksum_url": "https://downloads.buwiz.com/bot/0.1.0/Buwiz-Bot-0.1.0-macos-arm64.dmg.sha256"
    }
  }
}
```

The filing app keys its dmg `darwin-universal`; the bot's is arm64 only,
hence `darwin-arm64`.
