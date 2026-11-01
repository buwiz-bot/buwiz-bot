# App icons

The app's icon is the Buwiz logo: buwiz.com's white hand-drawn "b" mark on
its green square (`#0f5b46`, corners of 22% of the side), placed as the
site's own 512 px icon places it.

- `AppIcon.svg`: the vector source, 1024x1024.
- `AppIcon.png`: the 1024x1024 master rendered from it.
- `AppIcon.icns`: every iconset size (16, 32, 128, 256, 512 and their @2x),
  each rendered from the vector. `app/bundle-dev.sh` and
  `scripts/release/build.sh` put it in the bundle as
  `Contents/Resources/AppIcon.icns`.
- `AppIcon-test.svg`, `AppIcon-test.icns`: the same with an orange TEST
  band, the icon of the test bundles `scripts/buwiz-test` makes, so a test
  instance never looks like the real app in the Dock or the app switcher.

The PNG and the icns files are generated: edit the SVGs, then run
`scripts/make-app-icon` (it needs `rsvg-convert`, from librsvg) and commit
what it writes.

## Trademark

The Buwiz name and logo are trademarks of their owner. They are not
licensed with the code: the licence the code is released under (Apache-2.0,
whose §6 says as much) grants no right to use the Buwiz name or logo, these
files included, as a logo or mark for anything else. A fork that ships
must replace them with its own icon.
