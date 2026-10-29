// agents-tools.mjs — the agent tools (drafts/agent-messaging.md §4): in an
// agent's chat and in its delivery turns, never in a routine's run. The
// caller is the bot the turn answers as; `hop` counts relays (§3).
import { AgentError } from "./agents.mjs";
import { agents } from "./agents-wire.mjs";
import { store, threadBot } from "./state.mjs";

const TO = {
  description: 'An agent id or name (from agents_list), a list of them, or "*" for all of them.',
  anyOf: [{ type: "array", items: { type: "string" }, maxItems: 8 }, { type: "string" }],
};

export const AGENT_TOOLS = [
  {
    name: "agents_list",
    description: "The user's other agents you can message: id, name, what each is for (description), and whether it is your temporary helper.",
    input_schema: { type: "object", properties: {} },
  },
  {
    name: "agent_send",
    description:
      "Send a message to other agents. Use it when the user asks you to hand work to another agent (\"ask Boss to …\"), or when an agent's purpose fits the task better. " +
      "With expects_reply, each recipient works on it and its answer comes back to you; you report it in your chat. " +
      "Leave out `conversation` to start one (then `subject` is required); pass it, and `in_reply_to`, to answer in one. " +
      "Attachments are ticket tokens only (@plugin:scope:tkt_…).",
    input_schema: {
      type: "object",
      properties: {
        to: TO,
        subject: { type: "string", description: "What it is about, at most 120 characters. Required for a new conversation." },
        text: { type: "string", description: "The message (Markdown)." },
        data: { type: "object", description: "Structured data for the recipient, at most 64 KB." },
        attachments: { type: "array", items: { type: "string" }, maxItems: 16 },
        conversation: { type: "string" },
        in_reply_to: { type: "string" },
        expects_reply: { type: "boolean" },
        deadline: { type: "string", description: "RFC 3339: past it an undelivered message is dropped." },
      },
      required: ["text"],
    },
  },
  {
    name: "agent_conversations",
    description: "Your recent conversations with other agents: subject, participants, last message time, message count.",
    input_schema: { type: "object", properties: { with: { type: "string", description: "Only those with this agent id." }, limit: { type: "integer" } } },
  },
  {
    name: "agent_read",
    description: "The messages of one of your conversations with other agents, oldest first.",
    input_schema: { type: "object", properties: { conversation: { type: "string" }, since: { type: "string", description: "A message id or a time." } }, required: ["conversation"] },
  },
  {
    name: "agent_create",
    description:
      "Create an agent. temporary: false when the user asked for a new agent (\"assign a new agent to …\"): it is listed with the user's agents and only the user removes it. " +
      "temporary: true when you decide you need a helper for a task: it is not listed, you may have one at a time, and you remove it with agent_remove when the task ends. " +
      "It gets the model the user named, else yours, and at most your plugins. first_message hands it the task.",
    input_schema: {
      type: "object",
      properties: {
        name: { type: "string" },
        purpose: { type: "string", description: "What it is for; shown in agents_list so later requests find it." },
        temporary: { type: "boolean" },
        instructions: { type: "string" },
        model: { type: "string", description: "Only the model the user named." },
        plugins: { type: "array", items: { type: "string" }, description: "A subset of your plugins; all of yours when left out." },
        first_message: { type: "string" },
      },
      required: ["name", "purpose", "temporary"],
    },
  },
  {
    name: "agent_remove",
    description: "End a temporary helper you created, when its task is done. Its chat stays readable under Past helpers.",
    input_schema: { type: "object", properties: { id: { type: "string" } }, required: ["id"] },
  },
];

const NAMES = new Set(AGENT_TOOLS.map((t) => t.name));

export const isAgentTool = (name) => NAMES.has(name);

/** The agent tools a turn gets: an agent's turns only, never a routine's. */
export const agentToolsFor = (thread, job) => (job?.routine || job?.v1Surface || !(job?.agentBot || threadBot(thread?.id)) ? [] : AGENT_TOOLS);

/** Runs one agent tool for the turn's agent. */
export function toolAgents(thread, name, input = {}, job = null) {
  const caller = job?.agentBot ? store.bots.find((b) => b.id === job.agentBot) : threadBot(thread?.id);

  if (!caller || job?.routine || job?.v1Surface) return { content: `${name} works only in an agent's own turns`, is_error: true };
  const hop = job?.agentHop != null ? job.agentHop + 1 : 0;

  try {
    if (name === "agents_list") return { content: JSON.stringify(agents.list(caller)) };

    if (name === "agent_send") return { content: JSON.stringify(agents.send(caller, input, { hop })) };

    if (name === "agent_conversations") return { content: JSON.stringify(agents.conversations(caller, input)) };

    if (name === "agent_read") return { content: JSON.stringify(agents.read(caller, input)) };

    // another agent's request does not make or end the user's agents
    if (job?.agentBot) return { content: `${name} is not available while answering another agent`, is_error: true };

    if (name === "agent_create") return { content: JSON.stringify(agents.create(caller, input, { hop })) };

    if (name === "agent_remove") return { content: JSON.stringify(agents.remove(caller, input)) };
  } catch (e) {
    if (e instanceof AgentError) return { content: `${e.code}: ${e.message}`, is_error: true };
    throw e;
  }

  return { content: `unknown tool ${name}`, is_error: true };
}
