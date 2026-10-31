# Office: the spreadsheet's numbers

Generated workbooks (a quarter of the cells text, the rest numbers), on an
Apple-silicon Mac, measured by `src/bench.rs`:

```
cargo test -p buwiz-spreadsheet --lib bench::open_100k -- --ignored --nocapture --exact
cargo test -p buwiz-spreadsheet --lib bench::open_1m -- --ignored --nocapture --exact
cargo test -p buwiz-spreadsheet --lib bench::edit_10k_formulas -- --ignored --nocapture --exact
```

Buwiz Bot's dev profile builds every dependency at `opt-level = 3`, so the engine
runs at release speed in these tests. A window is 60 rows × 40 columns.
The drawing step uses a stand-in text measurer; on screen the platform
measures.

## Step 0: BetterOffice as it comes

`Workbook::open` builds a collaboration document (yrs) of the whole model
and fingerprints it before it returns, even for a single local user. Parsing
alone costs a fraction of that:

| | 100k cells | 1M cells |
|---|---|---|
| `Workbook::open` | 679 ms | 10.4 s |
| `Workbook::open_for_read` | 487 ms | — |
| parse with the saving package (`parse_workbook_with_owned_package`) | 168 ms | 1.69 s |
| parse, model only (`parse_workbook`) | 122 ms | — |
| `GridGeometry::new` | 5.5 ms | 59 ms |
| a 60×40 display list | 1.5–1.7 ms | 1.2–2.2 ms |
| RSS after `Workbook::open` | +142 MB | +697 MB (peak 897 MB) |

On a 10k-formula sheet (`C = A*2` down 10k rows, `E1 = SUM(C)`), `Workbook`
recalculates an edit and builds a window in 0.8 ms (median; 2.4 ms max), so
the engine itself is not the limit for editing.

## What the view does with it

The first paint does not wait for `Workbook::open`
(`src/doc.rs`): it parses the model only, and only the top 256 KB
of each worksheet's rows, draws, and then a helper thread parses the whole
model (the view switches to it) and builds the editable `Workbook`. Edits
made before that wait for it and apply in order. A canvas drawing sets the
font, color and alignment only when they change and clips only text that
overflows its cell (22,073 operations for a window became 3,167).

| | 100k cells | 1M cells |
|---|---|---|
| first paint (parse + window + drawing) | **81 ms** | 714 ms |
| of which: parse the top rows | 75 ms | 709 ms |
| the window's display list | 3.5 ms | 2.9 ms |
| its canvas drawing | 2.4 ms (3,167 ops) | 2.1 ms |
| editable after | 680 ms | 7.1 s |
| scrolled windows, whole sheet in (median / max) | 1.5 / 7.3 ms | 1.3 / 75.6 ms (the first one builds the engine's geometry) |
| peak RSS | 171 MB | 1.5 GB |

10k formulas: editable after 408 ms; an edit, its recalculation, the
window's display list and its drawing take 1.9 ms (median; 3.2 ms worst of
50 edits; an earlier run had one 21.6 ms outlier); undo 19 ms; save 23 ms.
Runs vary by 10–20%.

## Against the targets (#34, step 7)

| Target | Measured | |
|---|---|---|
| 100k cells reach first paint in under 150 ms | 81 ms | met |
| edit, recalc and redraw under 16 ms on a 10k-formula sheet | 1.9 ms median, 3.2 ms worst of 50 | met (one 21.6 ms outlier in another run) |
| scrolling shows no jank | a scroll moves the canvases (an offset); a new window is drawn only past a 320 px margin, 1.3–1.5 ms on the worker | to check on screen (jank monitor) |

For 1M cells the parse of the shared strings dominates the first paint
(the top rows are cut, the string table is not), and the full engine takes
7–9 s and over a gigabyte: viewing is fine, editing a sheet that size waits.
