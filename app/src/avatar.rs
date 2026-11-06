//! The agent avatar (`app/blob.tsx`) — a native `canvas()` port of `BotBlob`:
//! the `Mark`/`Director` engine from `mark.rs` advances on a 60fps clock
//! signal, and each paint either draws the live `Uh` tree through
//! `paint_tree` or — for a settled resting mark — the baked `paint_rest`
//! frames. `blobGaze`/`blobPokes`/`blobEdit` keep their JS shapes.

use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::time::{Duration, Instant};

use nsh_core::prelude::*;

use crate::mark::{self, PaintOpts, UhSize, V2};
use crate::mark::{gaze_advance, gaze_set, GazeState};
use crate::store::RoxBot;
use crate::theme;

// ---------------------------------------------------------------------------
// `blobGaze`/`blobPokes`/`blobEdit`
// ---------------------------------------------------------------------------

/// The `v` signal of `blobGaze`/`blobEdit`/`blobPokes`: a resting canvas
/// subscribes to it to wake (blob.tsx's `gaze?.v()` in the not-live
/// branch). Only an armed wake writes — a live canvas reads the latest
/// value untracked on its next frame, so a mousemove is never a commit.
struct Wake {
    v: Signal<u32>,
    armed: Cell<bool>,
}

impl Wake {
    fn new() -> Self {
        Self {
            v: signal(0),
            armed: Cell::new(false),
        }
    }
    fn ping(&self) {
        if self.armed.replace(false) {
            self.v.try_set(self.v.get_untracked().wrapping_add(1));
        }
    }
    /// Subscribes the running binding to the next `ping`.
    fn arm(&self) {
        self.armed.set(true);
        let _ = self.v.try_get();
    }
}

/// `blobGaze()` — the pointer-follow target shared between the outer view
/// (`follow`) and the canvas (`target`/`ver`). Cells the canvas reads
/// untracked, plus the wake a resting canvas sleeps on.
pub(crate) struct BlobGaze {
    target: Cell<Option<V2>>,
    ver: Cell<u32>,
    wake: Wake,
}

impl BlobGaze {
    pub(crate) fn new() -> Rc<Self> {
        Rc::new(Self {
            target: Cell::new(None),
            ver: Cell::new(0),
            wake: Wake::new(),
        })
    }
    fn change(&self, next: Option<V2>) {
        if self.target.get() == next {
            return;
        }
        self.target.set(next);
        self.ver.set(self.ver.get() + 1);
        self.wake.ping();
    }
    /// Pointer moved inside the mark cell: `follow(event, cx, cy, reach=60)`.
    pub(crate) fn follow(&self, event_x: f64, event_y: f64, cx: f64, cy: f64, reach: f64) {
        let clamp = |v: f64| v.clamp(-1.0, 1.0);
        self.change(Some([
            clamp((event_x - cx) / reach),
            clamp(-(event_y - cy) / reach),
        ]));
    }
    pub(crate) fn rest(&self) {
        self.change(None);
    }
    pub(crate) fn target(&self) -> Option<V2> {
        self.target.get()
    }
    pub(crate) fn ver(&self) -> u32 {
        self.ver.get()
    }
}

