import { signal, effect, Text, View, Scroll } from "nsh";

const PRI = ["None", "Urgent", "High", "Medium", "Low"];
const pri = (p: any): string => (typeof p === "number" ? (PRI[p] ?? "") : (p ?? ""));

export default function R() {
  const issues = signal<any[]>([]);
  const status = signal<string>("loading");
  const scope = signal<string>("");

  const load = () => {
    status.set("loading");
    fetch("http://127.0.0.1:8787/data/linear/issues")
      .then((r) => r.json())
      .then((d: any) => {
        if (!d || d.error || !Array.isArray(d.issues)) {
          status.set("error");
          return;
        }
        scope.set(d.scope || "");
        issues.set(d.issues);
        status.set("ok");
      })
      .catch(() => status.set("error"));
  };

  effect(() => {
    load();
  });

  return (
    <View class="flex-col gap-3 p-4 rounded-xl border border-gray-200 bg-white">
      <View class="flex-row items-center justify-between">
        <View class="flex-col">
          <Text class="text-base font-bold text-gray-900">Linear issues</Text>
          <Text class="text-xs text-gray-500">
            {scope() === "assigned" ? "Assigned to you" : scope() === "workspace" ? "Recent in workspace" : "Open issues"}
          </Text>
        </View>
        <View onPress={load} activeOpacity={0.7} class="px-3 py-1 rounded-md bg-gray-100">
          <Text class="text-sm text-gray-700">↻ Refresh</Text>
        </View>
      </View>

      <If when={status() === "loading"}>
        <Text class="text-sm text-gray-500">Loading issues…</Text>
      </If>

      <If when={status() === "error"}>
        <View class="flex-col gap-2">
          <Text class="text-sm text-red-600">Couldn't load Linear issues.</Text>
          <View onPress={load} activeOpacity={0.7} class="self-start px-3 py-1 rounded-md bg-blue-500">
            <Text class="text-sm text-white">Retry</Text>
          </View>
        </View>
      </If>

      <If when={status() === "ok" && issues().length === 0}>
        <Text class="text-sm text-gray-500">No open issues 🎉</Text>
      </If>

      <If when={status() === "ok" && issues().length > 0}>
        <Scroll class="max-h-96">
          <View class="flex-col gap-2">
            <Each of={issues()}>
              {(item: any, i: any) => (
                <View class="flex-col gap-1 p-3 rounded-lg bg-gray-50 border border-gray-100">
                  <View class="flex-row items-center justify-between">
                    <Text class="text-xs font-mono text-blue-600">{item().id}</Text>
                    <Text class="text-xs text-gray-500">{pri(item().priority)}</Text>
                  </View>
                  <Text class="text-sm font-medium text-gray-900">{item().title}</Text>
                  <Text class="text-xs text-gray-600">{item().state}</Text>
                </View>
              )}
            </Each>
          </View>
        </Scroll>
      </If>
    </View>
  );
}