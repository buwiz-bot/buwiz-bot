# The bridge: `rox` in an app

An app imports the bridge with `import { rox } from "rox"` (it is also the
global `rox`). Its methods are implemented by Rox; the app reaches nothing
else: no `fetch`, `XMLHttpRequest`, `WebSocket`, `localStorage` or
`sessionStorage`, and imports only from `nsh`, `nsh/base` and `rox` ([spec]
6.3). The module's **default export** is the component Rox mounts.

A method whose permission the manifest does not grant is **absent**
(`undefined`), not present and failing ([spec] 7.1, 6.4). Test for it if an
app is shared between manifests: `if (rox.copy) …`.

## Methods

| Member | Permission | Returns | Rejects with | Spec |
|---|---|---|---|---|
| `rox.props` | always | The open's props, validated against `apps[].props`; read-only | | 6.6 |
| `rox.app` | always | `{plugin, app, version, fingerprint, protocol, sdk}` | | 7.2 |
| `rox.viewport` | always | Signal of `{width, height, placement: "full" \| "inline"}` | | 7.1 |
| `rox.ticket.title(ticket)` | always | The ticket's title, or `null` | `rate_limited` | 7.4 |
| `rox.call(tool, args?)` | `tools` | `structuredContent`, else the text contents joined with `\n` | `permission_denied`, `disabled`, `not_found`, `invalid_args`, `tool_error`, `result_too_large`, `timeout`, `rate_limited`, `plugin_unavailable`, `plugin_restarted`, `closed` | 7.3 |
| `rox.open({app, props?, ticket?})` | `open.apps` | Pushes your app on the full-screen stack; no prompt | `permission_denied`, `not_declared`, `invalid_args` | 7.5 |
| `rox.open({url})` | `open.external` | Opens an `https` URL of a listed origin after the user agrees | `permission_denied`, `consent_declined`, `not_supported` | 7.5 |
| `rox.attach(scope, value)` | `attach` | `{token, title, attached}` after "Attach … to this chat?" | `permission_denied`, `consent_declined`, `no_chat` | 7.6 |
| `rox.pickFile({extensions?, multiple?})` | `pick_file` | `[{name, size, type, read()}]`, never a path; `[]` on cancel | `permission_denied`, `quota_exceeded`, `closed` (from `read()` after close) | 7.7 |
| `rox.storage.get/set/remove/keys` | `storage` | Strings; private to this app | `quota_exceeded`, `invalid_args` | 11 |
| `rox.copy(text)` | `copy` | Copies to the clipboard | | 5.6 |
| `rox.preview({tool, args?, title?})` | `preview` | Shows the document `tool` makes over the app, with Print and Close; resolves once shown | `not_declared`, `invalid_args`, `tool_error`, `result_too_large`, `not_supported`, and `rox.call`'s | 7.10 |

Every rejection is a `RoxError {code, retryable, message, details?}`
([spec] 14.1). Retryable: `rate_limited`, `timeout`, `plugin_unavailable`,
`plugin_restarted`, `internal`. A tool result with `isError: true` rejects
with `tool_error` and the first text content (at most 500 chars) as
`message`, so put the user-facing sentence first.

```tsx
import { signal, Text } from "nsh";
import { rox } from "rox";

export default function Status() {
  const line = signal("…");

  rox.call("hello_greet", { ticket: rox.props.ticket }).then(
    (r) => line.set(String(r.greeting)),
    (e) => line.set(e.retryable ? "Busy, try again" : `Could not load (${e.code})`),
  );

  return <Text>{line()}</Text>;
}
```

## Print preview: `rox.preview`