/// `blobPokes()` — queued `poke(kind, {turns})`s the canvas drains on its
/// next clock tick (≤33ms, like the TS loop reading `pokes.q`).
pub(crate) struct BlobPokes {
    queue: RefCell<Vec<(&'static str, f64)>>,
    wake: Wake,
}

impl BlobPokes {
    pub(crate) fn new() -> Rc<Self> {
        Rc::new(Self {
            queue: RefCell::new(vec![]),
            wake: Wake::new(),
        })
    }
    pub(crate) fn fire(&self, kind: &str, turns: Option<f64>) {
        let kind = mark::UN.iter().copied().find(|k| *k == kind).unwrap_or(mark::UN[0]);
        self.queue.borrow_mut().push((kind, turns.unwrap_or(1.0)));
        self.wake.ping();
    }
    fn drain(&self) -> Vec<(&'static str, f64)> {
        self.queue.borrow_mut().drain(..).collect()
    }
}

/// `createAnimatedValue` for style-bound fades (opacity/transform, not
/// canvas): a signal stepped toward `target` every 16ms, reaching it in
/// `ms` milliseconds — the TSX's `panelEditK.timing(1, {duration:220})`.
/// The stepping task runs only while the value is moving.
pub(crate) struct FadeValue {
    v: Signal<f32>,
    target: Cell<f32>,
    ms: Cell<f32>,
    running: Cell<bool>,
    owner: Owner,
}

impl FadeValue {
    pub(crate) fn new() -> Rc<Self> {
        Rc::new(Self {
            v: signal(0.0),
            target: Cell::new(0.0),
            ms: Cell::new(300.0),
            running: Cell::new(false),
            owner: Owner::current(),
        })
    }
    /// `timing(to, {duration: ms})`.
    pub(crate) fn timing(self: &Rc<Self>, to: f32, ms: f32) {
        self.target.set(to);
        self.ms.set(ms);
        if self.running.replace(true) {
            return;
        }
        let f = self.clone();
        self.owner.run(|| {
            spawn(async move {
                loop {
                    let (v, target) = (f.v.get_untracked(), f.target.get());
                    if (target - v).abs() <= 0.002 {
                        break;
                    }
                    let step = (16.0 / f.ms.get().max(16.0)).min(1.0);
                    f.v.set(v + (target - v).signum() * step.min((target - v).abs()));
                    timer(Duration::from_millis(16)).await;
                }
                f.running.set(false);
            })
        });
    }
    pub(crate) fn get(&self) -> f32 {
        self.v.get()
    }
}

/// `blobEdit()` — the picker's drag-to-recolor amount (`s.target`), a cell
/// the clock-tick paint reads like the TS `editK` ref.
pub(crate) struct BlobEdit {
    pub(crate) target: Cell<f64>,
    wake: Wake,
}

impl BlobEdit {
    pub(crate) fn new() -> Rc<Self> {
        Rc::new(Self {
            target: Cell::new(0.0),
            wake: Wake::new(),
        })
    }
    pub(crate) fn to(&self, target: f64) {
        self.target.set(target);
        self.wake.ping();
    }
}

// ---------------------------------------------------------------------------
// `BotBlob`
// ---------------------------------------------------------------------------

/// A designed body's shape string: `outline:` then its SVG path data (the
/// broker's `shape: "custom"` + `outline`, folded into one string so every
/// place that passes a shape around carries the outline with it).
pub(crate) const OUTLINE: &str = "outline:";

/// A rich (multi-colour) body's shape string: `rich:` then its recipe as
/// compact JSON `{"o": outline, "c": shell, "l": [[path, fill, opacity]],
/// "e": eyes}` (`mark::rich_body`). Its colours are its own: the colour
/// picker doesn't apply.
pub(crate) const RICH: &str = "rich:";

/// The colour effects and accents take from an avatar. A rich avatar paints
/// itself from its recipe, and its `color` is only that recipe's main colour,
/// often skin, so what isn't the body takes the primary ink instead. A plain
/// shape keeps the colour the person gave it.
pub(crate) fn accent_of<'a>(shape: &str, color: &'a str) -> &'a str {
    if shape.starts_with(RICH) { "#FFFFFF" } else { color }
}

/// The mark body for a shape string: a picker name, or `outline:<path>`
/// (registered once; a path the engine can't use draws the ball).
pub(crate) fn body_of(shape: &str) -> &'static str {
    if let Some(recipe) = shape.strip_prefix(RICH) {
        return mark::rich_body(recipe).unwrap_or("sphere");
    }
    match shape.strip_prefix(OUTLINE) {
        Some(path) => mark::custom_body(path).unwrap_or("sphere"),
        None => mark::yn(shape),
    }
}

