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
