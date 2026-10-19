import { cleanup, effect, Input, signal, Text, View } from "nsh";
import { isAvailable, Menu } from "nsh/desktop";
import { keymap } from "nsh/keymap";
import { tc } from "../theme";

const GEN = "http://127.0.0.1:8787";
const get = (p: string) =>
  fetch(`${GEN}${p}`).then((r) => (r.ok ? r.json() : null)).catch(() => null);
const post = (p: string, b: object) =>
  fetch(`${GEN}${p}`, {
    method: "POST",
    headers: { "Content-Type": "application/json" },
    body: JSON.stringify(b),
  }).catch(() => null);

type Item = {
  name: string;
  title: string;
  publishedAt: string;
  kinds: string[];
  pinned: boolean;
  archived: boolean;
  tags: string[];
};

// kinds: db = persisted via /state (surfaces share it) · api = fetches a
// /data source · keys = keyboard-driven (↗ window for real key control) ·
// mem = in-memory only, every surface gets fresh state.
const KIND_COLOR: Record<string, string> = {
  db: "#10a37f",
  api: "#5e6ad2",
  keys: "#f2994a",
  mem: "#f5f5f585",
};

function TitleBox(props: {
  item: Item;
  reload: () => void;
  editing: boolean;
  begin: () => void;
  done: () => void;
}) {
  let input!: any;
  // The field is uncontrolled (defaultValue) — its live text rides in
  // 'latest' via a listener that never writes back (that's what kept
  // snapping the old title over an edit). Enter commits it, Escape drops
  // it, blur/tab-away commits it — the usual text-field contract.
  let latest = props.item.title;
  let open = false;
  let cancelled = false;
  let finished = false;
  keymap.bind({
    escape: () => {
      if (!open) return;
      cancelled = true;
      try {
        input?.blur?.();
      } catch {}
      props.done();
    },
  });
  const commit = (t: string) => {
    if (finished) return;
    finished = true;
    props.done();
    t = t.trim();
    if (t && t !== props.item.title)
      post("/widget-meta", { name: props.item.name, title: t }).then(() =>
        props.reload(),
      );
  };
  // Double-click to rename — a single stray click on the title never
  // opens the editor (that was the annoying part).
  let lastTap = 0;
  return (
    <If
      when={props.editing}
      else={
        <View
          class="flex-row items-center gap-1"
          onPress={() => {
            const t = Date.now();
            if (t - lastTap < 380) props.begin();
            lastTap = t;
          }}
          activeOpacity={0.7}
          accessibilityLabel="double-click to rename"
        >
          <Text
            style={{ color: tc("#f5f5f5"), fontSize: 12.5, fontWeight: "600" }}
            numberOfLines={1}
          >
            {props.item.title}
          </Text>
          <Text style={{ color: tc("#f5f5f566"), fontSize: 10 }}>✎</Text>
        </View>
      }
    >
      <Input
        ref={input}
        defaultValue={props.item.title}
        returnKeyType="done"
        autoFocus
        onFocus={() => (open = true)}
        onChangeText={(t) => (latest = t)}
        onBlur={() => {
          open = false;
          if (!cancelled) commit(latest);
          cancelled = false;
        }}
        onSubmitEditing={(e) => commit(e.text)}
        style={{
          fontSize: 12.5,
          fontWeight: "600",
          color: tc("#f5f5f5"),
          paddingVertical: 1,
          paddingHorizontal: 5,
          backgroundColor: tc("#f5f5f50f"),
          borderRadius: 5,
        }}
      />
    </If>
  );
}

function TagBox(props: {
  item: Item;
  reload: () => void;
  open: boolean;
  begin: () => void;
  done: () => void;
}) {
  let input!: any;
  let latest = "";
  let open = false;
  let cancelled = false;
  let finished = false;
  keymap.bind({
    escape: () => {
      if (!open) return;
      cancelled = true;
      try {
        input?.blur?.();
      } catch {}
      props.done();
    },
  });
  const commit = (raw: string) => {
    if (finished) return;
    finished = true;
    props.done();
    const t = raw.trim().toLowerCase();
    if (!t) return;
    post("/widget-meta", {
      name: props.item.name,
      tags: [...props.item.tags, t],
    }).then(() => props.reload());
  };
  return (
    <If
      when={props.open}
      else={
        <View
          onPress={props.begin}
          activeOpacity={0.7}
          style={{
            borderRadius: 999,
            paddingHorizontal: 7,
            paddingVertical: 2,
            backgroundColor: tc("#f5f5f50f"),
          }}
        >
          <Text style={{ color: tc("#f5f5f566"), fontSize: 9 }}>+tag</Text>
        </View>
      }
    >
      <Input
        ref={input}
        defaultValue=""
        placeholder="+tag"
        placeholderTextColor={tc("#f5f5f566")}
        returnKeyType="done"
        autoFocus
        onChangeText={(t) => (latest = t)}
        onFocus={() => (open = true)}
        onBlur={() => {
          open = false;
          if (!cancelled) commit(latest);
          cancelled = false;
        }}
        onSubmitEditing={(e) => commit(e.text)}
        style={{
          width: 56,
          fontSize: 10,
          color: tc("#f5f5f5"),
          paddingVertical: 2,
          paddingHorizontal: 6,
          backgroundColor: tc("#f5f5f50f"),
          borderRadius: 6,
        }}
      />
    </If>
  );
}