/// Whether the engine can draw `shape` as itself.
pub(crate) fn drawable(shape: &str) -> bool {
    if let Some(recipe) = shape.strip_prefix(RICH) {
        return mark::rich_body(recipe).is_ok();
    }
    match shape.strip_prefix(OUTLINE) {
        Some(path) => mark::custom_body(path).is_ok(),
        None => mark::BOT_SHAPES.contains(&shape),
    }
}

/// `BotBlobProps` — `shape` is the picker name (`blob`, `cloud`, …) or
/// `outline:<path>`, `state` a `STATES` name, `paused` pins the resting
/// pose.
pub(crate) struct BotBlobProps {
    pub shape: String,
    pub color: String,
    pub size: f32,
    pub state: &'static str,
    /// A live state source (run status, `botSt`): read on every paint, so
    /// the canvas painter subscribes to whatever it reads. Wins over
    /// `state` when set.
    pub state_src: Option<Rc<dyn Fn() -> String>>,
    pub paused: bool,
    pub emphasized: bool,
    /// A live emphasis source (`emphBot`): read on every paint. Wins over
    /// `emphasized` when set.
    pub emph_src: Option<Rc<dyn Fn() -> bool>>,
    pub gaze: Option<Rc<BlobGaze>>,
    pub edit: Option<Rc<BlobEdit>>,
    pub pokes: Option<Rc<BlobPokes>>,
    pub morph_shape: bool,
    pub seed: Option<String>,
    pub gaze_at: Option<V2>,
}

impl Default for BotBlobProps {
    fn default() -> Self {
        Self {
            shape: "blob".into(),
            color: "#FFFFFF".into(),
            size: 22.0,
            state: "idle",
            state_src: None,
            paused: false,
            emphasized: false,
            emph_src: None,
            gaze: None,
            edit: None,
            pokes: None,
            morph_shape: false,
            seed: None,
            gaze_at: None,
        }
    }
}

/// The per-canvas engine + prop mirrors (`draw`'s locals in `BotBlob`).
struct Inner {
    tab: Rc<mark::Tables>,
    mark: mark::Mark,
    dir: mark::Director,
    gs: GazeState,
    last_shape: String,
    last_state: &'static str,
    last_compact: bool,
    last_emphasis: bool,
    last_resting: bool,
    last_gaze_ver: u32,
    was_playing: bool,
    live_until: Instant,
    grace_until: Instant,
    ek: f64,
    prev_clock: f64,
    asleep: bool,
}

