// grammar.mjs — the one grammar for plugin names and `@` tokens.
//
//   @<server>:<scope>:<value>
//     server  [a-z0-9-]+       a connected server's id (mcp_servers.id)
//     scope   [A-Za-z0-9_-]+   a scope the server's manifest announces
//     value   \S+              non-empty, no whitespace; may hold `:` and `/`
//
// Command names a manifest announces (`/name`) follow the scope grammar.
// Every parser of these tokens and names on the server goes through here.

const SERVER = "[a-z0-9-]+";
const NAME = "[A-Za-z0-9_-]+";
const TOKEN = `@(${SERVER}):(${NAME}):(\\S+)`;

// Checks on the text form; callers parse their input to strings first.
export const isServerId = (s) => new RegExp(`^${SERVER}$`).test(String(s ?? ""));
export const isName = (s) => new RegExp(`^${NAME}$`).test(String(s ?? ""));

/** A whole string as one token: {server, scope, value}, or null. */
export function parseToken(s) {
  const m = new RegExp(`^${TOKEN}$`).exec(String(s ?? "").trim());

  return m ? { server: m[1], scope: m[2], value: m[3] } : null;
}

/** Every token in free text (a chat message), in order. */
export function tokensIn(text) {
  return [...String(text ?? "").matchAll(new RegExp(TOKEN, "g"))].map((m) => ({
    server: m[1],
    scope: m[2],
    value: m[3],
  }));
}

// ── Mini App Protocol v1 (spec section 3) ────────────────────────────────
// The v1 names, as the spec writes them: ECMAScript sources over the whole
// string. The broker, the fixture plugin and the tests read this one table
// (Rox carries the same strings), so host and plugin cannot drift apart.
// The v0 grammar above stays for v0 servers.
export const V1 = Object.freeze({
  pluginId: "^[a-z][a-z0-9-]{0,62}(\\.[a-z][a-z0-9-]{0,62}){2,7}$",
  short: "^[a-z][a-z0-9]{1,15}$",
  appId: "^[a-z][a-z0-9-]{0,47}$",
  command: "^[a-z][a-z0-9-]{0,31}$",
  scope: "^[a-z][a-z0-9-]{0,31}$",
  routineId: "^[a-z][a-z0-9-]{0,47}$",
  tool: "^[a-z][a-z0-9_]{0,62}$",
  propKey: "^[a-zA-Z][a-zA-Z0-9_]{0,31}$",
  ticket: "^tkt_[0-9A-HJKMNP-TV-Z]{26}$",
  canonicalToken: "^@([a-z][a-z0-9]{1,15}):([a-z][a-z0-9-]{0,31}):(tkt_[0-9A-HJKMNP-TV-Z]{26})$",
  typedToken: "(?<![\\w@.])@([a-z][a-z0-9]{1,15}):([a-z][a-z0-9-]{0,31}):(\\S*)",
  slash: "^/(?:([a-z][a-z0-9]{1,15}):)?([a-z][a-z0-9-]{0,31})(?:\\s+(.*))?$",
  reservedTool: "^rox\\.[a-z.]+$",
  publisherId: "^[a-z][a-z0-9-]{1,62}(\\.[a-z0-9-]{1,62})*$",
  keyId: "^[a-z0-9][a-z0-9-]{0,63}$",
});

// Names no plugin may take: reserved id prefixes, shorts and the built-in
// slash commands (3, 3.1).
export const V1_RESERVED = Object.freeze({
  idPrefixes: ["dev.goldcoders.rox.", "rox."],
  shorts: ["rox", "app", "chat", "mcp", "nsh", "all"],
  commands: ["status"],
});

const v1Re = Object.fromEntries(Object.entries(V1).map(([k, src]) => [k, new RegExp(src)]));

/** Whether `s` is a whole `kind` name of the v1 table (V1's keys). */
export const isV1 = (kind, s) => v1Re[kind].test(String(s ?? "")) && (kind !== "pluginId" || String(s).length <= 255);

/** Every typed token in composer text: {short, scope, value}, in order. */
export function v1TypedTokens(text) {
  return [...String(text ?? "").matchAll(new RegExp(V1.typedToken, "g"))].map((m) => ({ short: m[1], scope: m[2], value: m[3] }));
}

/** A canonical token `@short:scope:tkt_…` as {short, scope, ticket}, or null. */
export function v1Canonical(s) {
  const m = v1Re.canonicalToken.exec(String(s ?? ""));

  return m ? { short: m[1], scope: m[2], ticket: m[3] } : null;
}
