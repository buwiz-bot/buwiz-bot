# Testing a plugin: `npm run plugin-test`

`sdk/mini-app/bin/rox-plugin-test.mjs` runs the plugin-side checks of
[spec] 17 against one plugin and prints a checklist with the spec ID of each
check. It spawns your plugin the way the broker does and runs the broker's
real v1 code in its own process (`server/v1/plugins.mjs`, `sessions.mjs`,
`manifest.mjs`) on an in-memory database, a file keystore in a temp
directory and a trust file written for the run. It opens no port, never
reads Rox's or the broker's data directory and never touches the login
Keychain.

## Run it

```sh
# once: a local test key (0600 file; never a release key)
npm run plugin-test -- --keygen ~/.config/rox/test.key

# sign what the plugin serves with the test key, then test
npm run plugin-test -- --command node --args sdk/mini-app/hello-world/server.mjs \
  --sign-with ~/.config/rox/test.key

# or test a manifest you signed yourself (sign-manifest.mjs --key-file)
npm run plugin-test -- --command node --args sdk/mini-app/hello-world/server.mjs \
  --manifest /tmp/hello.signed.json --key-file ~/.config/rox/test.key
```

| Option | Meaning |
|---|---|
| `--command <cmd>` | The program to spawn (stdio only). |
| `--args <arg>` | One argument; repeat for more, or pass a JSON array. Relative paths are from the repo root (where npm runs). |
| `--env KEY=VAL` | An environment variable for the plugin; repeat for more. |
| `--sign-with <key file>` | Read the manifest the plugin serves, fill each app's fingerprint from the source it serves, sign with this key. |
| `--key-id <id>` | With `--sign-with`: the `publisher.key_id` to sign under (default: the manifest's). |
| `--manifest <file>` | A manifest you signed. Trusted with `--key-file`, else with the shipped `server/v1/trust.json`. |
| `--key-file <key file>` | With `--manifest`: the key it was signed with. |
| `--manifest-env NAME` | The variable that tells the plugin where its signed manifest is (default `ROX_PLUGIN_MANIFEST`, which the template reads). |
| `--json` | Print `{plugin, rows: [{id, what, status, note}], ok}` instead of the checklist. |
| `--keygen <file>` | Write a new test key (0600) and print its public key. |

Exit status: 0 when nothing failed, 1 when a check failed, 2 for a usage error.

## What it checks

| ID | Check |
|---|---|
| MAN-01 | The manifest is signed by the trusted key; the signature verifies over JCS. |
| MAN-02 | A byte changed in the manifest fails verification. |
| MAN-03 | The manifest is a JSON object (I-JSON). |
| MAN-04 | `protocol` 1.0.x; `version` is semver. |
| MAN-05 | `needs` absent or `[]`. |
| MAN-06 | `permissions` hold only known keys. |
| MAN-07, MAN-08 | Every grammar and limit ([manifest.md](manifest.md)). |
| MAN-10 | Every tool starts with `<short>_`; every tool the manifest names exists. |
| MAN-11, MAN-12 | No built-in command name; no unknown routine keys. |
| APP-01 | Each app's source is one `text/x-nsh-tsx` content, at most 512 KiB, whose fingerprint matches; the broker's own source check passes too. |
| APP-02 | Each app compiles as TSX and has a default export. |
| APP-03 | Each app imports only `nsh`, `nsh/base`, `rox`. |
| APP-04 | Each app uses no `fetch`, `XMLHttpRequest`, `WebSocket` or web storage. |
| RSV-01 | A tool called before `rox.provision` fails; provision answers `{ok, manifest_sha256}` of the served manifest, twice with the same key. |
| RSV-02 | `rox.session.open` refuses an unknown app, a wrong `perms_sha256` and `not_after` past 8 h with `invalid_args`. |
| RSV-03 | The certificate is exactly the request, signed with the provisioned key over JCS; the seed never appears in stderr. |
| RSV-04 | `rox.ticket.resolve` answers `{valid: false}` for an unknown ticket, and only `{valid, title}` for one the plugin issued. |
| RSV-05 | `rox.session.close` and `rox.uninstall` answer `ok: true`; an unknown `rox.*` is `-32601`; the broker's uninstall runs. |
| LCY-01 | The broker's install gate passes: provision, verify, `tools/list`. |
| BRG-03 | A session opens per app with tools and the broker verifies its certificate; one signed call per tool in `permissions.tools` reaches the plugin and is answered (an `isError` answer counts: the call went through). |
| BRG-04 | The same envelope sent again is `replayed`. |
| BRG-06 | An envelope signed by another key is `bad_signature`. |
| INV-04 | Each scope's resolve, with `complete` true and false, answers a [spec] 9.2 shape within its caps (checked on the raw answer, before the broker would cap it); an attachable scope resolves a chosen completion to a token. |
| INV-05 | Each command's resolve answers a 9.2 shape; an `open` names one of this plugin's apps with valid props. |

The calls send `{}` as arguments and the probe value `sdk-probe` to resolve
tools. Keep side effects behind explicit arguments (P5) and these calls are
harmless.

Not checked here: everything only Rox can show (consent, the sandbox, storage,
placement; [spec] 17.2 to 17.8 host rows), MAN-09 (it needs other installs),
and the review items of [security.md](security.md).

## In the repo's suites

`test/v1-sdk.test.mjs` runs the runner against the template and the fixture,
and against the fixture with `--misbehave=skip-provision`, `bad-cert` and
`log-seed` to prove it catches a broken plugin. `npm run conformance` lists
those as extension IDs SDK-01 onward, after the spec 17 checklist.

[spec]: ../../../../rox/drafts/rox-mini-app-protocol-v1.md