impl Inner {
    fn new(props: &BotBlobProps, tab: Rc<mark::Tables>) -> Self {
        #[cfg(test)]
        BUILT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let body = body_of(&props.shape);
        let seed = props
            .seed
            .as_deref()
            .map(mark::rp)
            .unwrap_or_else(|| (std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.subsec_nanos())
                .unwrap_or(1)) | 1);
        let box_ = props.size as f64 * 0.82;
        let mut mark = mark::Mark::new(&tab, Some(body), 1.0, false, false, seed);
        // Untracked: callers build an avatar inside a `dynamic`, and a tracked
        // read here would rebuild every avatar on any Bot's status write,
        // starting its animation over. `sync`, in the paint, follows the state.
        let state = props
            .state_src
            .as_ref()
            .map(|f| mark::state_name(&untracked(|| f())))
            .unwrap_or_else(|| mark::state_name(props.state));
        let resting = props.paused || state == "idle";
        let dir = mark::Director::new(&mut mark, state, seed, box_ < 48.0, resting, None);
        let initial = props
            .gaze
            .as_ref()
            .and_then(|g| g.target())
            .or(props.gaze_at);
        mark.set_gaze(initial);
        let mut gs = GazeState::default();
        gaze_set(&mut gs, initial);
        let now = Instant::now();
        Self {
            tab,
            mark,
            dir,
            gs,
            last_shape: props.shape.clone(),
            last_state: state,
            last_compact: box_ < 48.0,
            last_emphasis: false,
            last_resting: resting,
            last_gaze_ver: props.gaze.as_ref().map_or(0, |g| g.ver()),
            was_playing: false,
            live_until: now,
            grace_until: now,
            ek: 0.0,
            prev_clock: 0.0,
            asleep: true,
        }
    }

    /// `draw`'s prop-diff section (blob.tsx: shape/state/compact/emphasis/
    /// gaze/pokes/resting), then the clock advance.
    fn sync(&mut self, props: &BotBlobProps) -> bool {
        let now = Instant::now();
        let mut dirty = false;
        if props.shape != self.last_shape {
            self.mark.set_body(body_of(&props.shape), false);
            if props.morph_shape {
                self.grace_until = now + Duration::from_millis(1400);
            }
            self.live_until = now + Duration::from_millis(1200);
            self.last_shape = props.shape.clone();
            dirty = true;
        }
        let state = props
            .state_src
            .as_ref()
            .map(|f| mark::state_name(&f()))
            .unwrap_or_else(|| mark::state_name(props.state));
        if state != self.last_state {
            if state == "idle" {
                self.grace_until = now + Duration::from_millis(1400);
            }
            self.dir.set_state_pub(&mut self.mark, state);
            self.last_state = state;
            dirty = true;
        }
        #[cfg(test)]
        if let Some(seed) = &props.seed {
            PAINTED.lock().unwrap().insert(seed.clone(), state);
        }
        let compact = (props.size as f64 * 0.82) < 48.0;
        if compact != self.last_compact {
            self.dir.set_compact(compact);
            self.last_compact = compact;
        }
        let emphasized = props
            .emph_src
            .as_ref()
            .map(|f| f())
            .unwrap_or(props.emphasized);
        if emphasized != self.last_emphasis {
            self.dir.set_emphasis(&mut self.mark, emphasized);
            self.live_until = now + Duration::from_millis(400);
            self.last_emphasis = emphasized;
        }
        if let Some(gaze) = &props.gaze
            && gaze.ver() != self.last_gaze_ver {
                let target = gaze.target().or(props.gaze_at);
                self.mark.set_gaze(target);
                gaze_set(&mut self.gs, target);
                self.last_gaze_ver = gaze.ver();
        }
        if let Some(pokes) = &props.pokes {
            for (kind, turns) in pokes.drain() {
                self.mark.poke(kind, turns);
                self.live_until = now + Duration::from_millis(1500);
            }
        }
        let resting = props.paused || (state == "idle" && now >= self.grace_until);
        if resting != self.last_resting {
            self.dir.set_resting(&mut self.mark, resting);
            self.last_resting = resting;
        }
        dirty
    }

    /// `draw`'s live branch: advance clocks, update `ek`, mirror the
    /// rest-pose hold reset.
    fn advance(&mut self, props: &BotBlobProps, clock: f64, resting: bool, live: bool) {
        let edit_target = props.edit.as_ref().map_or(0.0, |e| e.target.get());
        let mut dt = 0.0;
        if live {
            if self.asleep {
                self.prev_clock = clock;
                self.asleep = false;
            } else {
                dt = (clock - self.prev_clock).clamp(0.0, 0.1);
                self.prev_clock = clock;
            }
            if dt > 0.0 {
                if !resting {
                    self.dir.advance(&mut self.mark, dt);
                }
                self.mark.advance(dt);
                gaze_advance(&mut self.gs, dt);
                self.ek += (edit_target - self.ek) * (1.0 - (-14.0 * dt).exp());
            }
        } else {
            self.asleep = true;
        }
        let playing = self.mark.is_playing();
        if self.was_playing && !playing {
            self.gs.hold = 0.0;
        }
        self.was_playing = playing;
    }

    /// `draw`'s paint section.
    fn paint(&mut self, p: Painter, props: &BotBlobProps, resting: bool) -> Painter {
        let size = props.size as f64;
        let canvas_size = size * 1.5;
        let box_ = size * 0.82;
        let extent = (box_ * (0.92 / 0.6) * 100.0).round() / 100.0;
        let x = (canvas_size - extent) / 2.0;
        let y = x;
        // `inkOf` (blob.tsx:70): "#FFFFFF" is the adaptive swatch — black on
        // light themes. Reading `dark()` inside the painter subscribes it
        // to the env memo, so a theme flip repaints.
        let ink = mark::ink_of(accent_of(&props.shape, &props.color), !dark());
        let shade = 0.7 * self.ek;
        let ink_paint = if self.ek > 0.001 {
            mark::darken(&ink, shade)
        } else {
            ink.clone()
        };
        let playing = self.mark.is_playing();
        let now = Instant::now();
        if resting && !playing && self.last_state == "idle" && !self.last_emphasis && now >= self.live_until
            && let Some(bytes) = mark::rest_bytes(&props.shape) {
                return mark::paint_rest(p, &bytes, &self.gs, x, y, extent, &ink_paint);
        }
        let frame = self.mark.frame();
        let tree = mark::uh(
            &frame,
            &PaintOpts {
                ink: [0.0, 0.0, 0.0],
                spot: [0.0, 0.0, 0.0],
                cutout: true,
                ink_paint: Some(ink_paint),
                spot_paint: None,
                shade,
            },
            extent,
            "m",
            &UhSize {
                display_size: Some(box_),
                extent,
            },
            &self.tab,
        );
        mark::paint_tree(p, &tree, x, y, extent)
    }
}

