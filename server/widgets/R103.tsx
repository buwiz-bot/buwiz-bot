import { signal, effect, cleanup, Text, View } from "nsh";

const S = "R103";
const URL = `http://127.0.0.1:8787/state/${S}`;
const NAMES = ["Sun", "Mon", "Tue", "Wed", "Thu", "Fri", "Sat"];
const RAW = [
  { icon: "⛅", hi: 32, lo: 25, rain: 40, desc: "Partly cloudy" },
  { icon: "⛈", hi: 30, lo: 25, rain: 80, desc: "Thunderstorms" },
  { icon: "🌦", hi: 31, lo: 24, rain: 60, desc: "Showers" },
  { icon: "☀️", hi: 33, lo: 25, rain: 10, desc: "Sunny" },
  { icon: "⛅", hi: 32, lo: 26, rain: 30, desc: "Partly cloudy" },
];
const today = new Date().getDay();
const DAYS = RAW.map((d, i) => ({ ...d, label: i === 0 ? "Today" : NAMES[(today + i) % 7] }));

export default function R() {
  const unit = signal("C");
  let snap = "";

  const conv = (c: number): number => (unit() === "C" ? c : Math.round((c * 9) / 5 + 32));

  const save = () => {
    const body = JSON.stringify({ unit: unit() });
    snap = JSON.stringify({ unit: unit() });
    fetch(URL, { method: "POST", headers: { "Content-Type": "application/json" }, body })
      .catch(() => {});
  };

  const toggle = () => {
    unit.set(unit() === "C" ? "F" : "C");
    save();
  };

  effect(() => {
    const load = async () => {
      try {
        const d = await fetch(URL).then((r) => r.json());
        const s = JSON.stringify(d?.state || {});
        if (s !== "{}" && s !== snap) {
          snap = s;
          if (d.state.unit) unit.set(d.state.unit);
        }
      } catch {}
    };
    load();
    const t = setInterval(load, 2000);
    cleanup(() => clearInterval(t));
  });

  return (
    <View class="flex-col w-full p-4 rounded-2xl bg-sky-600 gap-3">
      <View class="flex-row justify-between items-center">
        <View class="flex-col">
          <Text class="text-white font-bold text-lg">Manila</Text>
          <Text class="text-sky-100 text-xs">Philippines</Text>
        </View>
        <View onPress={toggle} activeOpacity={0.7} class="px-3 py-1 rounded-full bg-sky-400">
          <Text class="text-white text-xs font-bold">{`°${unit()} · switch`}</Text>
        </View>
      </View>

      <View class="flex-row items-center justify-between">
        <Text class="text-white text-5xl font-bold">{`${conv(32)}°`}</Text>
        <View class="flex-col items-end">
          <Text class="text-white text-sm">Partly cloudy</Text>
          <Text class="text-sky-100 text-xs">{`Feels like ${conv(38)}° · Humidity 78%`}</Text>
          <Text class="text-sky-100 text-xs">{`H ${conv(32)}°  L ${conv(25)}°`}</Text>
        </View>
      </View>

      <View class="flex-row justify-between gap-1 mt-2">
        <Each of={DAYS}>
          {(d) => (
            <View class="flex-col items-center flex-1 py-2 rounded-xl bg-sky-500">
              <Text class="text-sky-100 text-xs">{d().label}</Text>
              <Text class="text-lg">{d().icon}</Text>
              <Text class="text-white text-xs font-bold">{`${conv(d().hi)}°`}</Text>
              <Text class="text-sky-200 text-xs">{`${conv(d().lo)}°`}</Text>
              <Text class="text-sky-100 text-xs">{`☂ ${d().rain}%`}</Text>
            </View>
          )}
        </Each>
      </View>

      <Text class="text-sky-200 text-xs">Sample forecast. Live weather isn't connected.</Text>
    </View>
  );
}