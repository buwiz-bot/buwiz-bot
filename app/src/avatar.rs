//! The agent avatar: a `canvas()` hosting the mascot (`crate::mascot`).
//! Its animator advances on the store's shared clock while anything moves;
//! a settled, resting avatar sleeps until its gaze, edit shade or a poke
//! wakes it. `BlobGaze`/`BlobPokes`/`BlobEdit` are the drivers views share
//! with an avatar.

use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::time::{Duration, Instant};

use nsh_core::prelude::*;

use crate::mascot::{self, Gesture, Look};
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
    target: Cell<Option<Look>>,
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
    fn change(&self, next: Option<Look>) {
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
    pub(crate) fn target(&self) -> Option<Look> {
        self.target.get()
    }
    pub(crate) fn ver(&self) -> u32 {
        self.ver.get()
    }
}

/// Queued gestures (and how many times to play each) the canvas drains on
/// its next paint.
pub(crate) struct BlobPokes {
    queue: RefCell<Vec<(Gesture, f64)>>,
    wake: Wake,
}

impl BlobPokes {
    pub(crate) fn new() -> Rc<Self> {
        Rc::new(Self {
            queue: RefCell::new(vec![]),
            wake: Wake::new(),
        })
    }
    pub(crate) fn fire(&self, gesture: Gesture, turns: Option<f64>) {
        self.queue.borrow_mut().push((gesture, turns.unwrap_or(1.0)));
        self.wake.ping();
    }
    fn drain(&self) -> Vec<(Gesture, f64)> {
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

/// `BotBlobProps` — `shape` is the picker name (`blob`, `cloud`, …),
/// `state` a `STATES` name, `paused` pins the resting pose.
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
    pub gaze_at: Option<Look>,
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

/// The per-canvas animator + prop mirrors.
struct Inner {
    animator: mascot::Animator,
    last_shape: String,
    last_state: &'static str,
    last_compact: bool,
    last_emphasis: bool,
    last_resting: bool,
    last_gaze_ver: u32,
    live_until: Instant,
    grace_until: Instant,
    edit_level: f64,
    prev_clock: f64,
    asleep: bool,
}

/// The state a props' source (or fixed name) shows now.
fn state_of(props: &BotBlobProps) -> &'static mascot::StateSpec {
    match &props.state_src {
        Some(source) => mascot::state(&source()),
        None => mascot::state(props.state),
    }
}

/// Small avatars skip the outline wobble.
fn is_compact(size: f32) -> bool {
    size * 0.82 < 48.0
}

impl Inner {
    fn new(props: &BotBlobProps) -> Self {
        let seed = props.seed.as_deref().map(mascot::seed_from).unwrap_or_else(|| {
            (std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.subsec_nanos())
                .unwrap_or(1))
                | 1
        });
        let state = state_of(props);
        let resting = props.paused || state.name == "idle";
        let mut animator = mascot::Animator::new(
            mascot::Shape::from_name(&props.shape),
            state,
            seed,
            resting,
            is_compact(props.size),
        );
        animator.look_at(props.gaze.as_ref().and_then(|g| g.target()).or(props.gaze_at));
        animator.settle_gaze();
        let now = Instant::now();
        Self {
            animator,
            last_shape: props.shape.clone(),
            last_state: state.name,
            last_compact: is_compact(props.size),
            last_emphasis: false,
            last_resting: resting,
            last_gaze_ver: props.gaze.as_ref().map_or(0, |g| g.ver()),
            live_until: now,
            grace_until: now,
            edit_level: 0.0,
            prev_clock: 0.0,
            asleep: true,
        }
    }

