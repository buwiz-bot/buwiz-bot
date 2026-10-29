// v1/routine-threads.mjs — where a v1 routine's runs land (spec 5.9,
// "Where a routine's runs go"). Each routine instance has its own thread,
// made when it is enabled (routines.mjs) and reused by every run, titled by
// the routine's name (and the instance's params) and marked `routine:
// {plugin, id, instance, name, thread}` — `thread` is the chat the
// instance reports to (`v1_instances.thread`), whose bot answers in the
// routine's thread too. It is never a sidebar row.
//
// The chat it reports to gets one `notice` turn per run that ran items or
// failed: a line the client draws centered, opening the routine's thread
// at that run. A run with nothing new leaves nothing. Works on the store's
// shape ({list: [{id, uid, title, turns, routine?}]}) so the tests can
// hand it a plain object.
import { approvedText } from "./agent.mjs";

/** The notice's one line: "<name> ran · 2 new items" / "<name> failed: …";
 *  a consent card's question. */
export function noticeText(n) {
  if (n.kind === "routine_consent") {
    const what = n.summary ? `${n.name} (${n.summary})` : n.name;
    const may = n.writes?.length ? ` It may change ${n.title} data without asking: ${n.writes.join(", ")}.` : "";
    const reads = n.reads?.length ? ` It reads ${n.reads.join("; ")}.` : "";

    return `Enable ${what} here?${may}${reads}`;
  }

  return n.error ? `${n.name} failed: ${approvedText(n.error, 200)}` : `${n.name} ran · ${n.items} new item${n.items === 1 ? "" : "s"}`;
}

export function createRoutineThreads({ store, nextId, uid, save }) {
  const byKey = (key) => store.list.find((t) => t.id === key || t.uid === key);
  const find = (plugin, routine, instance) =>
    store.list.find((t) => t.routine?.plugin === plugin && t.routine.id === routine && t.routine.instance === instance) || null;

  /** Gives a routine's thread from before instances to `instance`. */
  function adopt(plugin, routine, instance) {
    const t = store.list.find((x) => x.routine?.plugin === plugin && x.routine.id === routine && !x.routine.instance);

    if (!t) return;
    t.routine = { ...t.routine, instance };
    save();
  }

  /** The instance's thread, made the first time it is asked for. */
  function ensure({ plugin, routine, instance, name, chat }) {
    const mark = { plugin, id: routine, instance, name, thread: chat };
    let t = find(plugin, routine, instance);

    if (!t) {
      t = { id: nextId(), uid: uid(), title: name, turns: [], routine: mark };
      store.list.push(t);
      save();
    } else if (t.routine.name !== name || t.routine.thread !== chat) {
      // renamed in an update, or enabled in another chat since
      t.routine = mark;
      t.title = name;
      save();
    }

    return t;
  }

  /** Appends a run's notice to the chat `chat`; null when it is gone. */
  function notice(chat, n) {
    const t = byKey(chat);

    if (!t) return null;
    const turn = { id: uid(), role: "notice", text: noticeText(n), notice: n };

    t.turns.push(turn);
    save();

    return turn;
  }

  /** The consent cards for `instance` in chat `chat` read as `state`
   *  ("enabled" / "declined"). */
  function settle(chat, instance, state) {
    const t = byKey(chat);
    let changed = false;

    for (const u of t?.turns || []) {
      if (u.notice?.kind === "routine_consent" && u.notice.instance === instance && u.notice.state === "pending") {
        u.notice = { ...u.notice, state };
        changed = true;
      }
    }

    if (changed) save();
  }

  return { adopt, ensure, find, notice, settle };
}
