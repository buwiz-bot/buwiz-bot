# buwiz-office-kit

What Buwiz Bot's native office views share, as a Rust library on
[nsh](https://github.com/longbridge/nsh):

- `ui`: the controls, drawn and behaving as BetterOffice's web editor does
  them (buttons, dropdowns, colour pickers, menus and the menu bar, the
  table grid, dialogs and their fields, the icon font);
- `draw`: the spreadsheet's display lists on the nsh canvas;
- `fonts`: BetterOffice's bundled office fonts (registered with CoreText),
  the chains a family resolves to, and the installed face behind a family;
- `session`: the worker thread an open document lives on (commands in,
  versioned states and answers out, atomic saves, the file watched);
- `prompts`, `clipboard`: the bar asking about a document's edits, and the
  pasteboard;
- `host`: what the views need of the app that hosts them;
- `testing` (feature `testing`): a headless driver for the views' tests.

Word (`buwiz-word`) and the spreadsheet (`buwiz-spreadsheet`) are built on
it.

## The host interface

The app owns the open document's lifetime: it routes a file to its view,
opens and closes the document's `Session`, answers the close prompt and
uploads a save. It gives the views what they need once, at startup:

```rust
buwiz_office_kit::host::install(app);            // impl Host
buwiz_word::FORMAT.install(Hooks { ui, with_session });
buwiz_spreadsheet::FORMAT.install(Hooks { ui, with_session });
```

`Host` (`src/host.rs`) covers the open document (`showing`, `state`,
`is_word`, `ask_close`, `may_close`, `close`, `answer_close`, `save`,
`reload`, `keep_mine`), the file viewer around it (`viewing_name`,
`close_viewer`, `viewer_save`, `viewer_print`), and the rest of the app
(`open_link`, `print`, `toast`, `menu_open`, `context_menu`, `theme`).
A format's `Hooks` say where its view state is kept (the app's store) and
run a closure with the open document's session.

The app also calls into the kit: `fonts::register` and
`ui::icons::font_data` at startup, for the office and icon fonts.

## Tests

```
cargo test -p buwiz-office-kit
```

The fonts' tests read BetterOffice's font files: `ROX_OFFICE_FONTS`, or the
checkout's `server/node_modules/@betteroffice/fonts/assets`.

## Licence

No licence is granted: the same terms as the Buwiz Bot repository it comes
from, which has no licence file (all rights reserved).
