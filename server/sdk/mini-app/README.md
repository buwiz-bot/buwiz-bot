# Rox mini app kit: start here

Build a first-party plugin for the Rox Mini App Protocol v1: an MCP server
that ships apps into Rox. Read [docs/overview.md](docs/overview.md) first (one
page), then follow these ten steps. Paths are from the repo root.

1. **Copy the template** next to it, so its one import
   (`../lib/rox-plugin.mjs`) still resolves:

   ```sh
   cp -R sdk/mini-app/template sdk/mini-app/hello-world
   ```

2. **Rename it.** In `manifest.json` set `id` (reverse DNS, e.g.
   `dev.goldcoders.notes`), `short` (2 to 16 lowercase letters and digits,
   e.g. `notes`), `title`, `description`, and start `version` at `0.1.0`. Every
   tool name must start with `<short>_` ([docs/manifest.md](docs/manifest.md)).

3. **Write your tools** in `server.mjs`: each is
   `{name, description, inputSchema, annotations, run(args, caller)}`.
   Answer with `ok(data)` or `fail(code, message)`. Mark read-only tools
   `readOnlyHint: true`. Check every ticket argument against your own store
   ([docs/security.md](docs/security.md), P1).

4. **Write the resolve tool** for your commands and scopes: it answers
   `choices`, `open`, `token` or `error`, with tickets from
   `tickets.issue(subject)` instead of raw identifiers
   ([docs/bridge.md](docs/bridge.md#resolve-tools)).

5. **Write the app** (`hello.tsx` → `<id>.tsx`): a default-exported
   component that imports only `nsh`, `nsh/base` and `rox`, reads
   `rox.props`, calls your tools with `rox.call`, and sizes itself from
   `rox.viewport`. List it in `startPlugin({apps})` and in the manifest's
   `apps[]`, and grant it exactly the tools it calls in `permissions.tools`.

6. **Make a local test key** (once; it is not a release key):

   ```sh
   npm run plugin-test -- --keygen ~/.config/rox/test.key
   ```

7. **Compute fingerprints and sign** for local tests. `sign-manifest.mjs`
   fills each app's `fingerprint` from its source and signs the result:

   ```sh
   node server/v1/sign-manifest.mjs sdk/mini-app/hello-world/manifest.json \
     --key-file ~/.config/rox/test.key \
     --app hello=sdk/mini-app/hello-world/hello.tsx --out /tmp/hello.signed.json
   ```

8. **Run the test runner** until every line passes
   ([docs/testing.md](docs/testing.md)):

   ```sh
   npm run plugin-test -- --command node --args sdk/mini-app/hello-world/server.mjs \
     --manifest /tmp/hello.signed.json --key-file ~/.config/rox/test.key
   ```

   (`--sign-with ~/.config/rox/test.key` instead of `--manifest … --key-file …`
   does step 7 for you on every run.)

9. **Go through the security checklist** ([docs/security.md](docs/security.md)):
   the runner checks provisioning, certificates, key hygiene and ticket titles;
   authorization (P1), side effects (P5, P6) and text (P9) are yours to review.

10. **Release: the owner signs.** Hand the owner the plugin with its unsigned
    `manifest.json` and app sources. The owner adds the plugin id to
    `approved_plugins` in `server/v1/trust.json` and signs with the Goldcoders
    key from the login Keychain (key id `gc-2026-01`, the manifest's
    `publisher.key_id`):

    ```sh
    node server/v1/sign-manifest.mjs sdk/mini-app/hello-world/manifest.json \
      --keychain gc-2026-01 --app hello=sdk/mini-app/hello-world/hello.tsx \
      --out sdk/mini-app/hello-world/manifest.json
    ```

    The signed `manifest.json` ships beside `server.mjs`, which serves it when
    `ROX_PLUGIN_MANIFEST` is unset. Any change to an app's source or to the
    manifest needs a new `version` and a new signature.

## What is where

| Path | What |
|---|---|
| `template/server.mjs` | The plugin: one tool (`hello_greet`), one resolve tool (`hello_resolve`) for the `/hello` command and the attachable `@hello:friend` scope, tickets |
| `template/hello.tsx` | The app: full screen and inline 360×160 |
| `template/manifest.json` | The unsigned manifest; fingerprints are placeholders until step 7 |
| `lib/rox-plugin.mjs` | `startPlugin`, `ticketStore`, `ok`, `fail`: the reserved tools of spec 7.8, done once |
| `bin/rox-plugin-test.mjs` | The test runner |
| `docs/` | [overview](docs/overview.md), [manifest](docs/manifest.md), [bridge](docs/bridge.md), [security](docs/security.md), [testing](docs/testing.md) |

11. **Make cards fast** ([docs/performance.md](docs/performance.md)): render
    answer cards from their snapshot props, keep module top levels empty and
    sources small, declare the inline size, and read Rox's `mini v1 timing:`
    log to see where an open's time goes.
