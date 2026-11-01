import { mount, Text, View } from "nsh";
import { isAvailable, Window } from "nsh/desktop";
import { colorScheme } from "nsh/device";
import R, * as mod from "${rel}";
// Sizing contract: `export const popoutSize = { w, h }` fixes both axes
// (canvas apps, games); `export const popoutWidth` sets the width and
// the height hugs the content; neither → ${POP_W}pt wide, hugged height.
const exact = (mod as any).popoutSize as { w?: number; h?: number } | undefined;
const wantW = Math.min(720, Math.max(240, Math.round(exact?.w ?? (mod as any).popoutWidth ?? ${hintW || POP_W})));
const capH = ${capH};
// The hidden title bar still paints the traffic lights over the top ~26pt —
// the strip is real chrome: pad the module below it, add it to the frame.
const STRIP = 26;
if (isAvailable) {
  Window.setTitle("widget · " + ${JSON.stringify(popName)});
  Window.setTitleBar("hidden", 32); // natural height: the lights land at AppKit's own (9,9) spot
  // A regular window level: aerospace keeps it out of the tile grid (a
  // free-standing window), but it stacks normally — it can sit behind
  // other windows and comes forward when focused, not pinned on top.
  Window.setLevel("normal");
  Window.setFrame({ width: wantW, height: ${wantH} + STRIP });
  // A spawned host process has no activation context — without this the
  // window orders in behind the front app, never key (grayed lights).
  Window.orderFront();
}
// The window hugs the widget's real height — with manners. A resize is
// applied only once the measured height has been stable for ~150ms, so a
// transient "loading" or empty render can never collapse it; a change
// under 12pt is wobble, not layout. anchorTop keeps the top edge fixed —
// the window grows and shrinks at its bottom edge. popoutSize-h modules
// opt out of hugging entirely: their frame is exactly what they asked.
let lastH = ${hintH ? wantH : 0}, pendingH = 0, pendingT = 0, timer: any = null;
const hug = (raw: number) => {
  if (!isAvailable || exact?.h) return;
  const h = Math.max(120, Math.min(capH, Math.ceil(raw)));
  if (!isFinite(h)) return;
  if (Math.abs(h - lastH) <= 12) return;
  if (h !== pendingH) {
    pendingH = h;
    pendingT = Date.now();
  }
  if (timer) clearTimeout(timer);
  timer = setTimeout(() => {
    timer = null;
    if (Date.now() - pendingT < 140) {
      hug(pendingH); // still settling — re-arm
      return;
    }
    lastH = pendingH;
    Window.setFrame({ height: pendingH + STRIP, anchorTop: true, animated: true });
  }, 150);
};
mount(() => (
  <View
    style={{
      flex: 1,
      // Follows the system appearance — a resize or a resize gap shows a
      // themed surface, never a black void behind a light card.
      backgroundColor: colorScheme() === "dark" ? "#161616" : "#ffffff",
      overflow: "hidden",
    }}
  >
    <View style={{ paddingTop: STRIP }}>
      <View onLayout={(e) => hug(e.height)}>
        <R />
      </View>
    </View>
    {/* The version badge, top right in the title strip: which version
        of the app this window runs. */}
    {${JSON.stringify(badge)} ? (
      <View
        style={{
          position: "absolute", top: 6, right: 8, height: 17, paddingHorizontal: 7,
          borderRadius: 8.5, justifyContent: "center",
          backgroundColor: colorScheme() === "dark" ? "rgba(245,245,245,0.10)" : "rgba(28,28,28,0.07)",
        }}
      >
        <Text
          style={{
            fontSize: 11, fontWeight: "600",
            color: colorScheme() === "dark" ? "rgba(245,245,245,0.75)" : "rgba(28,28,28,0.65)",
          }}
        >
          {${JSON.stringify(badge)}}
        </Text>
      </View>
    ) : null}
    {/* The drag holdout: invisible, and above the strip — presses on the
        top 26pt move the window instead of reaching the app, so canvas
        and pressable apps stay movable. */}
    <View
      style={{ position: "absolute", top: 0, left: 0, right: 0, height: STRIP }}
      desktop={{ windowDrag: true }}
    />
  </View>
));
