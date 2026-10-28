// A node:test reporter for `npm run conformance`: after the spec reporter's
// output, the spec 17 checklist this suite covers, one line per ID with
// pass or FAIL and the tests that cover it. A test covers an ID when its
// name starts with it (e.g. "BRG-04 the same envelope twice is replayed").
const ID = /^([A-Z]{3}-\d{2})\b/;

export default async function* checklist(source) {
  const ids = new Map(); // id → {ok, names}

  for await (const ev of source) {
    if (ev.type !== "test:pass" && ev.type !== "test:fail") continue;
    const id = ID.exec(ev.data.name)?.[1];

    if (!id) continue;
    const row = ids.get(id) || { ok: true, names: [] };

    row.ok &&= ev.type === "test:pass";
    row.names.push(ev.data.name);
    ids.set(id, row);
  }

  const lines = [...ids.keys()].sort().map((id) => {
    const { ok, names } = ids.get(id);

    return `  [${ok ? "x" : " "}] ${id} ${ok ? "pass" : "FAIL"}\n${names.map((n) => `        ${n}`).join("\n")}`;
  });

  yield `\nRox Mini App Protocol v1, spec 17 (broker side): ${ids.size} IDs\n${lines.join("\n")}\n`;
}
