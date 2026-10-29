// agents-wire.mjs — the server's one agents broker: agents.mjs on the live
// store, delivered by agents-run.mjs through agent.mjs. A request runs as
// the recipient (actingBot) on its own picker's model, with the
// conversation as its history; a reply wakes the asker in its own chat.
// app-server calls startAgents() once it listens, to hand over what a
// restart left undelivered.
import { MOCK, PROVIDER } from "./config.mjs";
import { createAgents } from "./agents.mjs";
import { createExternals } from "./agents-external.mjs";
import { createDelivery, historyFor, peerPrompt } from "./agents-run.mjs";
import { actingBot, allocJobId, botDefaults, db, findThread, jobs, modelCtx, nextThreadSeq, store, uuidv7, writeIndex } from "./state.mjs";

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
  const answer = await actingBot.run(bot, () =>
    modelCtx.run(choiceFor(bot.threadId), () => (!MOCK && PROVIDER === "anthropic" ? agentTurn(job, prompt, history) : chatReply(prompt, history, job))),
  );

  cardsToChat(bot, msg, t, job);

  return answer;
}

// A card the hidden turn made (an approval, a consent, an app) goes to the
// agent's own chat, where the user can press it; the read-only
// conversation says where it waits (spec 7, A2A-08).
export function cardsToChat(bot, msg, t, job) {
  const links = job.linkTurn?.links || [];
  const chat = findThread(bot.threadId);

  if (!links.length || !chat) return;
  chat.turns.push({ id: uuidv7(), role: "ai", text: `For ${msg.from.name}'s request · ${msg.subject}`, label: bot.name, links, agent: { conversation: msg.conversation, message: msg.id, from: msg.from, thread: t.id, thread_uid: t.uid } });
  t.turns.push({ id: uuidv7(), role: "notice", text: `Waiting for your OK in ${bot.name}'s chat`, notice: { kind: "agents_waiting", bot: bot.id, thread: chat.id, thread_uid: chat.uid, at: Date.now() } });
  writeIndex();
}

// A reply wakes the asker once its chat is idle (spec 5.3), unless the
// turn that asked read the answer already and told the user.
async function runReply(bot, msg, t) {
  const chat = findThread(bot.threadId);

  if (!chat) return;
  const busy = () => [...jobs.values()].some((j) => j.thread === chat.id && !j.done);

  for (let i = 0; i < 600 && busy(); i++) await new Promise((r) => setTimeout(r, 1000));

  if (agents.readBy(msg.id, bot.id)) return;
  const [{ backgroundTurn }, { choiceFor }] = await Promise.all([import("./agent.mjs"), import("./model.mjs")]);
  const job = { ...jobFor(chat.id, msg, bot), agentBot: undefined };
  const mark = { conversation: msg.conversation, message: msg.id, from: msg.from, thread: t.id, thread_uid: t.uid };

  await modelCtx.run(choiceFor(chat.id), () => backgroundTurn(job, chat, `Reply from ${msg.from.name} · ${msg.subject}`, peerPrompt(msg, { replying: true }), { agent: mark }));
}

let delivery, externals;

export const agents = createAgents({
  store,
  nextId: () => `t${nextThreadSeq()}`,
  uid: uuidv7,
  save: writeIndex,
  defaults: botDefaults,
  externals: { get: (id) => externals.get(id) },
  deliver: (to, msg, t) => route(to, msg, t),
});

// an outside endpoint's message goes out (or is held) at once; an agent's
// waits its turn
function route(to, msg, t) {
  if (!to.external) return delivery.deliver(to, msg, t);
  externals.deliver(to, msg);
  agents.delivered(t, msg.id, to.id);
}

delivery = createDelivery({ agents, runRequest, runReply, log: (line) => console.log(line) });
externals = createExternals({ db, agents, save: writeIndex, log: (line) => console.log(line) });

export { externals };

/** Hands what a restart left undelivered to its recipients again, and
 *  retries the outgoing webhooks still queued. */
export function startAgents() {
  for (const { bot, msg, t } of agents.undelivered()) route(bot, msg, t);
  externals.resume();
}