/// How many avatars were built: each one starts its animation from the top.
#[cfg(test)]
pub(crate) static BUILT: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);

/// The state each seeded avatar last painted, for the tests to read.
#[cfg(test)]
pub(crate) static PAINTED: std::sync::Mutex<std::collections::BTreeMap<String, &'static str>> =
    std::sync::Mutex::new(std::collections::BTreeMap::new());

/// `BotBlob` — a `size`-cell `View` with a `size*1.5` canvas overhanging it.
pub(crate) fn bot_blob(props: BotBlobProps) -> impl IntoElement {
    let size = props.size;
    let canvas_size = size * 1.5;
    let offset = -size * 0.25;
    let st = Rc::new(RefCell::new(Inner::new(&props, mark::shared_tables())));
    let props = Rc::new(props);
    view()
        .size(px(size))
        .child(
            canvas(move || {
                let mut st = st.borrow_mut();
                let props = &*props;
                st.sync(props);
                let now = Instant::now();
                let resting = props.paused || (st.last_state == "idle" && now >= st.grace_until);
                let live = !resting
                    || !st.mark.is_settled()
                    || (props.edit.as_ref().map_or(0.0, |e| e.target.get()) - st.ek).abs() > 0.002
                    || st.grace_until > now;
                // Live: the shared clock repaints it next frame. Settled:
                // sleep until gaze, edit or a poke wakes it (blob.tsx's
                // not-live branch), so resting avatars cost no frames.
                let t = if live {
                    crate::live::store().tick()
                } else {
                    if let Some(g) = &props.gaze {
                        g.wake.arm();
                    }
                    if let Some(e) = &props.edit {
                        e.wake.arm();
                    }
                    if let Some(p) = &props.pokes {
                        p.wake.arm();
                    }
                    0.0
                };
                st.advance(props, t, resting, live);
                st.paint(Painter::new(), props, resting)
            })
            .size(px(canvas_size))
            .absolute()
            .left(px(offset))
            .top(px(offset)),
        )
}

/// `CroppedImage` — a bot's photo avatar: the image zoomed and panned by
/// `crop` inside a circle (blob.tsx:459).
pub(crate) fn cropped_image(uri: &str, size: f32, crop: Option<crate::store::Crop>) -> impl IntoElement {
    let zoom = crop.map(|c| c.s).unwrap_or(1.0).max(0.01) as f32;
    let (dx, dy) = crop.map(|c| (c.x as f32, c.y as f32)).unwrap_or((0.0, 0.0));
    view()
        .size(px(size))
        .rounded(px(size / 2.0))
        .overflow_hidden()
        .bg(theme::PILL_BG)
        .child(
            image(crate::files::media_url(uri))
                .absolute()
                .w(px(size * zoom))
                .h(px(size * zoom))
                .left(px((size - size * zoom) / 2.0 + dx * size))
                .top(px((size - size * zoom) / 2.0 + dy * size)),
        )
}

