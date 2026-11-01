// Buwiz Bot's environment variables by their Buwiz names, the old Rox and
// Grok names still read: the same table as rox-v1's `env` module
// (broker/rox-v1/src/env.rs; a test there checks they match).
//
// `env(name)` takes the new name when it has a value, else the old one when
// it is set (an empty new name gives way to a non-empty old one), and then
// says once on stderr "<old> is an old name; use <new>".
// Whoever sets a variable for a child that may be an older build sets both
// names (`withOld`, `both`).

/** Every renamed variable: [new name, old name]. */
export const NAMES = Object.freeze([
  ["BUWIZ_BACKEND", "ROX_BACKEND"],
  ["BUWIZ_BACKGROUND", "ROX_BACKGROUND"],
  ["BUWIZ_DATA_DIR", "ROX_DATA_DIR"],
  ["BUWIZ_HOST_TOKEN_FILE", "ROX_HOST_TOKEN_FILE"],
  ["BUWIZ_JANK_LOG", "ROX_JANK_LOG"],
  ["BUWIZ_FPS", "ROX_FPS"],
  ["BUWIZ_MACHINE_ID", "ROX_MACHINE_ID"],
  ["BUWIZ_MINI_CACHE", "ROX_MINI_CACHE"],
  ["BUWIZ_OFFICE_FONTS", "ROX_OFFICE_FONTS"],
  ["BUWIZ_STATUS", "ROX_STATUS"],
  ["BUWIZ_PANE", "ROX_PANE"],
  ["BUWIZ_PLUGINS", "ROX_PLUGINS"],
  ["BUWIZ_MODELS", "ROX_MODELS"],
  ["BUWIZ_SIDE", "ROX_SIDE"],
  ["BUWIZ_ACCOUNT", "ROX_ACCOUNT"],
  ["BUWIZ_TAB", "ROX_TAB"],
  ["BUWIZ_AVATAR", "ROX_AVATAR"],
  ["BUWIZ_OPEN", "ROX_OPEN"],
  ["BUWIZ_SCROLL", "ROX_SCROLL"],
  ["BUWIZ_SEND", "ROX_SEND"],
  ["BUWIZ_PALETTE", "ROX_PALETTE"],
  ["BUWIZ_SLOTS", "ROX_SLOTS"],
  ["BUWIZ_MINI", "ROX_MINI"],
  ["BUWIZ_CARD", "ROX_CARD"],
  ["BUWIZ_GAZE", "ROX_GAZE"],
  ["BUWIZ_PILL", "ROX_PILL"],
  ["BUWIZ_OVERLAY", "ROX_OVERLAY"],
  ["BUWIZ_BROKER_HUB", "ROX_BROKER_HUB"],
  ["BUWIZ_BROKER_V1_HOST", "ROX_BROKER_V1_HOST"],
  ["BUWIZ_BROKER_MCP_HOST", "ROX_BROKER_MCP_HOST"],
  ["BUWIZ_BROKER_NO_NODE", "ROX_BROKER_NO_NODE"],
  ["BUWIZ_BROKER_FORWARD_ALL", "ROX_BROKER_FORWARD_ALL"],
  ["BUWIZ_BROKER_DATA_DIR", "GROK_DATA_DIR"],
  ["BUWIZ_ALLOWED_ORIGINS", "GROK_ALLOWED_ORIGINS"],
  ["BUWIZ_BUNDLE_ID", "GROK_BUNDLE_ID"],
  ["BUWIZ_PUBLIC_PORT", "ROX_PUBLIC_PORT"],
  ["BUWIZ_NODE_ROLE", "ROX_NODE_ROLE"],
  ["BUWIZ_PLUGIN_CATALOG", "ROX_PLUGIN_CATALOG"],
  ["BUWIZ_V1_LINE", "ROX_V1_LINE"],
  ["BUWIZ_V1_LINE_TOKEN", "ROX_V1_LINE_TOKEN"],
  ["BUWIZ_LINE_HOSTS", "ROX_LINE_HOSTS"],
  ["BUWIZ_LINE_TOKEN", "ROX_LINE_TOKEN"],
  ["BUWIZ_V1_CALL_TIMEOUT_MS", "ROX_V1_CALL_TIMEOUT_MS"],
  ["BUWIZ_V1_MANUAL_CLOCK", "ROX_V1_MANUAL_CLOCK"],
  ["BUWIZ_BRIDGE_URL", "ROX_BRIDGE_URL"],
  ["BUWIZ_BRIDGE_TOKEN", "ROX_BRIDGE_TOKEN"],
  ["BUWIZ_STUB_LOG", "ROX_STUB_LOG"],
  ["BUWIZ_DEFS_WRITE", "ROX_DEFS_WRITE"],
  ["BUWIZ_APP", "ROX_APP"],
  ["BUWIZ_LOG", "ROX_LOG"],
  ["BUWIZ_BUILD_LOCK", "ROX_BUILD_LOCK"],
  ["BUWIZ_BUILD_HOLD", "ROX_BUILD_HOLD"],
  ["BUWIZ_BOT_BIN", "ROX_BIN"],
  ["BUWIZ_BROKER_BIN", "ROX_BROKER_BIN"],
]);

const OLD = new Map(NAMES);
const warned = new Set();

/** The old name of `name`, when it had one. */
export const oldName = (name) => OLD.get(name);

/** `name`, then its old name when it had one. */
export const withOld = (name) => (OLD.has(name) ? [name, OLD.get(name)] : [name]);

/** `{[name]: value, [old name]: value}`: for a child's environment. */
export const both = (name, value) => Object.fromEntries(withOld(name).map((n) => [n, value]));

/** Says once per old name that it is old; true when it said it now. */
export function warnOnce(old, name) {
  if (warned.has(old)) return false;
  warned.add(old);
  process.stderr.write(`${old} is an old name; use ${name}\n`);
  return true;
}

/** `name`'s value in `from` (process.env), else its old name's (with the
 *  warning); undefined when neither is set. */
export function env(name, from = process.env) {
  const v = from[name];
  if (v) return v;
  // unset, or empty while the old name has a value: the old name's
  const old = OLD.get(name);
  const o = old === undefined ? undefined : from[old];
  if (o === undefined || (v !== undefined && o === "")) return v;
  warnOnce(old, name);
  return o;
}