`rox.preview({tool, args?, title?})` asks Rox to call `tool` (one of the
app's own `permissions.tools`; another name rejects `not_declared`) with
`args`, as the app, and show the document it returns full screen over the
app: a title bar with `title` (else the result's `structuredContent.title`,
else the app's title), **Print** (the macOS print panel, which also saves a
PDF) and **Close**. The document never passes through the app, so it may
be larger than a `rox.call` result: up to `permissions.preview.max_bytes`
(8 MiB at most). The call is signed and checked like `rox.call`; its
envelope carries `"purpose": "preview"`, which the broker accepts only from
an app with `preview`.

The tool returns the page as an MCP embedded resource:

```json
{
  "content": [{ "type": "resource",
                "resource": { "uri": "my://print/1601C.html", "mimeType": "text/html", "text": "<!DOCTYPE html>…" } }],
  "structuredContent": { "title": "Form 1601C — Dela Cruz Jane" }
}
```

Make the page **static**: Rox renders it with JavaScript off, no base URL
and a policy that loads nothing (no network, no files; inline `<style>` and
`data:` images, fonts and objects, such as an embedded PDF, only), and it does not follow links or navigate.
A result without a `text/html` resource rejects `tool_error`; `isError`
rejects `tool_error` with its first text; a larger page `result_too_large`.
Rox on other platforms rejects `not_supported`.

```tsx
if (rox.preview) await rox.preview({ tool: "acme_print_page", args: { ticket: rox.props.ticket } });
```

## What your tool receives

A bridge call reaches your tool as `tools/call` with the arguments as sent and
`_meta["rox/caller"]` = `{origin, plugin, app, open_id, ts}` ([spec] 8.4).
`origin` is `app` for bridge calls, `agent` for the model, `routine` for a
routine's query and `resolve` for resolve tools. `startPlugin` passes it to
your `run(args, caller)` as `caller`. Use it to authorize (see
[security.md](security.md)).

To offer the user an app from a tool result, add
`_meta["rox/open"] = {app, props?, ticket?, title, placement?}` to the result
([spec] 9.6). Rox shows a card with Open or Show; it never opens by itself.
It is ignored on `isError` results and never reaches the model.

## Resolve tools

A command or scope names a resolve tool ([spec] 9.2). It receives
`{kind: "command" | "scope", name, value, complete, context: {attached}}` and
answers `structuredContent` with one `result`:

| `result` | Fields | Caps |
|---|---|---|
| `choices` | `choices: [{label, detail?, value}]`; `value` should be a ticket | 20 choices; label 80, detail 160 chars |
| `open` | `app` (yours), `props?`, `ticket?`, `title`, `placement?` | title 60 chars; props validate; no other plugin's app |
| `token` | `ticket`, `title` | title 60 chars; ticket `^tkt_[0-9A-HJKMNP-TV-Z]{26}$` |
| `error` | `message` | 200 chars |

An attachable scope must answer `token` for a value it offered as a choice:
`rox.attach` and the composer rely on it ([spec] 7.6, 9.3).

## Reserved tools (Rox to your plugin)

`lib/rox-plugin.mjs` implements these; you only need them if you write your
own ([spec] 7.8, 7.9).

| Tool | Input | Answer |
|---|---|---|
| `rox.provision` | `{protocol, plugin, key: {alg, seed}, pub}` | `{ok: true, manifest_sha256}`; idempotent for the same key; first, before any other tool |
| `rox.session.open` | `{app, open_id, ks_pub, perms_sha256, not_after}` | `{cert, cert_sig}`; `invalid_args` for an unknown app, a wrong digest or `not_after` past 8 h |
| `rox.session.close` | `{open_id}` | `{ok: true}` |
| `rox.ticket.resolve` | `{ticket}` | `{valid: true, title}` or `{valid: false}`: never the subject |
| `rox.uninstall` | `{}` | `{ok: true}` after deleting tickets and subjects |
| any other `rox.*` | | MCP error `-32601` |

## Theme

Rox renders your app in Rox's own light or dark setting, not the OS's, and switches it live when the user changes it (spec 6.5). Give every color a `dark:` variant: `bg-white dark:bg-[#161616]`, `text-[#1c1c1c] dark:text-[#f5f5f5]`, `border-black/10 dark:border-white/10`. A color without one stays light in a dark Rox. The template shows the pattern.

## Limits

| Item | Limit |
|---|---|
| `rox.call` args | 256 KiB serialized |
| `rox.call` result | 1 MiB (structured plus text and embedded resources) |
| `rox.preview` document | `permissions.preview.max_bytes`, at most 8 MiB |
| Calls in flight per open | 8 (the ninth waits) |
| New calls per second per open | 20 (more: `rate_limited`) |
| Bridge call timeout | 30 s (host), 120 s (broker to plugin) |
| Resolve timeout | 10 s |
| `rox.provision`, `rox.session.open`, `rox.uninstall` | 5 s each |
| Session certificate | 8 h at most |
| Storage key / value / keys | 128 chars `^[A-Za-z0-9._:-]+$` / 64 KiB / 1000 |
| Link | 2048 chars; props in a link 1024 bytes |
| App source | 512 KiB |
| App load / one JS call / heap / stack | 10 s / 2 s / 256 MB / 4 MB |

[spec]: ../../../../rox/drafts/rox-mini-app-protocol-v1.md
