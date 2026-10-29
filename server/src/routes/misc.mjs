// routes/misc.mjs — extracted from app-server.mjs
import { MODEL, MOCK } from "../config.mjs";
import { store } from "../state.mjs";

export default async function handle(req, res) {
  if (req.method === "GET" && (req.url === "/" || req.url === "/health")) {
    res.setHeader("content-type", "text/plain");
    return res.end(
      `app-server ok — ${store.list.length} threads, model=${MOCK ? "mock" : MODEL}\n`,
    );
  }
  return false;
}
