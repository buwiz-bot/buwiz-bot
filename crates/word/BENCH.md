# Office: the Word document's numbers

A generated 200-page document (each page a heading, three paragraphs with
bold runs and one footnote, a three-item list, and a 3×3 table or a
picture; Arial), on an Apple-silicon Mac, measured by `src/bench.rs`:

```
cargo test -p buwiz-word --lib bench::pages_200 -- --ignored --nocapture --exact
cargo test -p buwiz-word --lib bench::typing_200 -- --ignored --nocapture --exact
cargo test -p buwiz-word --lib bench::parts_200 -- --ignored --nocapture --exact
```

Buwiz Bot's dev profile builds every dependency at `opt-level = 3`, so the engine
runs at release speed in these tests. The machine was shared with other
builds while these ran: runs vary by 2–3×. The table gives the quietest
run, the range of the others in brackets.

## The engine as it comes

BetterOffice's Word engine is `docx-edit`'s `EngineSession`, the one its web
editor runs in wasm. Its host talks JSON: a region request (sections,
settings, notes) and, for measuring, the font chains each
`family|bold|italic` key maps to. It seeds the whole package into its
stories, paginates, and builds each page's display list (glyph runs with
every glyph placed, rects, lines, images, shapes, decorations).

| | 200 pages |
|---|---|
| parse the package (for the request: sections, notes, comments) | 41 ms [41–51] |
| seed the whole document | 190 ms [190–605] |
| seed a preview (first 40–80 blocks) | 19–30 ms [≤74] |
| the first page, laid out and listed (prefix pass) | 16–39 ms [≤126] |
| the whole document laid out, 3 pages listed | 446–517 ms [≤669] |
| one more page's display list (incremental, its frame) | 12.8 ms median, 40 ms p95 [≤14 / 49] |
| a page's canvas drawing | 1.2 ms median, ~1,000 operations |
| peak RSS: laid out / every page listed | +366 MB / +492 MB [≤738] |

Two traps, both avoided in the view:

- **Frame epochs.** Every display-list call takes the last frame the host
  applied. Passing anything else (0) makes it a full recovery frame: each
  page then cost 312 ms and memory reached 1 GB. The view reads the epoch
  from each frame's header and passes it back (`layout.rs`).
- **A full rebuild.** `build_display_list_frame` outside an edit rebuilds
  every page's display input (1.3–1.7 s on 200 pages, windowed or not).
  The view calls it once, after the full layout, with a window of the pages
  in view, then builds pages as they come near (`build_display_pages_frame`).

## Fonts against CoreText

The shaper (rustybuzz over skrifa) is handed the very face the canvas
draws: the family is resolved as nsh's canvas resolves a CSS font, and that
face's tables become the sfnt the shaper reads (`fonts.rs`; a face of a
collection works too). A family that isn't installed takes an installed
stand-in on both sides (Calibri and Aptos → Arial, Cambria → Times New
Roman, Consolas → Courier New).

Every word of the first three pages, shaped width against CoreText's width
of the same word in the same face: **2,827 words, mean difference 0.000 px,
worst 0.001 px.** A page draws a word at a time, each at its first glyph's
place, so a justified line's stretched spaces land where the layout put
them.

Handing a face over costs ~10 ms. CoreText's first lookup in a process
costs about a second, but Buwiz Bot has paid it long before a document opens.
A wide fallback face (Arial Unicode MS) costs most of a second, so chains
hold the face alone: text in a script the face lacks is measured with its
missing glyphs while CoreText draws it from a fallback.

## Pictures

An inline picture comes through the display list as an image primitive
whose relationship id is the picture itself as a `data:` URL (192×96 for
the fixture's 2×1-inch picture). The view shows it as an nsh image view
over the page canvas; a relationship id that names a part is read from the
package (`save.rs`).

## What the view does with it

| | target | with footnotes | without |
|---|---|---|---|
| first page drawn (`Word::open` + first tick: parse, preview seed, prefix layout, list, drawing) | < 200 ms | 110–178 ms [≤255] | 75–82 ms [≤232] |
| the whole document after that (second tick) | | 434–930 ms | 420–465 ms |
| typing echo (key, relayout from its paragraph, pages in view listed and drawn) | < 16 ms | 113–118 ms [≤193] | 59–68 ms [≤140] |
| of which the engine's relayout and lists | | 66 ms | 19 ms |

- **First page.** The first tick seeds a preview of the first blocks,
  lays out a one-page prefix and draws it; the second seeds the whole
  document and lays it out.
- **Scrolling** never repaints a page canvas (see `view.rs`): a page's
  drawing is made once per page, layout and zoom on the worker, a scroll
  moves page views and only asks for pages coming near, and a relayout
  sends a page again only when its display list changed.
- **Typing misses its target.** The engine's own fast path for a key (a
  local patch of the lowered body) is reached only through
  `EngineSession::edit_resident_text`, which docx-edit 0.5.1 keeps
  `pub(crate)` (its wasm bindings use it). An edit made through
  `EditingDoc` re-lowers the whole body: 47 ms on 200 pages, 105 ms when
  the document has footnotes (notes take the engine off its resident
  path). The view's own share (the index, the caret, the drawing of the
  edited page) is a few ms.

