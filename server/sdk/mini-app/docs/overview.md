# Mini apps for Rox: overview

A **plugin** is a stdio MCP server that ships **mini apps** to Rox. It serves
three kinds of thing ([spec] 1, 5.1, 6.1):

- **tools**, which apps, the model, slash commands and `@` tokens call;
- the **manifest**, the resource `rox://manifest`: a signed JSON document that
  names the plugin, its commands, scopes, routines and apps, and what each
  app may do;
- **apps**, TSX modules served as `rox-app://<app id>` resources, which Rox
  compiles and runs full screen or inline in the chat.

## The three parties

| Party | Owns | Trusts |
|---|---|---|
| **Rox** (the host) | The UI, the app sandbox, the `rox` bridge object, consent prompts, session keys, per-app storage | The trust store it ships: publisher keys and approved plugin ids ([spec] 4.1) |
| **The broker** | Plugin processes, install records, plugin keys, verification of manifests, certificates and every signed call | The same trust store; nothing a plugin says until its manifest verifies ([spec] 5.2) |
| **The plugin** (you) | Its tools, its subjects (taxpayers, documents…) and the tickets that stand for them, its apps' source | Only the broker, the one process holding its stdin ([spec] 8.6) |

An app never talks to the plugin directly. It calls `rox.call(tool, args)`;
Rox signs the call in native code with a key the app cannot read; the broker
checks the signature, the session and the app's permissions, then calls the
tool with `_meta["rox/caller"]` saying who asked ([spec] 8).

## The lifecycle in one picture

```
 publisher (offline key)            Rox + broker                          plugin process
 ───────────────────────            ────────────                          ──────────────
 sign-manifest.mjs  ──signed manifest, app fingerprints──▶ (shipped with the plugin)

 install ─────────────────────────▶ spawn ───────────────────────────────▶ start (no key)
                                    rox.provision {key seed}  ───────────▶ key kept in memory
                                    ◀─────────────── {ok, manifest_sha256}
                                    read rox://manifest; verify (5.2)
                                    review screen → user clicks Install
 open an app ─────────────────────▶ session key Ks (Rox); rox.session.open ▶ checks app + perms digest
                                    ◀─────────────────────── cert signed by plugin key
                                    read rox-app://<id>; fingerprint check; compile
 rox.call(tool, args) in the app ─▶ envelope signed by Ks; 9 checks (8.2) ─▶ tools/call + _meta["rox/caller"]
                                    ◀────────────────────────────── result (`_meta` stripped)
 close ───────────────────────────▶ rox.session.close ───────────────────▶ forget the open
 crash ───────────────────────────▶ restart with backoff, provision again, new generation (14.4)
 uninstall ───────────────────────▶ rox.uninstall ───────────────────────▶ delete tickets and subjects
                                    delete key, pins, attachments, storage (13.6)
```

## What the kit gives you

| Path | What |
|---|---|
| `sdk/mini-app/template/` | The smallest correct plugin: copy it ([README](../README.md)) |
| `sdk/mini-app/lib/rox-plugin.mjs` | The reserved tools of [spec] 7.8, done once: `startPlugin`, `ticketStore`, `ok`, `fail` |
| `sdk/mini-app/bin/rox-plugin-test.mjs` | The test runner (`npm run plugin-test`), see [testing.md](testing.md) |
| `server/v1/sign-manifest.mjs` | Fills fingerprints and signs a manifest |
| `server/v1/fixture/` | The conformance fixture: every feature, used by the broker's own suite |

## Read next

- [manifest.md](manifest.md): every field and limit.
- [bridge.md](bridge.md): the `rox` object apps use.
- [security.md](security.md): what your plugin must do (P1 to P10).
- [testing.md](testing.md): the runner and what it checks.

[spec]: ../../../../rox/drafts/rox-mini-app-protocol-v1.md
