# buwiz-word

Buwiz Bot's Word view, as a Rust library on
[nsh](https://github.com/longbridge/nsh): `.docx` (and legacy `.doc`,
imported) opened, edited and saved natively. The document lives on the
office worker (`buwiz_office_kit::session`) in BetterOffice's editing
engine (`doc`, `layout`); its pages are drawn there from the engine's
display lists (`draw`) with the faces it measured with (`fonts`), and
`view`, `chrome`, `toolbar` and `dialogs` show them, with the web
editor's formatting bar, menus and dialogs. `BENCH.md` has its numbers.

## The host interface

The app hosts the view through `buwiz-office-kit`'s `host` (see its
README) and gives this crate, once at startup, its `FORMAT` hooks: where
the `WordUi` is kept, and the open document's `Session<Word>`.

```rust
buwiz_word::FORMAT.install(Hooks { ui: || &store().office.word, with_session: |f| with_word(|s| f(s)) });
```

The app shows `view::word_view()` in its viewer, binds its keys to the
view's commands (`view::key`, `view::step`, `view::format`, …) while
`view::editing()` holds, and passes the worker's answers to `on_out`.

## Tests

```
cargo test -p buwiz-word
```

Some read the demo document from the Buwiz Bot checkout
(`server/src/office/seeds/`). The feature `testing` gives the app's tests
the test documents (`fixture`).

## Licence

No licence is granted: the same terms as the Buwiz Bot repository it comes
from, which has no licence file (all rights reserved).