## Fonts an edit brings in

The request's font chains were filled once, at open, from the keys the
text needed then. Bold or italic on a face the text had only upright
(`calibri light|1|1` in a Title) made a key with no chain: the engine
measured that text one em a character and listed it as plain text, so the
selection (built from the measured width) ran far past the drawn glyphs.
Each edit now asks the engine for its font requirements first, as the web
editor does before every relayout, and lays out with a grown request when
a key is new (`layout.rs` `fonts_grew`). The check reads every story:
40–45 ms on 200 pages. Plain typing (a key, Backspace, Delete, a one-line
paste) takes the fonts of the text it lands in, so it skips the check; a
format, Enter, a paste of lines, a row, undo and redo run it.

## Local lowering (`EngineSession::set_local_lowering`)

Off by default in docx-edit 0.5.1: an eligible resident text edit
re-lowers only its paragraph. `typing_200` with it off and on, three runs
each (a shared machine; the "on" runs came second):

| typing echo, median [p90] | with footnotes | without |
|---|---|---|
| off | 116.4 [128], 112.9 [148], 117.9 [141] ms | 68.4 [93], 60.5 [70], 58.7 [69] ms |
| on | 138.0 [180], 120.2 [134], 235.0 [407] ms | 104.7 [303], 68.7 [82], 91.5 [118] ms |
| engine relayout + lists, off / on | 66–67 / 63–81 ms | 19–28 / 14–29 ms |

No run was faster with it on, so it stays off. Edits made through
`EditingDoc` don't reach the engine's resident text path
(`edit_resident_text` is `pub(crate)`), which is the one local lowering
serves.

## The thumbnail strip

`cargo test -p buwiz-word --lib bench::typing_200_strip -- --ignored --nocapture --exact`
is the typing echo above with the strip showing eight pages around the
caret (three of them the pages in view); `bench::strip_reflow_200` is an
edit that moves every later page, with the strip showing eight pages far
from it.

- A thumbnail is the page's display list drawn at 96 points wide
  (`draw::page` at a scale; no layout). It is the page's drawing only:
  the pictures on a page (image views over the page canvas) are not in its
  thumbnail, as in the strip that was removed in #58.
- The strip's ask (`Cmd::Thumbs`) does little itself: it draws the first
  three of the window's pages (their lists built in one call) so the strip
  does not show empty, and leaves the rest to the same one-page-a-tick pass
  the pause uses. `bench::strip_show_200`, a window of 20 pages far from the
  caret: the command 21 to 25 ms; the other 17 pages 118 to 130 ms in ticks
  of at most 11 ms each (a page 2 to 8 ms). A key that comes right after
  the command waits for the command (5 to 27 ms) and one tick (0 to 7 ms):
  its echo is 61 to 66 ms without footnotes (52 plain) and 98 ms with (91
  plain), the very first key of the document 153 ms with footnotes. A new
  ask drops what was left of the last. A scroll by one page: 1 ms and
  one tick. None of it is on the typing path, and a scroll changes no page,
  so it schedules no later check.
- A key sends no thumbnail. 150 ms after the last edit (`THUMB_SETTLE`)
  the worker brings every thumbnail the strip shows up to date, one page a
  tick (`tick`): it builds the page's list if it is not built, draws it and
  sends the thumbnail only if its drawing differs from the one it has (text
  typed earlier shifts the offsets in every later page's list, so the lists
  are not compared alone). A key that comes meanwhile clears the pass and
  starts the wait again; it waits for the tick under way, a page (2 to 9 ms),
  and no more.
- A plain key: the pass is 8 ticks, 12 to 20 ms in all without footnotes, 27
  to 46 ms with, one thumbnail drawn again (the edited page's).
- An edit that moves every later page (a page break, `strip_reflow_200`):
  the command 219 ms (283 with footnotes), as it is without the strip; the
  pause check 78 ms (79 with footnotes) in all, worst tick 14.6 ms (13.7),
  all 8 thumbnails drawn again. A key right after a pass's first tick: echo
  62.7 ms without footnotes (51.6 plain), 94.3 with (90.8 plain).
- The worker keeps the thumbnails of the strip's window and two pages
  beyond it, and the view the same: a thumbnail the strip has left is
  dropped on both sides.
- Typing echo with the strip showing: 95.2 ms with footnotes (90.8 without
  the strip, same session), 54.6 ms without (51.6).
