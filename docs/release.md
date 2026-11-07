# Releasing Buwiz Bot

A release is one Apple silicon (arm64) dmg, signed with the Goldcoders
Corp Developer ID, notarized and stapled, with its `manifest.json` and
`SHA256SUMS`. It goes to a GitHub release and to the R2 bucket
`buwiz-releases`, served at `https://downloads.buwiz.com/bot/` (the bucket is
shared with the other apps; the bot's files are under `bot/`):

```
bot/latest.json                              what buwiz.com reads
bot/releases/v<version>/Buwiz-Bot-<version>-macos-arm64.dmg
bot/releases/v<version>/manifest.json
bot/releases/v<version>/SHA256SUMS
```

## Publishing

Add the secrets below, then, on a clean, up-to-date `main`:

```sh
just publish            # the next version
just publish 0.1.0      # that version, replacing it if it exists
just publish-dry-run    # prints what either would do, changes nothing
just publish-dry-run 0.1.0
```

`just publish` is the only thing that starts a release build; no push, PR
or merge does. (`Release` can also be run from the Actions tab, but it
refuses anything but a version's own tag.)

- **`just publish`** takes `app/Cargo.toml`'s version if nothing is released
  yet. After that it takes the last release's patch + 1, or
  `app/Cargo.toml`'s version if that is already ahead. It never replaces
  anything: if the tag exists, it stops and says so.
- **`just publish 0.1.0`** takes that version. If a tag or release of it
  exists, it lists what will be replaced and asks. With no terminal to ask
  on, `PUBLISH_YES=1` goes ahead; on a terminal it always asks. The tag moves
  to `main`'s head at once. R2 and the GitHub release change at the end of
  the run, so a failed build leaves the old ones up.

An override needs the signing and R2 secrets to replace what is on R2. If
either is missing and R2 holds that version, or that can't be told, it stops
before the tag moves. `PUBLISH_KEEP_R2=1 just publish 0.1.0` goes ahead
anyway: the public download then stays on the old build and doesn't match the
tag. The command reads repository secrets by name, and the workflow makes the
same check, so an organization-level secret is seen only by the workflow.

The last job writes R2 first, then GitHub. Replacing updates the GitHub
release in place (files over the old ones, then leftovers removed, then the
title and notes); it is never deleted, so a failure leaves a release up,
possibly with a mix of old and new files. If R2 fails, GitHub isn't touched.
If GitHub fails, R2 is ahead of it. The run's summary says which happened,
and `just publish <version>` tries again.

A failed read is never taken for "nothing there": the workflow stops if it
can't read the GitHub release, the R2 listing or `bot/latest.json`, and
only a real not-found counts as missing. A build with no signed status counts
as unsigned.

The tag is pushed straight to origin and no local tag is made: this repo's
clones also hold nsh upstream's `v*` tags, and a failed push leaves nothing
to retry around. A local tag of the same name is ignored.

When the version differs from the Cargo.toml files, the command sets it in
`app/Cargo.toml`, `broker/buwiz-broker/Cargo.toml` and their lockfiles, and
commits `Release v<version>` to `main` before tagging. The app and the broker
report that version; the workflow stops if either disagrees with the tag.

A replaced version keeps its URLs, so R2 files are cached for 5 minutes
rather than for good. `bot/latest.json` only moves forward: replacing an
older version leaves it at the newest.

`just publish-test` runs the scripts' tests, offline.

## Targets

| Target | State |
| --- | --- |
| macOS arm64 | Built and published |
| Linux x64 | Off: the app doesn't run there |
| Windows x64 | Off: the app doesn't run there |

Both are commented-out entries in `release.yml`'s matrix. nsh has Linux
(GTK 4) and Windows (Win32) backends, but the Bot is not on them:

- `app/src/main.rs`'s `main` only exits outside macOS and iOS, and
  `app/Cargo.toml` depends on neither backend.
- `crates/office-kit`'s fonts are CoreText. It doesn't compile on Linux,
  which stops the app there.
- `broker/buwiz-protocol` and `broker/buwiz-mcp` use Unix-only calls, which
  stops the app and the broker on Windows. The broker compiles for Linux.
- The Agent Hub, native mini-app screens, keyboard handling, zoom and the
  Keychain vault are macOS-only.
- The packaging here (`build.sh`, `sign.sh`, `dmg.sh`) is macOS-only.

## The steps

All in `scripts/release/`; each step after `build.sh` reads names, ids and
the version from the bundle's `Info.plist`, so they follow it.

| Script | Does |
| --- | --- |
| `build.sh` | Release builds of the app, the broker workspace (`buwiz-broker`) and nsh (`nsh`, `nsh-macos-host`), through `scripts/wait-build`, with `RUSTC_WRAPPER` empty; assembles `dist/Buwiz Bot.app` (the layout of `app/bundle-dev.sh`, without its launcher, gpui-agent token or env exports, and without the gpui-agent server: it builds with `--no-default-features` and refuses a binary that has it). |
| `sign.sh [identity]` | Developer ID signing with the hardened runtime, inside out: every Mach-O, then nested bundles, then the app. `-` signs ad hoc (a dry run). |
| `notarize.sh <app or dmg>` | `notarytool submit --wait`, then `stapler staple` and `stapler validate`. Skips, saying so, without the `NOTARY_*` env (`REQUIRE_NOTARIZATION=1` makes that an error). |
| `dmg.sh [identity]` | A drag-to-Applications dmg (hdiutil, no new tools), signed, notarized and stapled; writes `<dmg>.sha256` and prints the sha256 and size. |
| `manifest.py <files-dir> <version> <out-dir>` | `manifest.json` and `SHA256SUMS` for the built files. |
| `r2-publish.sh <files-dir> <out-dir>` | Uploads the files, then the manifest, then `bot/latest.json`. |
| `publish.sh` | Behind `just publish`: picks the version, tags it and starts the workflow. |
| `override-check.sh` | Stops an override that can't replace what is on R2. |
| `release-exists.sh <tag>` | Whether GitHub has that release; fails if it can't tell. |
| `signed-state.sh <files-dir> <status-dir>` | Whether every build recorded that it was signed. |
| `gh-publish.sh <files-dir> <out-dir>` | Creates the GitHub release, or updates it in place. |
| `summary.sh` | The run's summary, from what each step really did. |

### Info.plist

The name, bundle id, executable name and URL schemes come from the
Info.plist `app/bundle-dev.sh` writes (`Buwiz Bot`, `com.buwiz.bot`,
`buwiz-bot`, `buwiz`), so the release and dev bundles agree. In the
release bundle `MacOS/buwiz-bot` is the app itself (the cargo `buwiz-bot`
binary), not the dev launcher. On top of those:

- `CFBundleShortVersionString`: `app/Cargo.toml`'s `[package] version`.
  The tag is `v` plus that version (CI checks). `CFBundleVersion` is
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
`buwiz-broker`; `nsh` and `nsh-macos-host` get none.

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

# a real release
export DEVELOPER_ID_APPLICATION="Developer ID Application: Goldcoders Corp (5KZ8MD34QW)"  # confirm with owner
export NOTARY_KEY_ID=... NOTARY_ISSUER_ID=... NOTARY_KEY_P8="$(base64 -i AuthKey_XXXX.p8)"
scripts/release/sign.sh
scripts/release/notarize.sh "dist/Buwiz Bot.app"
scripts/release/dmg.sh
```

`build.sh` takes prebuilt binaries (`BUWIZ_BOT_BIN`, `BUWIZ_BROKER_BIN`,
`NSH_BIN` with `NSH_HOST_BIN`) and other input paths
(`NODE_MODULES`, `OFFICE_DIST`, `OFFICE_FONTS`, `MATERIAL_SYMBOLS_TTF`); its
header lists them. Output goes to `dist/`
(`RELEASE_OUT`), which git ignores.

## CI

`.github/workflows/release.yml` runs only by `workflow_dispatch`, from the
version's tag, which is what `just publish` does. On `macos-15` (Apple
silicon) it builds the office editor and the bundle, imports the
certificate into a temporary keychain, signs, notarizes and staples the app
and the dmg, and runs the Gatekeeper checks. One last job writes the
manifest, uploads to R2 and creates the GitHub release, so a failed build
publishes nothing.

`.github/workflows/ci.yml` runs, on every PR, `cargo test -p buwiz-bot -p
buwiz-office-kit -p buwiz-word -p buwiz-spreadsheet` (the app workspace;
the office crates take their fonts from `server/`'s `npm ci`) and the
broker workspace's `cargo test` (whose diff tests run `server/`'s Node
code).

GitHub Actions has been blocked on billing on these accounts before; a red
or never-started run may be that and not the code. The scripts run the
same way on a Mac.

### Secrets

Repository secrets (Settings › Secrets and variables › Actions). None is
needed to run the workflow; each group left out changes what the release is:

| Left out | What happens |
| --- | --- |
| Any signing secret | An ad-hoc signed dmg, published as a GitHub pre-release and not to R2: macOS blocks it, so buwiz.com shouldn't offer it |
| Any R2 secret | A signed GitHub release, not on R2, so buwiz.com keeps saying there is no release |

The run warns, naming what is missing. Add them and `just publish <version>`
replaces it. An override with a group left out is different: it stops if R2
holds that version (see Publishing).

Signing:

| Secret | Value |
| --- | --- |
| `MACOS_CERT_P12` | The Developer ID Application certificate and its private key, exported as `.p12`, base64-encoded: `base64 -i cert.p12` |
| `MACOS_CERT_PASSWORD` | The `.p12`'s export password |
| `DEVELOPER_ID_APPLICATION` | Optional. The identity's full name, `Developer ID Application: Goldcoders Corp (5KZ8MD34QW)` (the filing app's team; confirm with owner); the run fails if the `.p12` holds another |
| `NOTARY_KEY_ID` | The App Store Connect API key's Key ID |
| `NOTARY_ISSUER_ID` | The API keys page's Issuer ID |
| `NOTARY_KEY_P8` | The key's `AuthKey_<KeyID>.p8`, base64-encoded: `base64 -i AuthKey_XXXX.p8` |

R2:

| Secret | Value |
| --- | --- |
| `R2_ENDPOINT` | `https://<account id>.r2.cloudflarestorage.com`: the S3 API address on the bucket's settings page in Cloudflare, the same one eBIRForms' workflows use |
| `R2_ACCESS_KEY_ID` | An R2 API token's Access Key ID |
| `R2_SECRET_ACCESS_KEY` | That token's Secret Access Key |

Getting them:

- **Certificate.** developer.apple.com › Certificates › + › *Developer ID
  Application* (only the Account Holder can create one). Make the CSR in
  Keychain Access, install the certificate, then in *My Certificates*
  export it together with its private key as `.p12` with a password.
- **API key.** App Store Connect › Users and Access › Integrations › App
  Store Connect API › Team Keys › +, access *Developer*. Download the
  `.p8` (once only); note its Key ID and the Issuer ID above the list.
- **R2 token.** Cloudflare › R2 › Manage API Tokens › Create, *Object Read &
  Write*, limited to the bucket `buwiz-releases`. The secret is shown once.
  eBIRForms' `R2_*` secrets are in its own organization; a token can be
  shared but has to be added here too.

## Spaces in plugin command paths

The app installs as `/Applications/Buwiz Bot.app`, so its bundled tools
(`Contents/Resources/broker/bin/nsh`, ...) have a space in their paths.
Connect Apps splits a plugin's command line at spaces outside quotes (#66), so
quote such a path. Google Workspace isn't bundled: it installs from its GitHub
release, as any mini app does, and its native screens are compiled in
(`gws-screens` in `app/Cargo.toml`).

## Checking a release

On a Mac other than the build machine, after downloading the dmg:

```sh
shasum -a 256 -c SHA256SUMS
xcrun stapler validate Buwiz-Bot-<version>-macos-arm64.dmg
spctl -a -vvv -t open --context context:primary-signature Buwiz-Bot-<version>-macos-arm64.dmg
hdiutil attach Buwiz-Bot-<version>-macos-arm64.dmg
codesign --verify --deep --strict --verbose=2 "/Volumes/Buwiz Bot/Buwiz Bot.app"
codesign -dv --verbose=4 "/Volumes/Buwiz Bot/Buwiz Bot.app"   # Authority=Developer ID Application: …, flags=0x10000(runtime), TeamIdentifier
spctl -a -vvv -t exec "/Volumes/Buwiz Bot/Buwiz Bot.app"      # accepted, source=Notarized Developer ID
xcrun stapler validate "/Volumes/Buwiz Bot/Buwiz Bot.app"
```

## latest.json

`bot/latest.json` is a copy of the newest version's `manifest.json`. It has
the shape buwiz.com reads (`src/sites/downloads.rs` in `buwiz-com`), as
eBIRForms' does:

```json
{
  "version": "0.1.0",
  "published_at": "2026-11-07T00:00:00Z",
  "files": [
    {
      "platform": "macos",
      "arch": "arm64",
      "kind": "dmg",
      "file": "Buwiz-Bot-0.1.0-macos-arm64.dmg",
      "size": 153137147,
      "sha256": "<hex>"
    }
  ]
}
```

`file` is relative to `bot/releases/v<version>/`. The site lists a platform
only if the manifest has a file for it, so a new target needs nothing there.
