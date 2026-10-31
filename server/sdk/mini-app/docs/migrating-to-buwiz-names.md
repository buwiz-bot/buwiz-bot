# Protocol 2.0: the buwiz names (breaking)

Protocol 2.0 renames every protocol name from `rox` to `buwiz`. It is a
clean cut: Buwiz Bot no longer accepts the old names anywhere, and it
refuses a manifest that declares protocol 1.x with
`protocol_unsupported`: "this plugin is built for an older protocol;
update it". A plugin keeps working only once it ships a release with the
names below and a manifest re-signed under them.

## Every name, old → new

| Old (1.x, refused) | New (2.0) | Where |
|---|---|---|
| `"protocol": "1.x.y"` | `"protocol": "2.0.0"` | manifest |
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

Unchanged: `src/v1/sign-manifest.mjs`, the JCS rules, the key format,
every other manifest field and limit, and the meaning of each reserved
tool and bridge method.

## Checklist for a plugin

- [ ] Manifest: `"protocol": "2.0.0"`; every `apps[].entry` is
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
first start of 2.0).
