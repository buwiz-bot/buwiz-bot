# Make cards fast

An answer card (spec 9.8) mounts under the assistant's reply the moment it
arrives. Rox does most of the work to make that instant (spec 14.6, "Host
caching and prewarm"); this page is what your plugin should do so the rest is
instant too.

## What Rox does for you

- **Compiled cache.** Rox keeps each app's checked and compiled module in
  memory and on disk, keyed by your app's **signed manifest fingerprint** and
  the Rox build. An open with a cached module skips the source fetch and the
  compile.
- **Prewarm.** When your plugin starts at a new generation (install, update,
  restart) and shortly after Rox starts, Rox compiles every app whose
  `placement` includes `inline`, one at a time in the background.
- **Session pre-open.** When an answer card arrives, Rox opens its session
  while it prepares the module.

You do nothing to get these. A new version with a changed app source has a new
fingerprint: its first open compiles again (or prewarm already did).

## What you should do

1. **Render from the snapshot.** Put the data the answer was based on in
   `_meta["rox/open"].props.snapshot` (with `as_of`, unix ms) and draw it on the
   first render. Call tools only when the person presses Refresh: Rox then
   mounts the card again without `snapshot` and `as_of`, and the card reads
   live. A card that calls three tools before it draws anything costs three
   round trips through the plugin on every open.
2. **No work at module top level.** The module is evaluated on every open.
   Build lookup tables, parse data and call tools inside the component (and
   after the first render), never at the top of the file.
3. **Small sources.** Sources are compiled and their imports checked on a
   cache miss; keep each app to what it shows. Share code through the
   plugin's tools, not by bundling it into every app. The limit is 512 KiB;
   cards should be a few KiB.
4. **Declare the inline size.** Give `inline.width` and `inline.height` in the
   manifest (5.5). A card with its size known draws once, at that size,
   instead of growing as it lays out.
5. **Keep props small.** The snapshot counts against the 8 KiB props limit;
   send what the card shows, not the whole record.
6. **Change the fingerprint only when the source changes.** Every new
   fingerprint is a cache miss for every user on first open. Do not stamp
   build times or random values into app sources.

## Reading the timing log

Rox logs each open at info level (`RUST_LOG=info`):

```
mini v1 timing: dev.goldcoders.bir/deadlines cache hit 0 ms (no fetch, no compile)
mini v1 timing: dev.goldcoders.bir/deadlines plugin list 3 ms
mini v1 timing: dev.goldcoders.bir/deadlines session open 0 ms (pre-opened)
mini v1 timing: v1:dev.goldcoders.bir/deadlines?ticket=… view to first frame 9 ms (session, engine start, module, layout)
```

| Line | What it measures | If it is large |
|---|---|---|
| `source fetch` | Reading the app's source from your plugin (a miss only) | Your server is slow to answer `resources/read`; keep the source in memory. |
| `checks+compile` | The import check and the compile (a miss only) | The source is large; split it. |
| `cache hit` | The cached module was used | — |
| `session open` | The signed session (4.4); `pre-opened` when Rox opened it ahead | Your `rox.session.open` is slow; it must only sign. |
| `view to first frame` | From the view to the first laid-out frame: the session, engine start, module evaluation and the first layout | Top-level work in the module, or a first render that waits on tools. |
| `prewarmed in` | A background compile after your plugin started | — |

Measured in Rox's development build (BIR, October 2026): a cold open of the
Deadlines card took about 50 ms of host time (fetch 26, compile 5, session
2, first frame 8); with the cache and a pre-opened session about 12 ms. What
remains is your card's own tool calls: render from the snapshot.