export function WidgetShelf() {
  const list = signal<Item[]>([]);
  const q = signal("");
  const kind = signal("all");
  const tag = signal("");
  const sent = signal("");
  // Which card is renaming / tagging right now (its name, "" = none).
  const renaming = signal("");
  const tagging = signal("");

  const load = async () => {
    const d = await get("/catalog");
    if (d?.list) list.set(d.list);
  };
  load();
  effect(() => {
    const t = setInterval(load, 3000);
    cleanup(() => clearInterval(t));
  });

  // The card's context menu — a real NSMenu at the pointer (right-click on
  // the card, or its ⋯ button, calls this).
  const openMenu = (i: Item) => {
    if (!isAvailable) return;
    Menu.popup([
      {
        label: "Open window",
        onSelect: () => post("/popout", { name: i.name }),
      },
      {
        label: "Pin to desktop",
        onSelect: () => post("/popout", { name: i.name, mode: "desktop" }),
      },
      { type: "separator" },
      { label: "Rename…", onSelect: () => renaming.set(i.name) },
      { label: "Add tag…", onSelect: () => tagging.set(i.name) },
      {
        // Re-prompt the model against the same request and patch the
        // published file — the fix path for a broken or badly-sized app.
        label: "Regenerate",
        onSelect: () => {
          post("/retry", { name: i.name });
        },
      },
      {
        label: i.pinned ? "Unpin" : "Nominate (★)",
        onSelect: () =>
          post("/pin", { name: i.name, pinned: !i.pinned }).then(load),
      },
      {
        label: i.archived ? "Unarchive" : "Archive",
        onSelect: () =>
          post("/widget-meta", { name: i.name, archived: !i.archived }).then(
            load,
          ),
      },
      { type: "separator" },
      {
        label: "Delete from Mini Apps",
        onSelect: () => post("/unpublish", { name: i.name }).then(load),
      },
    ]);
  };

  const allTags = () => [...new Set(list().flatMap((i) => i.tags))];
  const shown = () =>
    list().filter((i) => {
      const hay = `${i.title} ${i.name} ${i.tags.join(" ")}`.toLowerCase();
      const matchTxt =
        (!tag() || i.tags.includes(tag())) &&
        (!q() || hay.includes(q().toLowerCase()));
      // Archived apps hide unless the archived filter is on.
      if (kind() === "arch") return i.archived && matchTxt;
      return (
        !i.archived &&
        (kind() === "all" || i.kinds.includes(kind())) &&
        matchTxt
      );
    });

  const act = (g: string, onTap: () => void, on: boolean) => (
    <View
      onPress={onTap}
      activeOpacity={0.7}
      style={{
        borderRadius: 7,
        paddingHorizontal: 8,
        paddingVertical: 4,
        backgroundColor: on ? tc("#f5f5f514") : "transparent",
      }}
    >
      <Text style={{ color: on ? tc("#f5f5f5") : "#f5f5f585", fontSize: 12 }}>{g}</Text>
    </View>
  );

  const chip = (label: string, on: boolean, onTap: () => void, color = "#5e6ad2") => (
    <View
      onPress={onTap}
      activeOpacity={0.7}
      style={{
        borderRadius: 999,
        paddingHorizontal: 10,
        paddingVertical: 3,
        backgroundColor: on ? color : tc("#f5f5f50f"),
      }}
    >
      <Text style={{ color: on ? tc("#f5f5f5") : tc("#f5f5f5a8"), fontSize: 10, fontWeight: "600" }}>
        {label}
      </Text>
    </View>
  );

  const card = (w: () => Item) => (
    <View
      onContextMenu={() => openMenu(w())}
      style={{
        width: "48%",
        borderRadius: 14,
        borderWidth: 1,
        borderColor: tc("#f5f5f51f"),
        backgroundColor: tc("#1f1f1f"),
        padding: 12,
        gap: 7,
      }}
    >
      <View class="flex-row items-center gap-2">
        <View
          style={{
            width: 30,
            height: 30,
            borderRadius: 9,
            backgroundColor: tc(KIND_COLOR[w().kinds[0]] || "#f5f5f585"),
            alignItems: "center",
            justifyContent: "center",
          }}
        >
          <Text style={{ color: "#fff", fontSize: 13, fontWeight: "700" }}>
            {w().title.slice(0, 1).toUpperCase()}
          </Text>
        </View>
        <Text style={{ color: tc("#f5f5f566"), fontSize: 9.5, flex: 1 }}>
          {w().name}.tsx · {w().publishedAt?.slice(0, 10)}
        </Text>
        {act("↗", () => post("/popout", { name: w().name }), false)}
        {act("⋯", () => openMenu(w()), false)}
      </View>
      <TitleBox
        item={w()}
        reload={load}
        editing={renaming() === w().name}
        begin={() => renaming.set(w().name)}
        done={() => renaming.set("")}
      />
      <View class="flex-row items-center gap-1" style={{ flexWrap: "wrap" }}>
        <Each of={w().kinds} key={(k) => k}>
          {(k) => (
            <View
              style={{
                borderRadius: 999,
                paddingHorizontal: 7,
                paddingVertical: 2,
                backgroundColor: tc(KIND_COLOR[k()] || "#f5f5f585") + "26",
              }}
            >
              <Text
                style={{
                  color: tc(KIND_COLOR[k()] || "#f5f5f585"),
                  fontSize: 9,
                  fontWeight: "700",
                }}
              >
                {k()}
              </Text>
            </View>
          )}
        </Each>
        <Each of={w().tags} key={(t) => t}>
          {(t) => (
            <View
              onPress={() => {
                post("/widget-meta", {
                  name: w().name,
                  tags: w().tags.filter((x) => x !== t()),
                }).then(load);
              }}
              activeOpacity={0.7}
              style={{
                borderRadius: 999,
                paddingHorizontal: 7,
                paddingVertical: 2,
                backgroundColor: tc("#f5f5f50f"),
              }}
            >
              <Text style={{ color: tc("#f5f5f5a8"), fontSize: 9 }}>#{t()}</Text>
            </View>
          )}
        </Each>
        <TagBox
          item={w()}
          reload={load}
          open={tagging() === w().name}
          begin={() => tagging.set(w().name)}
          done={() => tagging.set("")}
        />
        <View style={{ flex: 1 }} />
        <View
          onPress={() => {
            post("/reuse", { name: w().name }).then(() => {
              sent.set(w().name);
              setTimeout(() => sent.set(""), 1500);
            });
          }}
          activeOpacity={0.7}
          style={{
            borderRadius: 8,
            paddingHorizontal: 9,
            paddingVertical: 4,
            backgroundColor: tc("#f5f5f514"),
          }}
        >
          <Text style={{ color: tc("#f5f5f5"), fontSize: 10.5 }}>
            {sent() === w().name ? "＋ chat ✓" : "＋ chat"}
          </Text>
        </View>
      </View>
    </View>
  );

  return (
    <View class="gap-3 px-4 py-4">
      <View class="flex-row items-center justify-between">
        <Text style={{ color: tc("#f5f5f566"), fontSize: 12, fontWeight: "600" }}>
          {"MINI APPS — " + shown().length}
        </Text>
        <Input
          value={q()}
          onChangeText={(t) => q.set(t)}
          placeholder="filter…"
          placeholderTextColor={tc("#f5f5f566")}
          style={{
            width: 140,
            fontSize: 11,
            color: tc("#f5f5f5"),
            paddingVertical: 4,
            paddingHorizontal: 10,
            backgroundColor: tc("#f5f5f50f"),
            borderRadius: 8,
          }}
        />
      </View>
      <View class="flex-row gap-1.5" style={{ flexWrap: "wrap" }}>
        {chip("all", kind() === "all" && !tag(), () => {
          kind.set("all");
          tag.set("");
        }, tc("#f5f5f53d"))}
        {chip("db", kind() === "db", () => kind.set(kind() === "db" ? "all" : "db"), KIND_COLOR.db)}
        {chip("api", kind() === "api", () => kind.set(kind() === "api" ? "all" : "api"), KIND_COLOR.api)}
        {chip("keys", kind() === "keys", () => kind.set(kind() === "keys" ? "all" : "keys"), KIND_COLOR.keys)}
        {chip("mem", kind() === "mem", () => kind.set(kind() === "mem" ? "all" : "mem"), KIND_COLOR.mem)}
        {chip("archived", kind() === "arch", () => kind.set(kind() === "arch" ? "all" : "arch"), "#c95901")}
        <Each of={allTags()} key={(t) => t}>
          {(t) =>
            chip("#" + t(), tag() === t(), () => tag.set(tag() === t() ? "" : t()), "#7c5e10")
          }
        </Each>
      </View>
      <View class="flex-row" style={{ flexWrap: "wrap", gap: 12 }}>
        <Each of={shown()} key={(i) => i.name}>{(w) => card(w)}</Each>
      </View>
      <If when={!shown().length}>
        <Text style={{ color: tc("#f5f5f566"), fontSize: 13 }}>
          Nothing here — publish a generated card with ⬆, or loosen the filter.
        </Text>
      </If>
    </View>
  );
}
