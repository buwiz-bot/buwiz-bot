// The two names the bridge reads, by their Buwiz names with the old Rox names
// still read (the table is broker/buwiz-protocol/src/env.rs's).
//
// `env(name)` takes the new name when it has a value, else the old one when
// it is set, and then says once on stderr "<old> is an old name; use <new>".
const OLD = new Map([
  ["BUWIZ_BRIDGE_URL", "ROX_BRIDGE_URL"],
  ["BUWIZ_BRIDGE_TOKEN", "ROX_BRIDGE_TOKEN"],
]);
const warned = new Set();

export function env(name, from = process.env) {
  const v = from[name];
  if (v) return v;
  // unset, or empty while the old name has a value: the old name's
  const old = OLD.get(name);
  const o = old === undefined ? undefined : from[old];
  if (o === undefined || (v !== undefined && o === "")) return v;
  if (!warned.has(old)) {
    warned.add(old);
    process.stderr.write(`${old} is an old name; use ${name}\n`);
  }
  return o;
}