/// `BotAvatar`'s props (blob.tsx:500) — the mark or, when the bot has a
/// photo, the cropped image; `box_` is the photo's diameter
/// (`size*0.82` by default).
#[derive(Default)]
pub(crate) struct BotAvatarProps {
    pub shape: String,
    pub color: String,
    pub image: String,
    pub crop: Option<crate::store::Crop>,
    pub size: f32,
    pub box_: Option<f32>,
    pub state: &'static str,
    pub state_src: Option<Rc<dyn Fn() -> String>>,
    pub paused: bool,
    pub emphasized: bool,
    pub emph_src: Option<Rc<dyn Fn() -> bool>>,
    pub gaze: Option<Rc<BlobGaze>>,
    pub edit: Option<Rc<BlobEdit>>,
    pub pokes: Option<Rc<BlobPokes>>,
    pub morph_shape: bool,
    pub seed: Option<String>,
    pub gaze_at: Option<V2>,
}

impl BotAvatarProps {
    /// Filled from a catalog bot: shape, color, photo, seed.
    pub(crate) fn bot(b: &RoxBot, size: f32) -> Self {
        Self {
            shape: b.shape.clone(),
            color: b.color.clone(),
            image: b.image.clone(),
            crop: b.crop,
            size,
            seed: Some(b.id.clone()),
            ..Default::default()
        }
    }
}

/// `BotAvatar` — the mark cell: a `BotBlob` at `size` or, when `image`
/// is set, the `CroppedImage` photo at `box_` (blob.tsx:500).
pub(crate) fn bot_avatar(p: BotAvatarProps) -> impl IntoElement {
    let box_ = p.box_.unwrap_or((p.size * 0.82).round());
    view()
        .size(px(p.size))
        .items_center()
        .justify_center()
        .when(!p.image.is_empty(), |v| {
            v.child(cropped_image(&p.image, box_, p.crop))
        })
        .when(p.image.is_empty(), |v| {
            v.child(bot_blob(BotBlobProps {
                shape: p.shape,
                color: p.color,
                size: p.size,
                state: p.state,
                state_src: p.state_src,
                paused: p.paused,
                emphasized: p.emphasized,
                emph_src: p.emph_src,
                gaze: p.gaze,
                edit: p.edit,
                pokes: p.pokes,
                morph_shape: p.morph_shape,
                seed: p.seed,
                gaze_at: p.gaze_at,
            }))
        })
}

/// `SleepingBlob` (blob.tsx:399): the paused sleeping mark breathing
/// (opacity .85→1, scale 1→1.04 over 2.4s) plus three `z` glyphs that
/// rise and fade in a 2.4s loop, staggered by 800ms.
pub(crate) fn sleeping_blob(shape: &str, size: f32, ink: &str) -> impl IntoElement {
    // Same shared ~30fps tick `bot_blob` runs on — here it drives the
    // breathing + zzz phases directly on the style bindings.
    let clock = || crate::live::store().tick();
    let breath = move || 0.5 - 0.5 * (std::f64::consts::TAU * (clock() % 2.4) / 2.4).cos() as f32;
    let z_phase = move |i: usize| {
        let p = (clock() - i as f64 * 0.8) / 2.4;
        if p < 0.0 {
            0.0f32
        } else {
            (p % 1.0) as f32
        }
    };
    let ink_c = theme::tc(ink);
    // Resolved now (the `dark()` read rebuilds the enclosing `dynamic` on a
    // theme flip) — the blob painter's `ink_of` would leave a `tc` neutral
    // unmapped since it only special-cases the #FFFFFF swatch.
    let ink_s = if dark() {
        ink.to_owned()
    } else {
        theme::tc(ink).light.to_hex()
    };
    let z_tops = [-11.0f32, -14.0f32, -3.0f32];
    view()
        .size(px(size))
        .child(
            view()
                .size(px(size))
                .opacity(move || 0.85 + 0.15 * breath())
                .scale(move || 1.0 + 0.04 * breath())
                .child(bot_blob(BotBlobProps {
                    shape: shape.to_owned(),
                    color: ink_s.clone(),
                    size: size / 0.82,
                    state: "sleeping",
                    paused: true,
                    gaze_at: Some([0.0, 0.0]),
                    ..Default::default()
                })),
        )
        .children((0..3usize).map(|i| {
            let zp = z_phase;
            let zp2 = z_phase;
            let zp3 = z_phase;
            text("z")
                .text_size(px(11.0))
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(ink_c)
                .absolute()
                .left(px(size - 3.0 + i as f32 * 4.0))
                .top(px(z_tops[i]))
                .opacity(move || {
                    let p = zp(i);
                    if p < 0.25 {
                        p / 0.25
                    } else {
                        (1.0 - p) / 0.75
                    }
                })
                .translate_y(move || px(-12.0 * zp2(i)))
                .scale(move || 0.6 + 0.4 * zp3(i))
                .into_element()
        }))
}

