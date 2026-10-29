// v1/sessions-line.mjs — app opens and the calls they make, held by
// rox-broker (Rust broker plan 3.5): sessions.mjs's interface over the line
// (broker/rox-broker/src/helper/sessions.rs). A call's result goes through
// the MCP SDK's schema here, as plugins-line.mjs's tool calls do, so Rox
// gets what the in-process sessions gave it.
import { CallToolResultSchema } from "@modelcontextprotocol/sdk/types.js";

/** Sessions of `plugins`, a createLinePlugins() on the same line. */
export function createLineSessions(plugins) {
  const post = plugins.line;

  return {
    open: (b) => post("sessions/open", b),
    close: (b) => post("sessions/close", b),
    call: async (b) => {
      const r = await post("sessions/call", b);

      return { ...r, result: CallToolResultSchema.parse(r.result) };
    },
    ticketTitle: (b) => post("tickets/title", b),
    resolve: (b) => post("resolve", b),
    appSource: (b) => post("apps/source", b),
    stat: async (openId) => (await post("sessions/stat", { open_id: openId })).open,
  };
}
