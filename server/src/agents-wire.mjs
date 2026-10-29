// agents-wire.mjs — the server's one agents broker: agents.mjs on the live
// store, delivered by agents-run.mjs through agent.mjs. A request runs as
// the recipient (actingBot) on its own picker's model, with the
// conversation as its history; a reply wakes the asker in its own chat.
// app-server calls startAgents() once it listens, to hand over what a
// restart left undelivered.
import { MOCK, PROVIDER } from "./config.mjs";
import { createAgents } from "./agents.mjs";
import { createDelivery, historyFor, peerPrompt } from "./agents-run.mjs";
import { actingBot, allocJobId, botDefaults, findThread, modelCtx, nextThreadSeq, store, uuidv7, writeIndex } from "./state.mjs";

const jobFor = (thread, msg, bot) => ({
  id: allocJobId(), thread, seq: 0, phase: "queued", detail: "", done: false, result: null, extra: {},
  // what agent_send reads: who is sending, and how many relays deep
  agentBot: bot.id, agentHop: msg.hop,
});

// agent.mjs and model.mjs import the tools, which import this module:
// hence the late imports
async function runRequest(bot, msg, t) {
  const [{ agentTurn }, { chatReply, choiceFor }] = await Promise.all([import("./agent.mjs"), import("./model.mjs")]);
  const job = jobFor(t.id, msg, bot);
  // a stand-in for the thread: the conversation from this agent's side;
  // what the turn writes into it is the turn's own, never shown
  const history = { id: t.id, uid: t.uid, title: t.title, turns: historyFor(bot, t, msg) };
  const prompt = peerPrompt(msg);

  return actingBot.run(bot, () =>
    modelCtx.run(choiceFor(bot.threadId), () => (!MOCK && PROVIDER === "anthropic" ? agentTurn(job, prompt, history) : chatReply(prompt, history, job))),
  );
}

async function runReply(bot, msg, t) {
  const chat = findThread(bot.threadId);

  if (!chat) return;
  const [{ backgroundTurn }, { choiceFor }] = await Promise.all([import("./agent.mjs"), import("./model.mjs")]);
  const job = { ...jobFor(chat.id, msg, bot), agentBot: undefined };
  const mark = { conversation: msg.conversation, message: msg.id, from: msg.from, thread: t.id, thread_uid: t.uid };

  await modelCtx.run(choiceFor(chat.id), () => backgroundTurn(job, chat, `Reply from ${msg.from.name} · ${msg.subject}`, peerPrompt(msg, { replying: true }), { agent: mark }));
}

let delivery;

export const agents = createAgents({
  store,
  nextId: () => `t${nextThreadSeq()}`,
  uid: uuidv7,
  save: writeIndex,
  defaults: botDefaults,
  deliver: (bot, msg, t) => delivery.deliver(bot, msg, t),
});

delivery = createDelivery({ agents, runRequest, runReply, log: (line) => console.log(line) });

/** Hands what a restart left undelivered to its recipients again. */
export function startAgents() {
  for (const { bot, msg, t } of agents.undelivered()) delivery.deliver(bot, msg, t);
}
