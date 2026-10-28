// v1/routine-threads.mjs — where a v1 routine's runs land (spec 5.9,
// "Where a routine's runs go"). Each routine has its own thread, made on
// its first run and reused by every run after, titled by the routine's
// name and marked `routine: {plugin, id, name, thread}` — `thread` is the
// chat the routine reports to (`v1_routines.thread`), whose bot answers in
// the routine's thread too. It is never a sidebar row.
//
// The chat it reports to gets one `notice` turn per run that ran items or
// failed: a line the client draws centered, opening the routine's thread
// at that run. A run with nothing new leaves nothing. Works on the store's
// shape ({list: [{id, uid, title, turns, routine?}]}) so the tests can
// hand it a plain object.
import { approvedText } from "./agent.mjs";

/** The notice's one line: "<name> ran · 2 new items" / "<name> failed: …". */
export const noticeText = (n) =>
  n.error ? `${n.name} failed: ${approvedText(n.error, 200)}` : `${n.name} ran · ${n.items} new item${n.items === 1 ? "" : "s"}`;

export function createRoutineThreads({ store, nextId, uid, save }) {
  const byKey = (key) => store.list.find((t) => t.id === key || t.uid === key);
  const find = (plugin, routine) => store.list.find((t) => t.routine?.plugin === plugin && t.routine.id === routine) || null;

  /** The routine's thread, made on its first run. */
  function ensure({ plugin, routine, name, chat }) {
    const mark = { plugin, id: routine, name, thread: chat };
    let t = find(plugin, routine);

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

  return { ensure, find, notice };
}