    /// Mirrors prop changes (shape, state, size, emphasis, gaze, pokes,
    /// resting) into the animator.
    fn sync(&mut self, props: &BotBlobProps, reduced_motion: bool) {
        let now = Instant::now();
        if props.shape != self.last_shape {
            let morph = props.morph_shape && !reduced_motion;
            self.animator.set_shape(mascot::Shape::from_name(&props.shape), morph);
            if morph {
                self.grace_until = now + Duration::from_millis(1400);
            }
            self.live_until = now + Duration::from_millis(1200);
            self.last_shape = props.shape.clone();
        }
        let state = state_of(props);
        if state.name != self.last_state {
            if state.name == "idle" {
                self.grace_until = now + Duration::from_millis(1400);
            }
            self.animator.set_state(state);
            self.last_state = state.name;
        }
        let compact = is_compact(props.size);
        if compact != self.last_compact {
            self.animator.set_compact(compact);
            self.last_compact = compact;
        }
        let emphasized = props.emph_src.as_ref().map(|f| f()).unwrap_or(props.emphasized);
        if emphasized != self.last_emphasis {
            self.animator.set_emphasis(emphasized);
            self.live_until = now + Duration::from_millis(400);
            self.last_emphasis = emphasized;
        }
        if let Some(gaze) = &props.gaze
            && gaze.ver() != self.last_gaze_ver {
                self.animator.look_at(gaze.target().or(props.gaze_at));
                self.last_gaze_ver = gaze.ver();
        }
        if let Some(pokes) = &props.pokes {
            for (gesture, turns) in pokes.drain() {
                // reduced motion: a click doesn't throw the body around
                if !reduced_motion {
                    self.animator.play(gesture, turns);
                    self.live_until = now + Duration::from_millis(1500);
                }
            }
        }
        let resting = self.is_resting(props, now, reduced_motion);
        if resting != self.last_resting {
            self.animator.set_resting(resting);
            self.last_resting = resting;
        }
    }

    /// Paused, idle past the grace period, or reduced motion: the state's
    /// still frame (eyes still follow the pointer).
    fn is_resting(&self, props: &BotBlobProps, now: Instant, reduced_motion: bool) -> bool {
        props.paused || reduced_motion || (self.last_state == "idle" && now >= self.grace_until)
    }

    /// Advances the animator by the clock's step while live, and eases the
    /// edit shade toward its target.
    fn advance(&mut self, props: &BotBlobProps, clock: f64, live: bool) {
        if !live {
            self.asleep = true;
            return;
        }
        if self.asleep {
            self.prev_clock = clock;
            self.asleep = false;
            return;
        }
        let dt = (clock - self.prev_clock).clamp(0.0, 0.1);
        self.prev_clock = clock;
        if dt > 0.0 {
            self.animator.advance(dt);
            let edit_target = props.edit.as_ref().map_or(0.0, |e| e.target.get());
            self.edit_level += (edit_target - self.edit_level) * (1.0 - (-14.0 * dt).exp());
        }
    }

    /// Paints the current frame: the body on a `size*1.5` canvas, centered.
    fn paint(&self, p: Painter, props: &BotBlobProps) -> Painter {
        let size = props.size;
        let center = size * 0.75;
        let radius = size * 0.41;
        // Reading `dark()` inside the painter subscribes it to the theme,
        // so a theme flip repaints.
        let ink = mascot::ink_of(&props.color, !dark());
        let ink = if self.edit_level > 0.001 {
            mascot::darken(ink, 0.7 * self.edit_level)
        } else {
            ink
        };
        self.animator.figure().paint(p, center, center, radius, ink)
    }
}

/// `BotBlob` — a `size`-cell `View` with a `size*1.5` canvas overhanging it.
pub(crate) fn bot_blob(props: BotBlobProps) -> impl IntoElement {
    let size = props.size;
    let canvas_size = size * 1.5;
    let offset = -size * 0.25;
    let st = Rc::new(RefCell::new(Inner::new(&props)));
    let props = Rc::new(props);
    view()
        .size(px(size))
        .child(
            canvas(move || {
                let mut st = st.borrow_mut();
                let props = &*props;
                let reduced_motion = reduce_motion();
                st.sync(props, reduced_motion);
                let now = Instant::now();
                let live = !st.animator.is_settled()
                    || !st.is_resting(props, now, reduced_motion)
                    || (props.edit.as_ref().map_or(0.0, |e| e.target.get()) - st.edit_level).abs() > 0.002
                    || st.grace_until > now
                    || st.live_until > now;
                // Live: the shared clock repaints it next frame. Settled:
                // sleep until gaze, edit or a poke wakes it, so resting
                // avatars cost no frames.
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
                st.advance(props, t, live);
                st.paint(Painter::new(), props)
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
    pub gaze_at: Option<Look>,
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
