# Brand mascot and logo

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
cargo test -p buwiz-bot --lib brand::export -- --ignored
```

`brand::svgs_are_current` (part of `cargo test -p buwiz-bot`) fails when the
SVGs here no longer match the bake.

## Logo

The Buwiz logo is Wiz's grin hat: a wizard hat with Wiz's resting eyes and
a smile, leaning back. `app/resources/AppIcon.svg` sets it on the green
square; these files are the hat alone on a transparent ground, with the eyes
and the smile cut through.

- `logo-light.svg`, `logo-light-{256,512,1024}.png`: black, for light
  backgrounds.
- `logo-dark.svg`, `logo-dark-{256,512,1024}.png`: white, for dark
  backgrounds.

The SVGs are the source. Render the PNGs from them with `rsvg-convert`:

```sh
for v in light dark; do for s in 256 512 1024; do
  rsvg-convert -w $s -h $s logo-$v.svg -o logo-$v-$s.png
done; done
```

## Wiz's avatar

`wiz-avatar.json` is Wiz's avatar as the app saves it (`outline`, `color`,
`layers`, `eyes`, `solids`): the hat as a custom body with the smile as a
layer, its eyes placed on the cone, and the brim a lathe solid. Send it to
the broker's `/bot/update` with the bot's `id` and `"shape": "custom"`, or
draw it with `app/tools/mark-sheet`.
