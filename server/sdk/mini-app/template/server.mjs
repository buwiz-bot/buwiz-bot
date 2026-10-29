#!/usr/bin/env node
// sdk/mini-app/template/server.mjs — the smallest correct v1 plugin.
//
// Copy this directory, rename the id and short in manifest.json, and
// replace the tools and the app (README.md, steps 1-10). The reserved
// tools of spec 7.8 (provision first, the key in memory only, session
// certificates, ticket titles, uninstall, -32601 for unknown rox.*) live
// in ../lib/rox-plugin.mjs; this file holds only what is yours.
//
// The signed manifest is read from ROX_PLUGIN_MANIFEST, else manifest.json
// beside this file. App sources sit beside it too.
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";
import { fail, ok, startPlugin, ticketStore } from "../lib/rox-plugin.mjs";

const HERE = dirname(fileURLToPath(import.meta.url));
const tickets = ticketStore();

// The subjects this plugin knows. A real plugin looks them up in its own
// store; tickets stand for them everywhere outside this process (9.5).
const FRIENDS = ["Ada", "Grace", "Linus"];

// The resolve tool (9.2): `@hello:friend:<name>` and the /hello command.
function resolve(args) {
  const value = String(args.value ?? "").trim();
  const matches = FRIENDS.filter((f) => f.toLowerCase().startsWith(value.toLowerCase()));

  if (args.kind === "command") {
    const props = matches.length === 1 && value ? { ticket: tickets.issue(matches[0]) } : {};

    return ok({ result: "open", app: "hello", props, title: "Hello", placement: "full" });
  }

  if (args.complete) return ok({ result: "choices", choices: matches.slice(0, 20).map((f) => ({ label: f, detail: "A friend", value: tickets.issue(f) })) });

  // a value may be a ticket already (a chosen completion) or typed text
  const subject = tickets.subject(value) ?? matches.find((f) => f.toLowerCase() === value.toLowerCase());

  if (!subject) return ok({ result: "error", message: `No friend called ${value.slice(0, 60)}.` });

  return ok({ result: "token", ticket: tickets.issue(subject), title: subject });
}

// The one tool: a greeting for a ticket, or for everyone. Read only (P8).
// P1: a ticket this plugin did not issue is refused, not guessed at.
function greet(args) {
  if (args.ticket === undefined) return ok({ greeting: "Hello, world" });
  const name = tickets.subject(args.ticket);

  if (name === undefined) return fail("not_found", "That friend is no longer available.");

  return ok({ greeting: `Hello, ${name}` });
}

await startPlugin({
  name: "hello",
  manifestFile: process.env.ROX_PLUGIN_MANIFEST || join(HERE, "manifest.json"),
  apps: { hello: join(HERE, "hello.tsx") },
  tickets,
  ticketTitle: (name) => name,
  tools: [
    {
      name: "hello_greet",
      description: "Greet a friend by ticket, or everyone.",
      inputSchema: { type: "object", properties: { ticket: { type: "string" } } },
      annotations: { readOnlyHint: true },
      run: greet,
    },
    {
      name: "hello_resolve",
      description: "Resolve /hello and @hello:friend tokens.",
      inputSchema: { type: "object" },
      annotations: { readOnlyHint: true },
      run: resolve,
    },
  ],
});
