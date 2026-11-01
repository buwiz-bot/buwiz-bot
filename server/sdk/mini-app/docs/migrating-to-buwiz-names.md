# Protocol 1.0: the buwiz names (breaking)

Mini app protocol 1.0 ([the spec](https://github.com/buwiz-app/buwiz-sdk/blob/main/docs/protocol-1.0.md)) renames every protocol name
of Rox's from `rox` to `buwiz`. (2.0 and 2.1 were pre-release numbers of
1.0.) It is a clean cut: Buwiz Bot no longer accepts the old names
anywhere; an install from Rox (its tools named `rox.*`, whatever protocol
it names) fails before it starts with "… was made for an older Buwiz Bot
… Install its current version, or uninstall it", and a manifest naming
2.0.x or 2.1.x is refused with `protocol_unsupported`: "<title> uses the
pre-release mini app protocol <protocol>; install its 1.0 version". A
plugin keeps working only once it ships a release with the names below
and a manifest re-signed under them.

## Every name, old → new

| Old (Rox, refused) | New (1.0) | Where |
|---|---|---|
| `"protocol": "1.x.y"` (Rox), `"2.0.0"`, `"2.1.0"` (pre-release) | `"protocol": "1.0.0"` (the host accepts exactly 1.0) | manifest |
| `dev.goldcoders.bir` | `app.buwiz.ebirforms` | plugin id (manifest `id`, trust store) |
| `dev.goldcoders.books` | `app.buwiz.books` | plugin id |
| `dev.goldcoders.google-workspace` | `app.buwiz.google-workspace` | the bundled plugin's id |
| `publisher.id: "dev.goldcoders"` | `publisher.id: "app.buwiz"`, `key_id: "gc-2026-01"` (same key) | manifest `publisher` |
| `rox.provision` | `buwiz.provision` | reserved tool your plugin answers (first) |
| `rox.session.open` | `buwiz.session.open` | reserved tool |
| `rox.session.close` | `buwiz.session.close` | reserved tool |
| `rox.ticket.resolve` | `buwiz.ticket.resolve` | reserved tool |
| `rox.uninstall` | `buwiz.uninstall` | reserved tool |
| any other `rox.*` → `-32601` | any other `buwiz.*` → `-32601` | reserved names; `rox.*` is now just an invalid tool name |
| `rox://manifest` | `buwiz://manifest` | resource your plugin serves and lists |
| `rox-app://<app id>` | `buwiz-app://<app id>` | `apps[].entry` and the app source resources |
| `typ: "rox-session"` | `typ: "buwiz-session"` | the session certificate your plugin signs |
| `typ: "rox-call"` | `typ: "buwiz-call"` | the call envelope Buwiz Bot signs (your plugin never sees it) |
| `_meta["rox/caller"]` | `_meta["buwiz/caller"]` | sent with every call to your tools |
| `_meta["rox/open"]` | `_meta["buwiz/open"]` | a tool result's card |
| `_meta["rox/suggest"]` | `_meta["buwiz/suggest"]` | a tool result's next steps |
| `_meta["rox/audience"]` | `_meta["buwiz/audience"]` | a tool's audience in tools/list |
| link provider `"rox-app"` | `"buwiz-app"` | Buwiz Bot's own links |
| `import { rox, RoxError } from "rox"` | `import { buwiz, BuwizError } from "buwiz"` | your apps (TSX) |
| `globalThis.rox`, `globalThis.RoxError` | `globalThis.buwiz`, `globalThis.BuwizError` | your apps |
| `rox.props`, `rox.call`, `rox.open`, `rox.fit`, `rox.ready`, `rox.actions`, `rox.onAction`, `rox.settle`, `rox.meta`, `rox.attach`, `rox.pickFile`, `rox.storage`, `rox.copy`, `rox.preview`, `rox.ticket.title`, `rox.viewport`, `rox.app` | the same members on `buwiz` | your apps |
| `rox://app/…`, `rox://chat/…`, `rox://view?path=…`, `rox://plugin/…` | `buwiz://app/…`, `buwiz://chat/…`, `buwiz://view?path=…`, `buwiz://plugin/…` | links your plugin or app writes; `rox://` is no longer routed |
| `X-Rox-Host`, `X-Rox-Line`, `X-Rox-Signature` | `X-Buwiz-Host`, `X-Buwiz-Line`, `X-Buwiz-Signature` | broker headers; agent webhooks are signed with `X-Buwiz-Signature` only |
| `ROX_BROKER_DIR`, `ROX_BROKER_CMD`, `ROX_BROKER_BIN` | `BUWIZ_BROKER_DIR`, `BUWIZ_BROKER_CMD`, `BUWIZ_BROKER_BIN` | env your build and tests read |
| `ROX_TRUST_FILE`, `ROX_KEYSTORE`, `ROX_PLUGIN_MANIFEST` | `BUWIZ_TRUST_FILE`, `BUWIZ_KEYSTORE`, `BUWIZ_PLUGIN_MANIFEST` | env the broker and the SDK template read |
| `lib/rox-plugin.mjs` | `lib/buwiz-plugin.mjs` | SDK |
| `bin/rox-plugin-test.mjs` | `bin/buwiz-plugin-test.mjs` (`npm run plugin-test`) | SDK |
| plugin ids `rox.*`, `dev.goldcoders.rox.*` reserved | `buwiz.*`, `com.buwiz.bot`, `com.buwiz.bot.*` reserved; short `buwiz` reserved | manifest `id`, `short` |

New in 1.0: an app may be a native screen instead of TSX,
`"native": {"screen": "<id>", "abi": 1}` with no `entry`, `mime`, `sdk` or
`fingerprint`, for first-party plugins only (key `gc-2026-01`, publisher
`app.buwiz`). This build verifies such apps; opening one says "this screen
needs a newer Buwiz Bot" until the native loader ships.

Unchanged: `src/v1/sign-manifest.mjs`, the JCS rules, the key format,
every other manifest field and limit, and the meaning of each reserved
tool and bridge method.

## Checklist for a plugin

- [ ] Manifest: `"protocol": "1.0.0"`; every `apps[].entry` is
      `buwiz-app://<id>`; raise `version`.
- [ ] Reserved tools: answer `buwiz.provision`, `buwiz.session.open`,
      `buwiz.session.close`, `buwiz.ticket.resolve`, `buwiz.uninstall`;
      list them in tools/list; `-32601` for any other `buwiz.*`. Drop the
      `rox.*` names (they no longer pass the tool grammar check).
- [ ] Session certificates: `typ: "buwiz-session"`.
- [ ] Resources: serve and list `buwiz://manifest` and `buwiz-app://<id>`.
- [ ] Tools: read `_meta["buwiz/caller"]`; write `buwiz/open`,
      `buwiz/suggest`, and tool `_meta` `buwiz/audience`.
- [ ] Apps: `import { buwiz } from "buwiz"` (and `BuwizError`); every
      `rox.` member becomes `buwiz.`; links you build use `buwiz://`.
- [ ] Build and test env: `BUWIZ_BROKER_DIR`, `BUWIZ_BROKER_CMD`,
      `BUWIZ_BROKER_BIN`, `BUWIZ_TRUST_FILE`, `BUWIZ_KEYSTORE`.
- [ ] Re-sign the manifest under the new names with the Goldcoders key
      `gc-2026-01` (the manifest's `publisher.key_id`), app fingerprints
      included:

      ```sh
      node src/v1/sign-manifest.mjs <plugin>/manifest.json \
        --keychain gc-2026-01 --app <id>=<plugin>/<id>.tsx … \
        --out <plugin>/manifest.signed.json
      ```

- [ ] Run `npm run plugin-test` (`bin/buwiz-plugin-test.mjs`) against the
      signed manifest until every line passes.

## buwiz-forms (BIR, `app.buwiz.ebirforms`)

Its reserved tools live in `crates/bir-shell/src/mini_app/v1/` (its own
implementation, not the SDK): `manifest.rs` (protocol, entries),
`mod.rs` (the reserved tool names, `buwiz://manifest`, resources),
`keys.rs` (the certificate `typ`), `app_tools.rs` and `tickets.rs`
(`_meta` keys), the TSX apps (the `buwiz` module), `tests.rs` and the
demo tests. Then re-sign `BIR_V1_MANIFEST` with `gc-2026-01`.

## buwiz-books (`app.buwiz.books`)

Its reserved tools live in `crates/books-mini-app/src/`: `manifest.rs`
(the generator: protocol, entries), `reserved.rs` (the reserved tool
names, the certificate `typ`), `jsonrpc.rs` and `service.rs`
(`buwiz://manifest`, `buwiz-app://` sources), `tools.rs`, `cards.rs` and
`apps.rs` (`_meta` keys, the `buwiz` module), and
`tests/manifest_shape.rs`, `tests/stdio_roundtrip.rs`,
`tests/chat_cards.rs`. Then re-sign `manifest.signed.json`
(`BOOKS_V1_MANIFEST`) with `gc-2026-01`.

Until each ships, Buwiz Bot shows it as failed with the update message;
chats keep their history (their links were rewritten to `buwiz://` at the
first start of Buwiz Bot).

## Environment variables, scripts and temp files (old names still work)

Separately from the protocol, Buwiz Bot's own environment variables,
scripts and temp files now have Buwiz names. Unlike the protocol names
above, the old ones keep working for now: a variable set only by its old
name is read, and the program says once on stderr
`<old> is an old name; use <new>`. Removing the old names is a later
decision. The table lives in `broker/rox-v1/src/env.rs` and
`server/src/env.mjs` (a test checks they match).

| Old (still read, with a warning) | New |
|---|---|
| `ROX_BACKEND` | `BUWIZ_BACKEND` |
| `ROX_BACKGROUND` | `BUWIZ_BACKGROUND` |
| `ROX_DATA_DIR` | `BUWIZ_DATA_DIR` |
| `ROX_HOST_TOKEN_FILE` | `BUWIZ_HOST_TOKEN_FILE` |
| `ROX_JANK_LOG` | `BUWIZ_JANK_LOG` |
| `ROX_FPS` | `BUWIZ_FPS` |
| `ROX_MACHINE_ID` | `BUWIZ_MACHINE_ID` |
| `ROX_MINI_CACHE` | `BUWIZ_MINI_CACHE` |
| `ROX_OFFICE_FONTS` | `BUWIZ_OFFICE_FONTS` |
| `ROX_STATUS` | `BUWIZ_STATUS` |
| `ROX_PANE` | `BUWIZ_PANE` |
| `ROX_PLUGINS` | `BUWIZ_PLUGINS` |
| `ROX_MODELS` | `BUWIZ_MODELS` |
| `ROX_SIDE` | `BUWIZ_SIDE` |
| `ROX_ACCOUNT` | `BUWIZ_ACCOUNT` |
| `ROX_TAB` | `BUWIZ_TAB` |
| `ROX_AVATAR` | `BUWIZ_AVATAR` |
| `ROX_OPEN` | `BUWIZ_OPEN` |
| `ROX_SCROLL` | `BUWIZ_SCROLL` |
| `ROX_SEND` | `BUWIZ_SEND` |
| `ROX_PALETTE` | `BUWIZ_PALETTE` |
| `ROX_SLOTS` | `BUWIZ_SLOTS` |
| `ROX_MINI` | `BUWIZ_MINI` |
| `ROX_CARD` | `BUWIZ_CARD` |
| `ROX_GAZE` | `BUWIZ_GAZE` |
| `ROX_PILL` | `BUWIZ_PILL` |
| `ROX_OVERLAY` | `BUWIZ_OVERLAY` |
| `ROX_BROKER_HUB` | `BUWIZ_BROKER_HUB` |
| `ROX_BROKER_V1_HOST` | `BUWIZ_BROKER_V1_HOST` |
| `ROX_BROKER_MCP_HOST` | `BUWIZ_BROKER_MCP_HOST` |
| `ROX_BROKER_NO_NODE` | `BUWIZ_BROKER_NO_NODE` |
| `ROX_BROKER_FORWARD_ALL` | `BUWIZ_BROKER_FORWARD_ALL` |
| `GROK_DATA_DIR` | `BUWIZ_BROKER_DATA_DIR` |
| `GROK_ALLOWED_ORIGINS` | `BUWIZ_ALLOWED_ORIGINS` |
| `GROK_BUNDLE_ID` | `BUWIZ_BUNDLE_ID` |
| `ROX_PUBLIC_PORT` | `BUWIZ_PUBLIC_PORT` |
| `ROX_NODE_ROLE` | `BUWIZ_NODE_ROLE` |
| `ROX_PLUGIN_CATALOG` | `BUWIZ_PLUGIN_CATALOG` |
| `ROX_V1_LINE` | `BUWIZ_V1_LINE` |
| `ROX_V1_LINE_TOKEN` | `BUWIZ_V1_LINE_TOKEN` |
| `ROX_LINE_HOSTS` | `BUWIZ_LINE_HOSTS` |
| `ROX_LINE_TOKEN` | `BUWIZ_LINE_TOKEN` |
| `ROX_V1_CALL_TIMEOUT_MS` | `BUWIZ_V1_CALL_TIMEOUT_MS` |
| `ROX_V1_MANUAL_CLOCK` | `BUWIZ_V1_MANUAL_CLOCK` |
| `ROX_BRIDGE_URL` | `BUWIZ_BRIDGE_URL` |
| `ROX_BRIDGE_TOKEN` | `BUWIZ_BRIDGE_TOKEN` |
| `ROX_STUB_LOG` | `BUWIZ_STUB_LOG` |
| `ROX_DEFS_WRITE` | `BUWIZ_DEFS_WRITE` |
| `ROX_APP` | `BUWIZ_APP` |
| `ROX_LOG` | `BUWIZ_LOG` |
| `ROX_BUILD_LOCK` | `BUWIZ_BUILD_LOCK` |
| `ROX_BUILD_HOLD` | `BUWIZ_BUILD_HOLD` |
| `ROX_BIN` | `BUWIZ_BOT_BIN` |
| `ROX_BROKER_BIN` | `BUWIZ_BROKER_BIN` |
| `${ROX_FILES_DIR}` (placeholder in a plugin's `.mcp.json`) | `${BUWIZ_FILES_DIR}` |

`BUWIZ_BACKEND`, `BUWIZ_BACKGROUND=1` or `BUWIZ_DATA_DIR` make an app a test
instance, and so does any of them by its old name.

| Old (still works) | New |
|---|---|
| `scripts/test-rox` | `scripts/buwiz-test` (the old one runs it) |
| `scripts/rox-agent` | `scripts/buwiz-agent` (the old one runs it) |
| `scripts/broker-live use-rox` | `scripts/broker-live use-bundled` (`status` says `mode: bundled`) |
| `$TMPDIR/rox-gpui-agent-token` | `$TMPDIR/buwiz-gpui-agent-token` (the dev launcher writes both) |
| `$TMPDIR/rox-agent-menu` | `$TMPDIR/buwiz-agent-menu` (the app reads both) |
| `$TMPDIR/rox-build.mutex`, `rox-build.hold` | `$TMPDIR/buwiz-build.mutex`, `buwiz-build.hold` (`scripts/wait-build` takes both locks and honours both hold files) |
