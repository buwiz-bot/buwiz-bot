# buwiz-spreadsheet

Buwiz Bot's spreadsheet view, as a Rust library on
[nsh](https://github.com/longbridge/nsh): `.xlsx`/`.xlsm`, `.xls`, `.csv`
and `.tsv` opened, edited and saved natively. The workbook lives on the
office worker (`buwiz_office_kit::session`) in BetterOffice's engine
(`doc`); this side keeps the shape of the sheet, the display list in view,
the selection and the cell being edited (`SheetUi`), and draws the grid,
the charts, the cell editor and the formula bar (`grid`, `chart`,
`resize`, `edit`). `BENCH.md` has its numbers.

## The host interface

The app hosts the view through `buwiz-office-kit`'s `host` (see its
README) and gives this crate, once at startup, its `FORMAT` hooks: where
the `SheetUi` is kept, and the open workbook's `Session<Book>`.

```rust
buwiz_spreadsheet::FORMAT.install(Hooks { ui: || &store().office.sheet, with_session: |f| with_session(|s| f(s)) });
```

The app shows `edit::sheet_view()` in its viewer, binds its keys to the
sheet's commands (`edit::walk`, `edit::copy`, `chart::nudge`, …) while
`edit::navigating()`, `edit::editing()` or `chart::selected()` holds, runs
its context menu's rows (`act` `sheet`) through `edit::action`, and passes
the worker's answers to `on_out`.

## Tests

```
cargo test -p buwiz-spreadsheet
```

Some read the showcase workbook from the Buwiz Bot checkout
(`server/src/office/seeds/`). The feature `testing` gives the app's tests
the test workbook (`fixture`).

## Licence

No licence is granted: the same terms as the Buwiz Bot repository it comes
from, which has no licence file (all rights reserved).
