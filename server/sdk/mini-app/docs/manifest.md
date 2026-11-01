# The manifest

`buwiz://manifest` is a UTF-8 JSON object, at most 256 KiB, listed in
`resources/list` and readable with `resources/read` (spec 5.1). Buwiz Bot reads
it after `buwiz.provision` and again on every `resources/list_changed`. Every
read is verified in the order of spec 5.2: I-JSON parse (no duplicate keys),
`protocol` and `version`, trust store, signature, then every grammar and limit
below. Any failure makes the plugin unavailable with `manifest_invalid`.

Unknown **top-level** fields are ignored. Unknown keys inside `permissions`,
`routines[]` entries, `query`, `reads[]` entries, `open` or `pick_file` are a
refusal: objects that grant access fail closed (spec 2.2).

## Top level (spec 5.3)

| Field | Req | Rule |
|---|---|---|
| `protocol` | yes | Semver of the protocol you implement: `"1.0.0"` ([the spec](https://github.com/buwiz-app/buwiz-sdk/blob/main/docs/protocol-1.0.md)). A 1.0 host accepts `1.0.x` and refuses `1.1.x` and up (`protocol_unsupported`: it needs a newer Buwiz Bot). 2.0 and 2.1 were pre-release numbers of 1.0: a manifest naming `2.0.x` or `2.1.x` is refused with `protocol_unsupported`: "<title> uses the pre-release mini app protocol <protocol>; install its 1.0 version". An install from Rox (its tools named `rox.*`) does not start; see [migrating-to-buwiz-names.md](migrating-to-buwiz-names.md). |
| `id` | yes | Reverse DNS, `^[a-z][a-z0-9-]{0,62}(\.[a-z][a-z0-9-]{0,62}){2,7}$`, at most 255 chars. Not `buwiz.`, `com.buwiz.bot` or `com.buwiz.bot.*` (Buwiz Bot's own). Buwiz plugins are `app.buwiz.<name>`. Must be on the trust store's approved list. |
| `short` | yes | `^[a-z][a-z0-9]{1,15}$`. Not `buwiz`, `app`, `chat`, `mcp`, `nsh`, `all`, `office`, `skill`, `routine`, `bot` (the last four are built-in tool prefixes the agent never sees from a plugin). Unique among enabled plugins. Every tool name starts with `<short>_`. |
| `version` | yes | Semver, at most 32 chars. Raise it on every release: a manifest that changes under the same version is `manifest_changed`. |
| `title` | yes | 1 to 60 chars. |
| `description` | no | At most 300 chars. Never sent to the model. |
| `icon` | no | Resource URI of a PNG or SVG, at most 64 KiB. |
| `publisher` | yes | `{id, key_id, name}` (spec 5.4): `id` like `app.buwiz`; `key_id` `^[a-z0-9][a-z0-9-]{0,63}$`; `name` at most 80 chars. |
| `signature` | yes | `{"alg": "ed25519", "key_id": <publisher.key_id>, "value": <86 chars base64url>}` over the JCS bytes of the manifest without `signature` (spec 4.2). Written by `sign-manifest.mjs`; never by hand. |
| `commands` | no | Slash commands, below. |
| `scopes` | no | `@` scopes, below. |
| `routines` | no | Timed agent runs, below. |
| `apps` | no | Apps, below. |
| `needs` | no | Reserved. Absent or `[]`; anything else is `not_supported`. |

## `commands[]` (spec 5.7)

`name` (`^[a-z][a-z0-9-]{0,31}$`, not `status`), `description` (shown in the
popup, approved text), `args` (hint such as `[tin]`, optional), `resolve` (one
of your tools, see [bridge.md](bridge.md#resolve-tools)).

## `scopes[]` (spec 5.8)

`name` (`^[a-z][a-z0-9-]{0,31}$`), `description` (its first word, lower-cased,
is the noun "this <noun>" in the agent's text), `resolve`, `attach` (bool,
default false: whether tokens may be attached to a chat), `hint` (text added
to the system prompt when the scope is in play; approved plugins only).

## `routines[]` (spec 5.9)

`id` (`^[a-z][a-z0-9-]{0,47}$`), `name`, `description`, `every_s`, `query`
`{tool, field, args?}` (your own tool; `structuredContent[field]` is the
search text), `prompt` (approved text), `reads` `[{plugin, tools}]` (read-only
tools of another approved plugin, approved once by the user), `writes`
(optional: your own tools that change data, which the routine's turn may call
without asking). No other keys.

A routine's turn may call your read-only tools (`readOnlyHint: true`) and the
`reads` tools. A tool of yours that changes data runs only when the routine
lists it in `writes`; the broker refuses any other. `writes` names only your
own tools (`<short>_`), each once, and never a read-only one (the plugin is
refused at install otherwise). The install review shows each routine's writes
("Routine 'BIR payments' may change BIR data without asking:
bir_record_payment"), and an update that adds one needs the user's consent.

In the turn, a call to your plugin carries `_meta["buwiz/caller"]` with
`origin: "agent"` and `routine: "<routine id>"`; the query call carries
`origin: "routine"` and the same `routine`. Check `routine` in your own
authorization too (spec P6): allow a write only for a routine that declares
it, and only on evidence the routine was given (BIR records a payment only
when the email's text it passes says so).

## `apps[]` (spec 5.5)

Each app is a TSX screen (`entry`, `mime`, `sdk`, `fingerprint`, all
required) or a native screen (`native`), never both and never neither.

| Field | Req | Rule |
|---|---|---|
| `id` | yes | `^[a-z][a-z0-9-]{0,47}$`, unique in the plugin. |
| `title` | yes | 1 to 60 chars. Window and link title. |
| `description` | yes | At most 300 chars. |
| `entry` | TSX | Exactly `buwiz-app://<id>`, and listed in `resources/list`. |
| `mime` | TSX | `text/x-nsh-tsx`. |
| `sdk` | TSX | `"1"`. |
| `native` | native | `{"screen": "<id>", "abi": 1}`: a screen built into Buwiz Bot (`screen` is 1 to 64 of `a-z`, `0-9`, `.`, `_`, `-`, e.g. `books.ledger`). Only for a first-party key (the trust store's `"first_party": true`, today `gc-2026-01`, publisher `app.buwiz`); any other key's manifest is refused. A native app has no `entry`, `mime`, `sdk` or `fingerprint`. This build verifies it and shows "this screen needs a newer Buwiz Bot" on open; the native loader comes next. |
| `fingerprint` | TSX | `sha256:` + lowercase hex SHA-256 of the source bytes as served (spec 6.2). `sign-manifest.mjs --app <id>=<file>` fills it. |
| `props` | no | JSON Schema subset for `buwiz.props`: `type`, `properties`, `required`, `items`, `enum`, `maxLength`, `minimum`, `maximum`; `type` must be `object` at the top (spec 6.6). |
| `placement` | yes | `["full"]`, `["inline"]` or `["full","inline"]`. |
| `inline` | if inline | `{width, height, min_width?, min_height?, max_height?, resizable?}` in pixels. `height` is the first frame; `min_height` (96..`height`, default 120) and `max_height` (`height`..800, default 640) bound what `buwiz.fit` sets ([bridge](bridge.md#inline-cards-that-fit-their-content-roxfit)). |
| `permissions` | no | What the app may reach; absent means nothing. |

### `apps[].permissions` (spec 5.6)

| Key | Grants | Rule |
|---|---|---|
| `tools` | `buwiz.call` | Your tool names, never `buwiz.*`. |
| `open` | `buwiz.open` | `{apps: [ids of your apps], external: [https origins]}`. |
| `attach` | `buwiz.attach` | Scope names whose scope has `attach: true`. |
| `pick_file` | `buwiz.pickFile` | `{extensions: [".xlsx", …], max_bytes, multiple?}`. |
| `storage` | `buwiz.storage` | `{quota_bytes?}`. |
| `copy` | `buwiz.copy` | `true`. |
| `preview` | `buwiz.preview` | `{max_bytes?}`: Buwiz Bot shows a document one of your `tools` makes (a print preview). `{}` allows the host's 8 MiB. |

The permissions digest is `sha256(JCS(permissions))` in lowercase hex, `{}`
when absent. Your plugin compares it on every `buwiz.session.open`.

## Limits

These are the limits the broker enforces (`LIMITS` in
`src/v1/manifest.mjs`, the spec's Appendix A). `test/v1-sdk.test.mjs`
(SDK-07) fails when this table and the code disagree.

| Field | Limit | Key |
|---|---|---|
| Manifest size | 256 KiB | `LIMITS.manifestBytes` |
| Plugin id | 255 chars | `LIMITS.pluginId` |
| `version` | 32 chars | `LIMITS.version` |
| `title`, `apps[].title` | 60 chars | `LIMITS.title` |
| `description`, `apps[].description` | 300 chars | `LIMITS.description` |
| `publisher.name` | 80 chars | `LIMITS.publisherName` |
| `commands` | 32 entries | `LIMITS.commands` |
| `scopes` | 16 entries | `LIMITS.scopes` |
| `routines` | 8 entries | `LIMITS.routines` |
| `apps` | 16 entries | `LIMITS.apps` |
| `commands[].description` | 200 chars | `LIMITS.commandDescription` |
| `commands[].args` | 120 chars | `LIMITS.commandArgs` |
| `scopes[].description` | 120 chars | `LIMITS.scopeDescription` |
| `scopes[].hint` | 1500 chars | `LIMITS.scopeHint` |
| `routines[].name` | 80 chars | `LIMITS.routineName` |
| `routines[].description` | 300 chars | `LIMITS.routineDescription` |
| `routines[].prompt` | 4000 chars | `LIMITS.routinePrompt` |
| `routines[].every_s` | 60 to 86400 s | `LIMITS.everyS` |
| `routines[].reads` | 4 entries | `LIMITS.reads` |
| `routines[].reads` tools, all entries together | 8 tools | `LIMITS.readsTools` |
| `routines[].writes` | 8 tools | `LIMITS.writes` |
| `permissions.tools` | 64 entries | `LIMITS.permTools` |
| `permissions.open.apps` | 16 entries | `LIMITS.openApps` |
| `permissions.open.external` | 8 entries | `LIMITS.openExternal` |
| `permissions.attach` | 16 entries | `LIMITS.attach` |
| `permissions.pick_file.extensions` | 16 entries | `LIMITS.pickExtensions` |
| `permissions.pick_file.max_bytes` | 50 MiB | `LIMITS.pickMaxBytes` |
| `permissions.storage.quota_bytes` | 10 MiB | `LIMITS.storageQuota` |
| `permissions.storage` default quota | 1 MiB | `LIMITS.storageDefault` |
| `permissions.preview.max_bytes` (and its default) | 8 MiB | `LIMITS.previewMaxBytes` |
| Props, serialized | 8 KiB | `LIMITS.propsBytes` |
| Props and props schema depth | 8 levels | `LIMITS.propsDepth` |
| Props array items | 100 items | `LIMITS.propsArray` |
| Props schema `properties` | 16 properties | `LIMITS.schemaProperties` |
| Props schema `maxLength` | 512 chars | `LIMITS.schemaMaxLength` |
| `inline.width` (and `min_width`) | 240 to 960 px | `LIMITS.inlineWidth` |
| `inline.height` | 96 to 800 px | `LIMITS.inlineHeight` |
| `inline.min_height` / `inline.max_height` | 96 to `height` / `height` to 800 px | `LIMITS.inlineHeight` (as `inline.height`) |

Limits outside the manifest (bridge calls, results, storage, timeouts) are
in [bridge.md](bridge.md#limits).

Section numbers such as spec 4.1 refer to the
[mini app protocol 1.0 specification](https://github.com/buwiz-app/buwiz-sdk/blob/main/docs/protocol-1.0.md)
(2.0 and 2.1 were its pre-release numbers).
