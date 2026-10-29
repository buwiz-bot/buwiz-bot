// agents.mjs — the user's agents messaging each other (drafts/agent-
// messaging.md): the v1 envelope, agents' threads (a thread marked
// `agents: {conversation, participants, subject, created_by}`, read-only
// and never a sidebar row), one centered notice per conversation in each
// participant's chat, who may message whom, the limits, and the agents an
// agent makes (permanent, or one temporary helper). Delivery — a
// recipient's model turn — is the caller's `deliver` (agents-run.mjs);
// this module decides and records. It works on the store's shape
// ({list, bots}) so the tests can hand it a plain object.

export const LIMITS = {
  recipients: 8,
  text: 16 * 1024,
  data: 64 * 1024,
  subject: 120,
  attachments: 16,
  hop: 6,
  sends: 30, // per agent per 10 minutes
  perHour: 20, // per conversation without a human, then paused
  mergeMs: 10 * 60_000,
};

// A v1 token naming a ticket: what an attachment may be (spec 3). A raw
// TIN or other identifier never rides a message.
const TICKET = /^@[a-z0-9-]+:[a-z0-9_-]+:tkt_[A-Za-z0-9_-]+$/;

export class AgentError extends Error {
  constructor(code, message) {
    super(message);
    this.code = code;
  }
}

const fail = (code, message) => {
  throw new AgentError(code, message);
};
const clip = (s, n) => String(s ?? "").trim().slice(0, n);
const bytes = (s) => Buffer.byteLength(s, "utf8");

/** A notice's one line. */
export function agentsNoticeText(n) {
  if (n.kind === "agent_created")
    return n.temporary ? `${n.parent} started a helper: ${n.name} · ${n.task}` : `${n.parent} made a new agent: ${n.name}`;

  if (n.kind === "helper_finished") return `Helper ${n.name} finished`;

  if (n.kind === "agents_paused") return `Paused: these agents have exchanged ${LIMITS.perHour} messages in an hour`;

  const to = n.to.length === 1 ? n.to[0].name : `${n.to.length} agents`;

  return `${n.from.name} → ${to} · ${n.subject}${n.count > 1 ? ` · ${n.count} messages` : ""}`;
}

