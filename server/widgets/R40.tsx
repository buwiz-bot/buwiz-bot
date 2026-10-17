import { signal, Text, View } from "nsh";

const S = "R40";

export default function R() {
  const count = signal(0);
  const loaded = signal(false);

  fetch(`http://127.0.0.1:8787/state/${S}`)
    .then((r) => r.json())
    .then((d: any) => {
      if (d && d.state && typeof d.state.count === "number") count.set(d.state.count);
      loaded.set(true);
    })
    .catch(() => loaded.set(true));

  const save = (v: number) => {
    fetch(`http://127.0.0.1:8787/state/${S}`, {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify({ count: v }),
    }).catch(() => {});
  };

  const change = (delta: number) => {
    const v = count() + delta;
    count.set(v);
    save(v);
  };

  return (
    <View class="flex flex-col items-center gap-4 p-6 rounded-2xl bg-white border border-gray-200">
      <Text class="text-sm text-gray-500">Persistent counter</Text>
      <Text class="text-5xl font-bold text-gray-900">{count()}</Text>
      <View class="flex flex-row gap-4">
        <View onPress={() => change(-1)} activeOpacity={0.7} class="w-16 h-16 rounded-full bg-red-500 items-center justify-center">
          <Text class="text-2xl font-bold text-white">-</Text>
        </View>
        <View onPress={() => change(1)} activeOpacity={0.7} class="w-16 h-16 rounded-full bg-green-500 items-center justify-center">
          <Text class="text-2xl font-bold text-white">+</Text>
        </View>
      </View>
      <Text class="text-xs text-gray-400">{loaded() ? "Saved" : "Loading..."}</Text>
    </View>
  );
}