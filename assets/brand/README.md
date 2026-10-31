# Brand mascot

Generated files: don't edit them by hand. They are the default bot (the
white `blob`) at rest, looking ahead, drawn from the same baked outline
the app paints (`app/src/mark/paint.rs`'s `rest_rings`), on a transparent
ground. The bake is a coarse polygon, so its rings are drawn as quadratic
curves through their edges' midpoints (`app/src/mark/brand.rs`).

- `mascot-light.svg`, `mascot-light-{256,512,1024}.png`: black ink, for
  light backgrounds (what a light theme draws).
- `mascot-dark.svg`, `mascot-dark-{256,512,1024}.png`: white ink, for
  dark backgrounds.

Regenerate them from `app/`:

```sh
cargo test -p rox --lib brand::export -- --ignored
```

`brand::svgs_are_current` (part of `cargo test -p rox`) fails when the
SVGs here no longer match the bake.
