# Security checklist for plugin authors

Rox guarantees H1 to H12 of [spec] 15.1: an app reaches only the bridge, every
call is signed and checked, consent is asked where [spec] 9.7 says. Your
plugin owns the rest: P1 to P10 of [spec] 15.2. Tick each before release.

- [ ] **P1 Authorize every resource, not only the app.** The broker has
  checked that the app may call the tool. It has not checked that the caller
  may use *this* ticket. Look the ticket up in your own store and refuse what
  you did not issue or what this user may not see.

  ```js
  function greet(args, caller) {
    const subject = tickets.subject(args.ticket);

    if (subject === undefined) return fail("not_found", "That item is no longer available.");
    if (caller?.origin === "app" && !mayUse(subject)) return fail("permission_denied", "Not yours.");

    return ok({ greeting: `Hello, ${subject.name}` });
  }
  ```

- [ ] **P2 Tickets are random; the subject map stays with you.** Use
  `tickets.issue(subject)` from `lib/rox-plugin.mjs` (130 random bits,
  `tkt_` + 26 Crockford base32). Never encode a TIN, an account number or a
  row id into a ticket, a link or props ([spec] 6.6, 9.4).

  ```js
  const t = tickets.issue(taxpayer.tin); // "tkt_7Q…", meaningless outside this process
  return ok({ result: "token", ticket: t, title: taxpayer.name });
  ```

- [ ] **P3 `rox.ticket.resolve` returns a title only.** `startPlugin` answers
  `{valid, title}` with your `ticketTitle(subject)`; return a display name,
  not the identifier.

  ```js
  startPlugin({ /* … */ ticketTitle: (s) => s.displayName });
  ```

- [ ] **P4 Never log the plugin key, a subject or `rox.provision`'s content.**
  The broker scans your stderr for the seed and stops a plugin that prints it
  ([spec] 4.3). Log tool names and outcomes, not arguments.

  ```js
  console.error(`hello_greet: ${result.isError ? "refused" : "ok"}`); // not JSON.stringify(args)
  ```

- [ ] **P5 Side effects need an explicit argument the model cannot supply
  alone.** Filing, sending and paying take a confirmation token that only the
  app's own UI produces (BIR's "send grant" pattern).

  ```js
  if (args.confirm !== pendingGrant.get(args.draft)) return fail("permission_denied", "Confirm in the app first.");
  ```

- [ ] **P6 No side effects for `origin: resolve` or `origin: routine`.**
  Resolve tools and routine queries read; they never write.

  ```js
  if (["resolve", "routine"].includes(caller?.origin)) return fail("permission_denied", "Read-only here.");
  ```

- [ ] **P7 `rox.provision` first; refuse every other tool before it.**
  `startPlugin` does this; a hand-written plugin must too ([spec] 7.8).

- [ ] **P8 Mark read-only tools `readOnlyHint: true`, and only those.** A
  routine may read another plugin's tool only when it is read-only ([spec] 5.9).

  ```js
  { name: "hello_greet", annotations: { readOnlyHint: true }, run: greet }
  ```

- [ ] **P9 Keep AI text within its caps and never tell the model to skip
  consent.** Tool descriptions 1024 chars, hints 1500, routine prompts and
  server instructions 4000 ([spec] 10.2).

- [ ] **P10 Do not read the broker's or Rox's files or environment.** Your
  process gets `PATH` and the env of its install record, nothing else; leave
  other plugins' data alone.

## Also

- Tickets in props, links and `_meta["rox/open"]`, never raw identifiers
  ([spec] 6.6, 9.4).
- `fail(code, message)`: the message is what the user may see; keep it short
  and free of subject data.
- The runner (`npm run plugin-test`) checks P3, P4 and P7 mechanically; P1,
  P2, P5, P6, P8 to P10 are review items.

[spec]: ../../../../rox/drafts/rox-mini-app-protocol-v1.md
