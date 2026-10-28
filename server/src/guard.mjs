// guard.mjs — who may call the server, checked before any route runs.
//
// The server answers Rox, the Grok UI, mini apps and its own pages. None
// of the native callers send an `Origin` header (nsh's fetch and Node's
// add none); the server's own pages (the office editor) send the server's
// origin. A browser adds `Origin` to every cross-site request, so a
// request whose Origin is not on the list comes from a web page and is
// refused. Bodies must be JSON: a page cannot send `application/json`
// cross-site without a preflight, and no route takes another type.

const BODY_METHODS = new Set(["POST", "PUT", "PATCH", "DELETE"]);

/** The origins allowed to call: the server's own, plus `extra`. */
export function allowedOrigins(port, extra = "") {
  return new Set([
    `http://127.0.0.1:${port}`,
    `http://localhost:${port}`,
    ...String(extra || "").split(",").map((s) => s.trim()).filter(Boolean),
  ]);
}

const hasBody = (req) =>
  req.headers["transfer-encoding"] !== undefined || Number(req.headers["content-length"] || 0) > 0;

const isJson = (type) =>
  String(type || "").split(";")[0].trim().toLowerCase() === "application/json";

/** Why `req` is refused — {status, error} — or null when it may proceed. */
export function refusal(req, allowed) {
  const origin = req.headers.origin;
  if (origin !== undefined && !allowed.has(origin))
    return { status: 403, error: `requests from origin ${String(origin).slice(0, 100)} are refused: this server answers Rox and its apps, not web pages` };
  if (BODY_METHODS.has(req.method) && hasBody(req) && !isJson(req.headers["content-type"]))
    return { status: 415, error: "send the request body as Content-Type: application/json" };
  return null;
}
