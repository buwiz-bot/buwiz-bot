// routes/groups.mjs — the sidebar's user-made groups (groups.mjs): list
// with resolved members, create, rename / recolor / fold / move, delete,
// and add / remove / reorder a member.
import { originOf } from "../appmeta.mjs";
import { deleteGroup, groupItem, listGroups, newGroup, updateGroup } from "../groups.mjs";

// POST bodies reply {ok, …} — 400 with the error when a call is refused.
const POSTS = {
  "/group/new": (j) => ({ id: newGroup(j) }),
  "/group/update": (j) => updateGroup(j),
  "/group/delete": (j) => deleteGroup(j),
  "/group/item": (j) => groupItem(j),
};

export default async function handle(req, res) {
  // GET /groups — {groups: [{id, name, color, order, collapsed, items}],
  // colors}; items are {kind, ref, title, hidden?, thread?, artifact?}.
  if (req.method === "GET" && req.url.split("?")[0] === "/groups") {
    res.setHeader("content-type", "application/json");
    return res.end(JSON.stringify(await listGroups(originOf(req))));
  }
  const post = req.method === "POST" && POSTS[req.url];
  if (!post) return false;
  let body = "";
  req.on("data", (c) => (body += c));
  req.on("end", () => {
    res.setHeader("content-type", "application/json");
    try {
      res.end(JSON.stringify({ ok: true, ...post(JSON.parse(body || "{}")) }));
    } catch (e) {
      res.statusCode = 400;
      res.end(JSON.stringify({ ok: false, error: String(e?.message || e) }));
    }
  });
  return true;
}