// `externals.get(id)` names an outside endpoint (agents-external.mjs: a
// webhook or a Claude Code bridge) as {id, name, bot}: the agent it belongs
// to. In a conversation it is the participant `ext:<id>`.
export function createAgents({ store, nextId, uid, save, now = () => Date.now(), deliver = () => {}, defaults = () => ({}), externals = { get: () => null } }) {
  const sent = new Map(); // sender id → send times, for the rate limit
  const bot = (id) => store.bots.find((b) => b.id === id);
  /** A participant: a bot, or an outside endpoint as {id: "ext:…", name, external, owner}. */
  const party = (id) => {
    if (!String(id).startsWith("ext:")) return bot(id);
    const ep = externals.get(String(id).slice(4));

    return ep ? { id: `ext:${ep.id}`, name: ep.name, external: true, owner: ep.bot } : undefined;
  };
  const live = (b) => !!b && !b.helper?.ended_at;
  const thread = (key) => store.list.find((t) => t.id === key || t.uid === key);
  const conversation = (conv) => store.list.find((t) => t.agents?.conversation === conv);

  /** Whether `to` takes messages from `from` (spec 8). */
  function accepts(to, from) {
    const a = to.accepts ?? "all";

    if (a === "none") return false;

    return Array.isArray(a) ? a.includes(from.id) : true;
  }

  /** The agents `caller` may message: live, not itself, taking messages
   *  from it; a temporary helper only to its parent. */
  function reachable(caller) {
    // an outside sender reaches the one agent its endpoint belongs to
    if (caller.external) return store.bots.filter((b) => b.id === caller.owner && live(b) && accepts(b, caller));

    return store.bots.filter((b) => b.id !== caller.id && live(b) && accepts(b, caller) && (!b.helper || b.helper.parent === caller.id));
  }

  /** `agents_list {}`. */
  function list(caller) {
    return reachable(caller).map((b) => ({
      id: b.id,
      name: b.name,
      description: b.purpose || b.description || "",
      accepts_messages: true,
      ...(b.helper ? { helper: true } : {}),
      ...(b.made_by ? { made_by: b.made_by } : {}),
    }));
  }

  /** One recipient as `to` names it: an id, a name, or {id}. */
  function recipient(caller, r) {
    const key = typeof r === "string" ? r : r?.kind === "external" && r?.id && !String(r.id).startsWith("ext:") ? `ext:${r.id}` : r?.id;

    if (typeof key !== "string" || !key.trim()) fail("invalid_args", "each of `to` is an agent id or name");

    // an agent writes to an outside endpoint of its own
    if (key.startsWith("ext:")) {
      const ep = party(key);

      if (!ep) fail("invalid_args", `no outside endpoint ${clip(key, 40)}`);

      if (ep.owner !== caller.id) fail("not_allowed", `${ep.name} is not one of your outside endpoints`);

      return ep;
    }
    const k = key.trim().toLowerCase();
    const b = store.bots.find((x) => x.id === key) || store.bots.find((x) => live(x) && x.name.toLowerCase() === k);

    if (!b) fail("invalid_args", `no agent ${clip(key, 40)}`);

    if (b.id === caller.id) fail("invalid_args", "an agent cannot message itself");

    if (!reachable(caller).includes(b)) fail("not_allowed", `${b.name} does not take messages from you`);

    return b;
  }

  /** The conversation's notice in `b`'s chat: a new line, or the last one
   *  updated in place within the merge window (spec 6). */
  function noticeIn(b, t, msg) {
    const chat = b && !b.external && thread(b.threadId);

    if (!chat) return;
    const last = [...chat.turns].reverse().find((u) => u.notice?.kind === "agents" && u.notice.conversation === t.agents.conversation);
    const at = now();

    if (last && at - last.notice.at <= LIMITS.mergeMs) {
      last.notice = { ...last.notice, from: msg.from, to: msg.to, count: last.notice.count + 1, last_id: msg.id, at };
      last.text = agentsNoticeText(last.notice);

      return;
    }
    const n = {
      kind: "agents", conversation: t.agents.conversation, thread: t.id, thread_uid: t.uid,
      from: msg.from, to: msg.to, subject: t.agents.subject, count: 1, last_id: msg.id, at,
    };

    chat.turns.push({ id: uid(), role: "notice", text: agentsNoticeText(n), notice: n });
  }

  function notice(b, n) {
    const chat = b && thread(b.threadId);

    if (chat) chat.turns.push({ id: uid(), role: "notice", text: agentsNoticeText(n), notice: { ...n, at: now() } });
  }

  function limited(caller) {
    const since = now() - 10 * 60_000;
    const times = (sent.get(caller.id) || []).filter((x) => x > since);

    sent.set(caller.id, times);

    if (times.length >= LIMITS.sends) fail("rate_limited", `at most ${LIMITS.sends} messages per 10 minutes`);
    times.push(now());
  }

  /** `agent_send`: validates, records the message in its conversation's
   *  thread, posts the notices and hands it to `deliver` for each
   *  recipient. `hop` is the broker's: 0 from a human-driven turn, +1 for
   *  each relay while handling another agent's message. */
  function send(caller, input = {}, { hop = 0, id } = {}) {
    const text = typeof input.text === "string" ? input.text : "";

    if (!text.trim()) fail("invalid_args", "`text` is required");

    if (bytes(text) > LIMITS.text) fail("invalid_args", "`text` is over 16 KB");

    if (input.data !== undefined && (input.data === null || typeof input.data !== "object" || Array.isArray(input.data))) fail("invalid_args", "`data` must be an object");

    if (input.data && bytes(JSON.stringify(input.data)) > LIMITS.data) fail("invalid_args", "`data` is over 64 KB");
    const attachments = input.attachments ?? [];

    if (!Array.isArray(attachments) || attachments.length > LIMITS.attachments) fail("invalid_args", `\`attachments\` is a list of at most ${LIMITS.attachments} tokens`);

    for (const a of attachments) if (typeof a !== "string" || !TICKET.test(a)) fail("invalid_args", "an attachment is a ticket token (@<plugin>:<scope>:tkt_…), never a raw identifier");
    let deadline;

    if (input.deadline !== undefined) {
      deadline = Date.parse(input.deadline);

      if (!Number.isFinite(deadline)) fail("invalid_args", "`deadline` is an RFC 3339 time");

      if (deadline <= now()) fail("invalid_args", "`deadline` has passed");
    }
    let t = null;

    if (input.conversation !== undefined) {
      t = conversation(String(input.conversation));

      if (!t) fail("invalid_args", "no such conversation");

      if (!t.agents.participants.includes(caller.id)) fail("not_allowed", "you are not in this conversation");
    }

    if (hop > LIMITS.hop) {
      if (t) t.turns.push({ id: uid(), role: "notice", text: `Stopped: a message was relayed more than ${LIMITS.hop} times`, notice: { kind: "agents_hop_limit", at: now() } }), save();
      fail("hop_limit", `a message relayed more than ${LIMITS.hop} times between agents is refused`);
    }
    let to;

    // one agent may come as its id or name alone
    if (typeof input.to === "string" && input.to !== "*") input = { ...input, to: [input.to] };

    if (input.to === "*") to = reachable(caller).filter((b) => !b.helper);
    else if (Array.isArray(input.to) && input.to.length) to = [...new Set(input.to.map((r) => recipient(caller, r)))];
    else if (input.to === undefined && t) to = t.agents.participants.filter((p) => p !== caller.id).map(party).filter((p) => p && (p.external || live(p)));
    else if (input.to === undefined && caller.external) to = reachable(caller);
    else fail("invalid_args", 'give `to`: a list of agents, or "*" for all of them');

    if (!to.length) fail("invalid_args", "no agent to send to");

    if (to.length > LIMITS.recipients) fail("too_many_recipients", `at most ${LIMITS.recipients} recipients`);

    if (input.in_reply_to !== undefined && !t?.turns.some((u) => u.message?.id === input.in_reply_to)) fail("invalid_args", "`in_reply_to` is not a message of this conversation");

    if (!t) {
      const subject = clip(input.subject, LIMITS.subject);

      if (!subject) fail("invalid_args", "`subject` is required for a new conversation");
      t = {
        id: nextId(), uid: uid(), title: subject, turns: [],
        agents: { conversation: `conv_${uid()}`, participants: [caller.id], subject, created_by: caller.id, created_at: now() },
      };
      store.list.push(t);
    }

    if (t.agents.paused) fail("paused", "this conversation is paused until the user resumes it");
    // the last hour's messages, those after the user last resumed it
    const recent = (u) => {
      const at = Date.parse(u.message.sent_at);

      return at >= now() - 3600_000 && !(at <= t.agents.resumed_at);
    };

    if (t.turns.filter((u) => u.role === "agent_message" && recent(u)).length >= LIMITS.perHour) {
      t.agents.paused = now();
      const n = { kind: "agents_paused", conversation: t.agents.conversation, thread: t.id, thread_uid: t.uid };

      t.turns.push({ id: uid(), role: "notice", text: agentsNoticeText(n), notice: { ...n, at: now() } });

      for (const id of t.agents.participants) notice(bot(id), n);
      save();
      fail("paused", `these agents exchanged ${LIMITS.perHour} messages in an hour; the user resumes the conversation`);
    }
    // an outside sender has its endpoint's own rate (agents-external.mjs)
    if (!caller.external) limited(caller);

    for (const b of to) if (!t.agents.participants.includes(b.id)) t.agents.participants.push(b.id);
    const kind = (p) => (p.external ? "external" : "agent");
    const msg = {
      v: 1,
      id: id || `msg_${uid()}`,
      conversation: t.agents.conversation,
      ...(input.in_reply_to !== undefined ? { in_reply_to: input.in_reply_to } : {}),
      from: { kind: kind(caller), id: caller.id, name: caller.name },
      to: to.map((b) => ({ kind: kind(b), id: b.id, name: b.name })),
      subject: t.agents.subject,
      text,
      ...(input.data ? { data: input.data } : {}),
      ...(attachments.length ? { attachments } : {}),
      expects_reply: !!input.expects_reply,
      ...(deadline ? { deadline: new Date(deadline).toISOString() } : {}),
      hop,
      sent_at: new Date(now()).toISOString(),
    };

    t.turns.push({ id: uid(), role: "agent_message", text, message: msg, pending: to.map((b) => b.id) });
    // a new message reopens an archived conversation (spec 6)
    delete t.agents.archived;

    for (const b of [caller, ...to]) noticeIn(b, t, msg);
    save();

    for (const b of to) deliver(b, msg, t);

    return { id: msg.id, conversation: msg.conversation, delivered_to: [], queued_for: to.map((b) => b.id) };
  }

  /** The message `msg` reached `b` (its turn ran, or it needed none). */
  function delivered(t, msgId, botId) {
    const u = t.turns.find((x) => x.message?.id === msgId);

    if (!u?.pending) return;
    u.pending = u.pending.filter((id) => id !== botId);

    if (!u.pending.length) delete u.pending;
    save();
  }

  /** Every message still waiting for a recipient, oldest first — what a
   *  restart hands to `deliver` again (spec 5.5). */
  function undelivered() {
    const out = [];

    for (const t of store.list) {
      if (!t.agents) continue;

      for (const u of t.turns)
        for (const id of u.pending || []) {
          const p = party(id);

          if (p && (p.external || live(p))) out.push({ bot: p, msg: u.message, t });
        }
    }

    return out;
  }

  /** `agent_conversations {with?, limit?}`. */
  function conversations(caller, { with: other, limit = 20 } = {}) {
    const n = Math.max(1, Math.min(50, Number(limit) || 20));

    return store.list
      .filter((t) => t.agents?.participants.includes(caller.id) && (!other || t.agents.participants.includes(other)))
      .map((t) => {
        const msgs = t.turns.filter((u) => u.role === "agent_message");

        return {
          conversation: t.agents.conversation,
          subject: t.agents.subject,
          participants: t.agents.participants.map((id) => ({ id, name: party(id)?.name || id })),
          last_at: msgs.at(-1)?.message.sent_at || null,
          count: msgs.length,
          ...(t.agents.archived ? { archived: true } : {}),
          ...(t.agents.paused ? { paused: true } : {}),
        };
      })
      .sort((a, b) => String(b.last_at).localeCompare(String(a.last_at)))
      .slice(0, n);
  }

  /** `agent_read {conversation, since?}`: the messages, oldest first;
   *  `since` is a message id or a time. */
  function read(caller, { conversation: conv, since } = {}) {
    const t = conversation(String(conv || ""));

    if (!t) fail("invalid_args", "no such conversation");

    if (!t.agents.participants.includes(caller.id)) fail("not_allowed", "you are not in this conversation");
    let msgs = t.turns.filter((u) => u.role === "agent_message").map((u) => u.message);

    if (since) {
      const i = msgs.findIndex((m) => m.id === since);

      msgs = i >= 0 ? msgs.slice(i + 1) : msgs.filter((m) => m.sent_at > String(since));
    }
    // what the reader has seen: a reply it read needs no wake-up (agents-run.mjs)
    const ids = new Set(msgs.map((m) => m.id));

    for (const u of t.turns) if (ids.has(u.message?.id) && !(u.read_by || []).includes(caller.id)) u.read_by = [...(u.read_by || []), caller.id];
    save();

    return { conversation: t.agents.conversation, subject: t.agents.subject, messages: msgs };
  }

  /** Whether `botId` has read message `msgId` with agent_read. */
  function readBy(msgId, botId) {
    return store.list.some((t) => t.agents && t.turns.some((u) => u.message?.id === msgId && (u.read_by || []).includes(botId)));
  }

  /** `agent_create`: a permanent agent the user asked for, or the caller's
   *  one temporary helper (spec 4.1). It starts with at most the caller's
   *  reach and the model the user named, else the caller's. */
  function create(caller, input = {}, { hop = 0 } = {}) {
    if (caller.helper) fail("not_allowed", "a helper cannot create agents");
    const name = clip(input.name, 40);
    const purpose = clip(input.purpose, 300);

    if (!name) fail("invalid_args", "`name` is required");

    if (!purpose) fail("invalid_args", "`purpose` is required: what the agent is for");

    if (store.bots.some((b) => live(b) && b.name.toLowerCase() === name.toLowerCase())) fail("invalid_args", `an agent named ${name} exists`);
    const temporary = !!input.temporary;
    const helper = temporary && store.bots.find((b) => live(b) && b.helper?.parent === caller.id);

    if (helper) fail("not_allowed", `you already have a helper (${helper.name}); remove it with agent_remove first`);
    const own = caller.integrations || [];
    const wanted = Array.isArray(input.plugins) ? input.plugins.map(String) : null;
    const tid = nextId();
    const b = {
      id: `b${now().toString(36)}${Math.floor(Math.random() * 46656).toString(36)}`,
      name,
      shape: "blob",
      color: clip(input.avatar, 9) || caller.color || "#1084FE",
      threadId: tid,
      ...defaults(name),
      description: purpose,
      ...(clip(input.instructions, 4000) ? { instructions: clip(input.instructions, 4000) } : {}),
      // the model the user named, else the parent's; its own picker after
      model: clip(input.model, 80) || caller.model || "",
      effort: caller.effort || "",
      fast: !!caller.fast,
      memories: [],
      // never more than the parent can use
      integrations: wanted ? own.filter((x) => wanted.includes(typeof x === "string" ? x : x?.id)) : [...own],
      made_by: caller.id,
      purpose,
      ...(temporary ? { helper: { parent: caller.id, task: purpose, created_at: now() } } : {}),
    };

    store.list.push({ id: tid, uid: uid(), title: name, turns: [] });
    store.bots.push(b);
    const chat = thread(tid);

    notice(caller, { kind: "agent_created", agent: b.id, name, parent: caller.name, temporary, task: purpose, thread: tid, thread_uid: chat.uid });
    save();
    let conv;

    if (clip(input.first_message, LIMITS.text))
      conv = send(caller, { to: [b.id], subject: clip(purpose, LIMITS.subject), text: String(input.first_message), expects_reply: true }, { hop }).conversation;

    return { id: b.id, name, temporary, ...(conv ? { conversation: conv } : {}) };
  }

  /** `agent_remove {id}`: ends a temporary helper the caller made; its
   *  chat and conversations stay, archived ("Past helpers"). */
  function remove(caller, { id } = {}) {
    const b = bot(String(id || ""));

    if (!b?.helper || b.helper.parent !== caller.id) fail("not_allowed", "you can remove only a temporary helper you made");

    if (b.helper.ended_at) fail("invalid_args", `${b.name} has finished already`);
    b.helper = { ...b.helper, ended_at: now() };

    for (const t of store.list) if (t.agents?.participants.includes(b.id)) t.agents.archived = true;
    const chat = thread(b.threadId);

    notice(caller, { kind: "helper_finished", agent: b.id, name: b.name, thread: b.threadId, thread_uid: chat?.uid });
    save();

    return { removed: true };
  }

  /** The user archives a conversation, or brings it back. */
  function archive(conv, archived = true) {
    const t = conversation(String(conv || ""));

    if (!t) fail("invalid_args", "no such conversation");

    if (archived) t.agents.archived = true;
    else delete t.agents.archived;
    save();
  }

  /** The user resumes a paused conversation: the hourly count starts over. */
  function resume(conv) {
    const t = conversation(String(conv || ""));

    if (!t) fail("invalid_args", "no such conversation");
    delete t.agents.paused;
    t.agents.resumed_at = now();
    save();
  }

  /** "Accept messages from": "all", "none", or a list of agent ids. */
  function setAccepts(botId, accepts) {
    const b = bot(String(botId || ""));

    if (!b) fail("invalid_args", "no such agent");

    if (!(accepts === "all" || accepts === "none" || (Array.isArray(accepts) && accepts.every((x) => typeof x === "string")))) fail("invalid_args", 'accepts is "all", "none" or a list of agent ids');
    b.accepts = accepts;
    save();
  }

  /** A message from an outside endpoint {id, name, bot}: to its agent unless
   *  `to` says otherwise; `id` is the sender's idempotency key. */
  function sendFrom(ep, input = {}, { id } = {}) {
    const caller = party(`ext:${ep.id}`);

    if (!caller) fail("invalid_args", "no such outside endpoint");

    return send(caller, input, { hop: 0, id });
  }

  return { list, send, sendFrom, readBy, delivered, undelivered, conversations, read, create, remove, archive, resume, setAccepts, conversation, reachable, party };
}
