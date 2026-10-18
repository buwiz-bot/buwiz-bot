import { signal, effect, cleanup, Text, View } from "nsh";

const S = "R41";
const BASE = "http://127.0.0.1:8787";

export default function R() {
  const data = signal<any>(null);
  const err = signal<boolean>(false);
  const busy = signal<boolean>(false);
  const now = signal<number>(Date.now());

  const pad = (n: number): string => (n < 10 ? "0" : "") + n;
  const clock = (iso: string): string => {
    const d = new Date(iso);
    return pad(d.getHours()) + ":" + pad(d.getMinutes());
  };
  const fmtMin = (m: number): string => {
    const h = Math.floor(m / 60);
    const mm = Math.floor(m % 60);
    return h > 0 ? h + "h " + pad(mm) + "m" : mm + "m";
  };
  const fmtSpan = (ms: number): string => {
    const s = Math.max(0, Math.floor(ms / 1000));
    return pad(Math.floor(s / 3600)) + ":" + pad(Math.floor((s % 3600) / 60)) + ":" + pad(s % 60);
  };

  const save = (): void => {
    const d: any = data();
    if (!d) return;
    fetch(`${BASE}/state/${S}`, {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify({ status: d.status, todayMin: d.todayMin }),
    }).catch(() => {});
  };

  const load = (): void => {
    fetch(`${BASE}/data/timeclock`)
      .then((r) => r.json())
      .then((d: any) => {
        if (!d || d.error) throw new Error("bad");
        data.set(d);
        err.set(false);
        save();
      })
      .catch(() => err.set(true));
  };

  const toggle = (): void => {
    const d: any = data();
    if (!d || busy()) return;
    const path = d.status === "in" ? "/data/timeclock/out" : "/data/timeclock/in";
    busy.set(true);
    fetch(BASE + path, { method: "POST" })
      .then(() => load())
      .catch(() => err.set(true))
      .finally(() => busy.set(false));
  };

  effect(() => {
    fetch(`${BASE}/state/${S}`)
      .then((r) => r.json())
      .then((d: any) => {
        if (d && d.state && d.state.todayMin != null) {
          data.set({ status: d.state.status || "out", since: null, todayMin: d.state.todayMin, sessions: [] });
        }
      })
      .catch(() => {});
    load();
    const poll = setInterval(load, 5000);
    // tick on wall-second boundaries so every surface shows the same second
    let tick = 0;
    const t0 = setTimeout(() => {
      now.set(Date.now());
      tick = setInterval(() => now.set(Date.now()), 1000);
    }, 1000 - (Date.now() % 1000));
    cleanup(() => {
      clearInterval(poll);
      clearTimeout(t0);
      clearInterval(tick);
    });
  });

  const isIn = (): boolean => !!data() && data().status === "in";
  const elapsedMin = (): number => {
    if (!data() || !isIn() || !data().since) return 0;
    return Math.max(0, (now() - Date.parse(data().since)) / 60000);
  };
  const todayTotal = (): number => (data() ? data().todayMin + (isIn() ? Math.floor(elapsedMin()) : 0) : 0);
  const recent = (): any[] => (data() && data().sessions ? data().sessions.slice(-5).reverse() : []);

  return (
    <View class="flex-col gap-3 p-4 rounded-xl border border-gray-200 bg-white w-full max-w-md">
      <View class="flex-row justify-between items-center">
        <Text class="text-lg font-bold text-gray-900">Time clock</Text>
        <View class="flex-row items-center gap-2">
          <View class={isIn() ? "w-2 h-2 rounded-full bg-green-500" : "w-2 h-2 rounded-full bg-gray-300"} />
          <Text class={isIn() ? "text-sm font-semibold text-green-600" : "text-sm font-semibold text-gray-500"}>
            {isIn() ? "Clocked in" : "Clocked out"}
          </Text>
        </View>
      </View>

      <If when={err() && !data()}>
        <View class="flex-col items-center gap-2 py-4">
          <Text class="text-sm text-gray-500">Couldn't load time clock</Text>
          <View onPress={load} activeOpacity={0.7} class="px-3 py-1 rounded-md bg-gray-100">
            <Text class="text-sm text-gray-700">Retry</Text>
          </View>
        </View>
      </If>

      <If when={data()}>
        <View class="flex-col items-center gap-1 py-2">
          <Text class="text-3xl font-bold text-gray-900">
            {isIn() && data().since ? fmtSpan(now() - Date.parse(data().since)) : "--:--:--"}
          </Text>
          <Text class="text-xs text-gray-500">
            {isIn() && data().since ? "Since " + clock(data().since) : "Not on the clock"}
          </Text>
        </View>

        <View
          onPress={toggle}
          activeOpacity={0.7}
          class={isIn() ? "py-4 rounded-lg bg-red-500 items-center" : "py-4 rounded-lg bg-green-600 items-center"}
        >
          <Text class="text-lg font-bold text-white">
            {busy() ? "..." : isIn() ? "Clock out" : "Clock in"}
          </Text>
        </View>

        <View class="flex-row justify-between items-center px-1">
          <Text class="text-sm text-gray-500">Today</Text>
          <Text class="text-base font-semibold text-gray-900">{fmtMin(todayTotal())}</Text>
        </View>

        <View class="flex-col gap-1 pt-2 border-t border-gray-100">
          <Text class="text-xs font-semibold text-gray-400">Recent sessions</Text>
          <If when={recent().length === 0}>
            <Text class="text-sm text-gray-400 py-1">No sessions yet</Text>
          </If>
          <Each of={recent()}>
            {(s: any, i: any) => (
              <View class="flex-row justify-between items-center py-1">
                <Text class="text-sm text-gray-700">
                  {clock(s().in)} – {s().out ? clock(s().out) : "now"}
                </Text>
                <Text class="text-sm font-medium text-gray-900">{fmtMin(s().min || 0)}</Text>
              </View>
            )}
          </Each>
        </View>
      </If>
    </View>
  );
}