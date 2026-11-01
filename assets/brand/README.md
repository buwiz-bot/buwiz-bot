# Brand mascot

The default bot (the white `blob`) at rest, looking ahead, on a
transparent ground. The app's avatar (`app/src/mascot/`) follows this art:
a round ink body with two slanted pill eyes cut out of it, sitting up and
to the right.

- `mascot-light.svg`, `mascot-light-{256,512,1024}.png`: black ink, for
  light backgrounds (what a light theme draws).
- `mascot-dark.svg`, `mascot-dark-{256,512,1024}.png`: white ink, for
  dark backgrounds.

To see the app's drawing of every shape without running the app, render
the offscreen preview from `app/`:

```sh
MASCOT_PREVIEW_DIR=<dir> cargo test -p rox --lib mascot::tests::preview -- --ignored
```