#[cfg(test)]
mod tests {
    use super::{accent_of, body_of, drawable, OUTLINE, RICH};
    use crate::mark;
    use crate::store::{body_fields, body_shape};

    /// Effects leave a rich avatar's colour alone and take the primary ink;
    /// a plain shape keeps the colour the person gave it.
    #[test]
    fn accents_come_from_the_primary_ink_on_a_rich_avatar() {
        let rich = format!("{RICH}{{}}");
        assert_eq!(accent_of(&rich, "#F6D1AE"), "#FFFFFF");
        assert_eq!(mark::ink_of(accent_of(&rich, "#F6D1AE"), true), "#000000");
        assert_eq!(mark::ink_of(accent_of(&rich, "#F6D1AE"), false), "#FFFFFF");
        assert_eq!(accent_of(&rich, "#FC2C3E"), accent_of(&rich, "#14C370"));
        assert_eq!(accent_of("blob", "#FC2C3E"), "#FC2C3E");
    }

    const CAT: &str = "M16 46L20 10L40 27Q50 24 60 27L80 10L84 46Q90 62 84 75Q72 92 50 92Q28 92 16 75Q10 62 16 46Z";

    #[test]
    fn a_designed_outline_round_trips_between_the_broker_and_the_app() {
        let shape = body_shape("custom", CAT);
        assert_eq!(shape, format!("{OUTLINE}{CAT}"));
        assert_eq!(body_fields(&shape), serde_json::json!({"shape": "custom", "outline": CAT}));
        assert_eq!(body_fields("heart"), serde_json::json!({"shape": "heart"}));
        assert_eq!(body_shape("heart", ""), "heart");
        assert_eq!(body_shape("custom", ""), "blob");
        assert_eq!(body_shape("", ""), "blob");
    }

    #[test]
    fn a_rich_design_round_trips_and_draws_its_own_body() {
        use crate::store::shape_of;
        let v = serde_json::json!({
            "shape": "custom", "outline": CAT, "color": "#F08A24", "eyes": "#2B1D12",
            "layers": [{"path": "M34 64 C34 56 66 56 66 64 C66 76 58 82 50 82 C42 82 34 76 34 64 Z", "fill": "#FBEBD3"}],
        });
        let shape = shape_of(&v);
        assert!(shape.starts_with(super::RICH), "{shape}");
        assert!(drawable(&shape));
        let body = body_of(&shape);
        assert!(body.starts_with("rich:"), "{body}");
        assert_eq!(shape_of(&body_fields(&shape)), shape, "fields → shape is the same design");
        // with extra 3D parts (a hat), and placed eyes, the round trip holds
        let mut with_parts = v.clone();
        with_parts["eyes"] = serde_json::json!({"c": "#111111", "at": [50, 64], "gap": 24, "s": 1.2, "rim": "#FFFFFF", "rw": 0.6});
        with_parts["parts"] = serde_json::json!([{"outline": CAT, "color": "#F2C062", "z": 0.1, "d": 1.1,
            "layers": [{"path": "M30 20 L70 20 L70 27 L30 27 Z", "fill": "#E0393A", "opacity": 1.0}]}]);
        with_parts["layers"] = serde_json::json!([{"path": "M34 64 C34 56 66 56 66 64 C66 76 58 82 50 82 C42 82 34 76 34 64 Z", "fill": "#FBEBD3", "opacity": 1.0, "over": true}]);
        with_parts["eyes"]["shut"] = "r".into();
        let shape = shape_of(&with_parts);
        assert!(shape.contains("\"p\":"), "{shape}");
        assert_eq!(shape_of(&body_fields(&shape)), shape);
        // no layers: the plain outline
        assert_eq!(shape_of(&serde_json::json!({"shape": "custom", "outline": CAT})), format!("{OUTLINE}{CAT}"));
    }

