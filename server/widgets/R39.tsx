import { signal, effect, cleanup, Text, View } from "nsh";

const S = "R39";
const FOCUS = 25 * 60;

export default function R() {
  const remaining = signal(FOCUS);
  const running = signal(false);
  const sessions = signal(0);
  const minutesFocused = signal(0);

  function save() {
    fetch(`http://127.0.0.1:8787/state/${S}`, {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify({ sessions: sessions(), minutesFocused: minutesFocused() }),
    });
  }

  fetch(`http://127.0.0.1:8787/state/${S}`)
    .then((r) => r.json())
    .then((d) => {
      if (d && d.state) {
        sessions.set(d.state.sessions ?? 0);
        minutesFocused.set(d.state.minutesFocused ?? 0);
      }
    })
    .catch(() => {});

  effect(() => {
    if (!running()) return;
    const t = setInterval(() => {
      remaining.update((r) => r - 1);
      if (remaining() <= 0) {
        running.set(false);
        remaining.set(FOCUS);
        sessions.update((s) => s + 1);
        minutesFocused.update((m) => m + 25);
        save();
      }
    }, 1000);
    cleanup(() => clearInterval(t));
  });

  const mm = () => String(Math.floor(remaining() / 60)).padStart(2, "0");
  const ss = () => String(remaining() % 60).padStart(2, "0");
  const pct = () => ((FOCUS - remaining()) / FOCUS) * 100;

  function toggle() {
    if (running()) {
      running.set(false);
    } else {
      if (remaining() <= 0) remaining.set(FOCUS);
      running.set(true);
    }
  }

  function reset() {
    running.set(false);
    remaining.set(FOCUS);
  }

  return (
    <View class="flex-col gap-4 p-5 rounded-2xl bg-white border border-slate-200 items-center">
      <Text class="text-sm font-semibold text-red-500 uppercase">
        {running() ? "Focus in progress" : remaining() < FOCUS ? "Paused" : "Ready to focus"}
      </Text>

      <Text class="text-6xl font-bold text-slate-900">
        {mm()}:{ss()}
      </Text>

      <View class="w-full h-2 rounded-full bg-slate-200 overflow-hidden">
        <View class="h-2 bg-red-500" style={{ width: `${pct()}%` }} />
      </View>

      <View class="flex-row gap-3 w-full">
        <View
          class="flex-1 py-3 rounded-xl bg-red-500 items-center"
          onPress={toggle}
          activeOpacity={0.7}
        >
          <Text class="text-white font-semibold">{running() ? "Pause" : "Start"}</Text>
        </View>
        <View
          class="flex-1 py-3 rounded-xl bg-slate-200 items-center"
          onPress={reset}
          activeOpacity={0.7}
        >
          <Text class="text-slate-700 font-semibold">Reset</Text>
        </View>
      </View>

      <View class="flex-row gap-3 w-full pt-2 border-t border-slate-100">
        <View class="flex-1 items-center">
          <Text class="text-2xl font-bold text-slate-900">{sessions()}</Text>
          <Text class="text-xs text-slate-500">Sessions</Text>
        </View>
        <View class="flex-1 items-center">
          <Text class="text-2xl font-bold text-slate-900">{minutesFocused()}</Text>
          <Text class="text-xs text-slate-500">Minutes focused</Text>
        </View>
      </View>
    </View>
  );
}