    /// `save_designs` rewrites the whole list from `body_fields`: a solid
    /// dropped on that road would quietly destroy the avatar. Every field
    /// of every kind goes through, and the design still draws.
    #[test]
    fn solids_survive_the_design_round_trip() {
        use crate::store::shape_of;
        let solids = serde_json::json!([
            {"kind": "lathe", "color": "#F2C062", "at": [50, 40, 0], "tilt": [-7, 0, 3], "oval": 0.9, "teeth": [6, 0.5],
             "profile": [[0, 20], [20, 18], [30, 0], [44, -3], [30, -5], [0, -5]],
             "marks": [{"path": "M-180 40 L180 40 L180 55 L-180 55 Z", "fill": "#E0393A"}]},
            {"kind": "shell", "color": "#151515", "offset": 2.5, "edge": [[-180, 120], [0, 70], [90, 100]],
             "scallop": [9, 8, 1], "lobes": [12, 9.5, 1]},
            {"kind": "tube", "color": "#9A5B3A", "path": [[20, 30, 0, 3], [10, 15, 2, 1.5]],
             "branches": [[[14, 22, 1, 2], [4, 20, 4, 1]]]},
        ]);
        let design = serde_json::json!({
            "shape": "custom", "outline": CAT, "color": "#F6D1AE", "eyes": "#111111",
            "layers": [{"path": "M40 70 L60 70 L60 74 L40 74 Z", "fill": "#A8503A"}], "solids": solids,
        });
        let shape = shape_of(&design);
        assert!(drawable(&shape), "{shape}");
        let saved = body_fields(&shape);
        assert_eq!(saved["solids"], solids, "body_fields keeps them whole");
        assert_eq!(shape_of(&saved), shape, "the saved design is the same design");
        // through the list's JSON, as `/state/buwiz-avatar-designs` holds it
        let stored: serde_json::Value = serde_json::from_str(&serde_json::json!({"designs": [saved]}).to_string()).unwrap();
        assert_eq!(shape_of(&stored["designs"][0]), shape);
        // no solids: the key isn't there, and the recipe has none
        let mut plain = design.clone();
        plain.as_object_mut().unwrap().remove("solids");
        let shape = shape_of(&plain);
        assert!(!shape.contains("\"s\":"), "{shape}");
        assert!(body_fields(&shape).get("solids").is_none());
        // an empty list is no solids
        plain["solids"] = serde_json::json!([]);
        assert_eq!(shape_of(&plain), shape);
    }

    #[test]
    fn a_designed_outline_draws_its_own_body_and_a_bad_one_draws_the_ball() {
        let shape = format!("{OUTLINE}{CAT}");
        assert!(drawable(&shape));
        let body = body_of(&shape);
        assert_ne!(body, "sphere");
        assert_eq!(body_of(&shape), body);
        assert!(!drawable("outline:M0 0Z"));
        assert_eq!(body_of("outline:M0 0Z"), "sphere");
        assert_eq!(body_of("heart"), "heartChubby");
        assert!(drawable("heart") && !drawable("nonsense"));
    }
}
