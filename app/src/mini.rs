//! The mini-app host: Grok Shell's generated widgets, compiled on demand
//! and run as real apps inside the chat.
//!
//! A widget is a `generated/R<n>.tsx` module (`export default function
//! R()`); `GET /module?name=` returns its source. [`compile_widget`]
//! wraps it with `mount(R)` (the same wrapper `pipeline.mjs`'s chunk
//! emits) and hands it to `nsh_bundler::compile_module` — TypeScript
//! stripped, JSX compiled, `nsh:*` imports left for the runtime.
//!
//! The element `native("rox/mini-app")` mounts a registered native
//! component whose NSView *is* an `AppView`: each mini app gets its own
//! engine + QuickJS (`AppSource::Js(Bundle::source(..))`) inside the
//! Rust tree — the pure-Rust root is untouched. The `source` prop feeds
//! the compiled JS; a changed source reloads the app.

use nsh_core::JsonValue;
use nsh_core::ui::prelude::*;

use crate::live::store;

/// A turn card's live widget: `app` is the module name (`R<n>`); `h` the
/// card body's height, or `<= 0` to fill its parent. The `source` prop
/// updates as the store compiles. `design` is the size the app lays out
/// at (its declared `w×h`): the element's box shows it scaled uniformly
/// to fit, as the TSX card scales an oversize app (shell.tsx:911).
/// `on_natural` hears the app's natural content height as it changes.
pub(crate) fn mini_app(
    app: &str,
    h: f32,
    design: Option<std::rc::Rc<dyn Fn() -> (f32, f32)>>,
    on_natural: Option<std::rc::Rc<dyn Fn(f32)>>,
) -> impl IntoElement {
    let name = app.to_owned();
    let name2 = app.to_owned();
    native("rox/mini-app")
        .prop("design", move || match &design {
            Some(d) => {
                let (w, h) = d();
                serde_json::json!([w, h])
            }
            None => JsonValue::Null,
        })
        .on_native_event(move |e: &NativeEvent| {
            if &*e.name == "natural" {
                if let (Some(f), Some(h)) = (&on_natural, e.data.as_f64()) {
                    f(h as f32);
                }
            }
        })
        .prop(
            "source",
            move || JsonValue::String(store().mini_source(&name).get()),
        )
        .prop(
            "app",
            move || JsonValue::String(store().mini_source_title(&name2).get()),
        )
        .prop("dev_type", || JsonValue::String(store().mini_dev_type.get()))
        .w_full()
        .when(h > 0.0, |v| v.h(px(h)))
        .when(h <= 0.0, |v| v.flex_1())
}

/// TSX → the module the runtime runs: the source plus the wrapper
/// `pipeline.mjs` writes into every emitted chunk.
pub(crate) fn compile_widget(name: &str, source: &str) -> Result<String, String> {
    let wrapped = format!("{source}\nimport {{ mount }} from \"nsh\";\nmount(R);\n");
    let sets: Vec<nsh_core::ui::description::SetDescription> = vec![
        nsh_base::registry().into(),
        crate::registry().into(),
    ];
    // a server's app name (`mcp:<server>/<app>|<props>`) is no file name
    let file: String = name
        .chars()
        .take_while(|&c| c != '|')
        .map(|c| if c.is_ascii_alphanumeric() || c == '-' { c } else { '_' })
        .collect();
    nsh_bundler::compile_module(&format!("{file}.tsx"), &wrapped, &sets)
}

/// The mini app an MCP server ships, full screen over the window
/// (`store.mini_full`): a title bar and the app filling the rest. A server
/// opens it from a tool result (`rox/open`) or a `rox://app/…` link.
pub(crate) fn full_layer() -> impl IntoElement {
    use crate::theme::tc;
    dynamic(|| {
        let st = store();
        let Some((key, title, _)) = st.mini_full.get() else {
            return view().into_element();
        };
        let k2 = key.clone();
        let k3 = key.clone();
        view()
            .absolute()
            .size_full()
            .bg(tc("#161616"))
            .child(
                view()
                    .flex_row()
                    .items_center()
                    .gap_2()
                    .h(px(52.0))
                    .pl(px(84.0))
                    .pr(px(12.0))
                    .border_b(px(0.5))
                    .border_b_color(tc("#f5f5f51a"))
                    .child(
                        view()
                            .pointer_events(PointerEvents::None)
                            .absolute()
                            .left(px(84.0))
                            .right(px(84.0))
                            .top(px(0.0))
                            .bottom(px(0.0))
                            .items_center()
                            .justify_center()
                            .child(
                                text(move || {
                                    let t = store().mini_source_title(&k2).get();
                                    if t.is_empty() { title.clone() } else { t }
                                })
                                .number_of_lines(1)
                                .text_size(px(14.0))
                                .font_weight(FontWeight::MEDIUM)
                                .text_color(tc("#f5f5f5")),
                            ),
                    )
                    .child(view().flex_1())
                    .child(
                        view()
                            .label("Close")
                            .size(px(32.0))
                            .rounded(px(16.0))
                            .items_center()
                            .justify_center()
                            .bg(tc("#f5f5f50f"))
                            .child(text("✕").text_size(px(13.0)).text_color(tc("#f5f5f5a8")))
                            .on_press(|| store().mini_full.set(None)),
                    ),
            )
            .child(
                view()
                    .flex_1()
                    .child(when(
                        move || !store().mini_err(&k3).get().is_empty(),
                        {
                            let k = key.clone();
                            move || {
                                let k = k.clone();
                                view()
                                    .flex_1()
                                    .items_center()
                                    .justify_center()
                                    .child(
                                        text(move || format!("Couldn't open this app: {}", store().mini_err(&k).get()))
                                            .text_size(px(13.0))
                                            .text_color(tc("#f5f5f5a8")),
                                    )
                                    .into_element()
                            }
                        },
                    ))
                    .child(mini_app(&key, 0.0, None, None)),
            )
            .into_element()
    })
}

// ---------------------------------------------------------------------------
// The macOS component: an AppView per element, loaded from the `source` prop.
// ---------------------------------------------------------------------------

#[cfg(target_os = "macos")]
mod imp {
    use std::cell::{Cell, RefCell};
    use std::rc::Rc;

    use nsh_core::JsonValue;
    use nsh_platform_macos::{
        AppOptions, AppView, Bundle, ComponentContext, NativeComponent,
        objc2::{
            DefinedClass, MainThreadMarker, MainThreadOnly, define_class, msg_send,
            rc::Retained,
            runtime::{AnyObject, NSObjectProtocol},
            sel,
        },
        objc2_app_kit::{NSAppearanceCustomization, NSAutoresizingMaskOptions, NSView},
        objc2_foundation::{NSPoint, NSRect, NSSize},
    };

    /// How often a live mini app's natural height is checked (frame reads
    /// only; an event goes out only when it changed).
    const MEASURE_EVERY: f64 = 0.4;

    pub struct FitIvars {
        /// Consecutive ticks spent collapsed (see `collapsed`).
        collapsed_ticks: Cell<u32>,
        design: Cell<(f64, f64)>,
        natural: Cell<f64>,
        alive: Cell<bool>,
        ctx: RefCell<Option<ComponentContext>>,
    }

    define_class!(
        /// The element's view: holds the mini app at its design size and
        /// keeps its bounds at that size whatever frame layout gives it,
        /// so AppKit scales the app — drawing and hit-testing alike.
        #[unsafe(super(NSView))]
        #[thread_kind = MainThreadOnly]
        #[ivars = FitIvars]
        #[name = "RoxMiniFitView"]
        struct FitView;

        unsafe impl NSObjectProtocol for FitView {}

        impl FitView {
            #[unsafe(method(isFlipped))]
            fn is_flipped(&self) -> bool {
                true
            }

            #[unsafe(method(setFrameSize:))]
            fn set_frame_size(&self, size: NSSize) {
                // SAFETY: NSView's own `setFrameSize:` with its argument.
                let () = unsafe { msg_send![super(self), setFrameSize: size] };
                self.fit();
            }

            #[unsafe(method(viewDidMoveToWindow))]
            fn did_move_to_window(&self) {
                // SAFETY: NSView's own `viewDidMoveToWindow`.
                let () = unsafe { msg_send![super(self), viewDidMoveToWindow] };
                self.sync_appearance();
            }

            #[unsafe(method(roxMeasure:))]
            fn measure(&self, _arg: Option<&AnyObject>) {
                if !self.ivars().alive.get() {
                    return;
                }
                self.sync_appearance();
                self.fit();
                if collapsed(self) {
                    self.ivars().collapsed_ticks.set(self.ivars().collapsed_ticks.get() + 1);
                    self.schedule();
                    return;
                }
                self.ivars().collapsed_ticks.set(0);
                if let Some(h) = self.subviews().firstObject().and_then(|app| natural(&app)) {
                    if (h - self.ivars().natural.get()).abs() >= 1.0 {
                        self.ivars().natural.set(h);
                        if let Some(ctx) = self.ivars().ctx.borrow().as_ref() {
                            ctx.emit("natural", serde_json::json!(h.ceil()));
                        }
                    }
                }
                self.schedule();
            }
        }
    );

    /// The app's natural content height — what Grok gets by laying the
    /// app out with unbounded height. The app's root element(s) are the
    /// surface root's children; a root sized to its content measures as
    /// itself. A root that fills its frame (a fixed height) can have its
    /// content spill out of it: then the content's own bottom edge counts,
    /// plus the root's top inset as the matching bottom padding. Once the
    /// card grows to it, the root fills exactly that — a fixed point, so
    /// the card never steps up or down by itself.
    fn natural(container: &NSView) -> Option<f64> {
        let root = container.subviews().firstObject()?;
        let apps = root.subviews();
        if apps.count() == 0 {
            return None;
        }
        let mut bottom = 0.0f64;
        let mut budget = 4000usize;
        for app in apps.iter() {
            let f = app.frame();
            let edge = f.origin.y + f.size.height;
            bottom = bottom.max(edge);
            let (mut content, mut top) = (0.0f64, f64::MAX);
            extent(&app, f.origin.y, &mut content, &mut top, &mut budget);
            if content > edge + 0.5 {
                let pad = (top - f.origin.y).clamp(0.0, 48.0);
                bottom = bottom.max(content + pad);
            }
        }
        Some(bottom)
    }

    /// The vertical extent of `view`'s visible descendants, in its
    /// parent's coordinates (`y0` is the view's top there; nsh's views are
    /// flipped). A view that clips (a scroll view, `overflow: hidden`)
    /// counts as its own box: a long scrolling list must not grow the card.
    fn extent(view: &NSView, y0: f64, bottom: &mut f64, top: &mut f64, budget: &mut usize) {
        for sub in view.subviews().iter() {
            if *budget == 0 {
                return;
            }
            *budget -= 1;
            let f = sub.frame();
            if sub.isHidden() || f.size.height <= 0.0 || f.size.width <= 0.0 {
                continue;
            }
            let y = y0 + f.origin.y;
            *bottom = bottom.max(y + f.size.height);
            *top = top.min(y);
            if !clips(&sub) {
                extent(&sub, y, bottom, top, budget);
            }
        }
    }

    fn clips(view: &NSView) -> bool {
        // SAFETY: NSView's `clipsToBounds` (macOS 14+) and CALayer's
        // `masksToBounds`, both BOOL getters.
        unsafe {
            let own: bool = msg_send![view, clipsToBounds];
            if own {
                return true;
            }
            let layer: Option<Retained<AnyObject>> = msg_send![view, layer];
            layer.is_some_and(|l| {
                let masks: bool = msg_send![&*l, masksToBounds];
                masks
            })
        }
    }

    /// Whether a view above `view` has an empty frame: a branch nsh has
    /// collapsed out of layout. Nothing clips there, so the app would draw
    /// at that branch's origin — over whatever the cell shows now (a
    /// reused list cell keeps its old card's views at 0×0) — and a cell
    /// not laid out yet is collapsed for a moment too.
    fn collapsed(view: &NSView) -> bool {
        // SAFETY: reading the superview chain on the main thread.
        let mut cur = unsafe { view.superview() };
        while let Some(p) = cur {
            if p.window().is_some_and(|w| w.contentView().as_deref() == Some(&*p)) {
                return false;
            }
            let f = p.frame().size;
            if f.width < 1.0 || f.height < 1.0 {
                return true;
            }
            cur = unsafe { p.superview() };
        }
        false
    }

    impl FitView {
        fn new(mtm: MainThreadMarker, ctx: ComponentContext) -> Retained<Self> {
            let this = Self::alloc(mtm).set_ivars(FitIvars {
                collapsed_ticks: Cell::new(0),
                design: Cell::new((0.0, 0.0)),
                natural: Cell::new(0.0),
                alive: Cell::new(true),
                ctx: RefCell::new(Some(ctx)),
            });
            // SAFETY: NSView's designated `init` on a fresh allocation.
            let this: Retained<Self> = unsafe { msg_send![super(this), init] };
            this.schedule();
            this
        }

        /// Generated widgets keep the system's appearance whatever the
        /// app's theme toggle says — Grok themes its own chrome through
        /// `tc()` and never recolors the widgets it hosts. The theme sits on
        /// the surface root; the window still carries the system's.
        fn sync_appearance(&self) {
            let Some(window) = self.window() else {
                return;
            };
            let system = window.effectiveAppearance();
            let current = self.appearance().map(|a| a.name());
            if current.as_deref() != Some(&*system.name()) {
                self.setAppearance(Some(&system));
            }
        }

        fn schedule(&self) {
            // A collapsed card checks again soon at first (a new cell is
            // laid out within a few frames), then at the usual pace (a
            // leftover from a reused cell stays collapsed).
            let n = self.ivars().collapsed_ticks.get();
            let delay = if n > 0 && n < 40 { 0.05 } else { MEASURE_EVERY };
            // SAFETY: `roxMeasure:` is this class's method taking one
            // object argument (nil here).
            let () = unsafe {
                msg_send![
                    self,
                    performSelector: sel!(roxMeasure:),
                    withObject: None::<&AnyObject>,
                    afterDelay: delay
                ]
            };
        }

        /// Design size, or the frame's own size (no scaling) without one.
        fn fit(&self) {
            let frame = self.frame().size;
            // An empty frame (no scale can be set on it) or a collapsed
            // branch above: the app would draw where it should not — over
            // the transcript's top. Hide it.
            let empty = frame.width < 1.0 || frame.height < 1.0 || collapsed(self);
            if let Some(app) = self.subviews().firstObject() {
                app.setHidden(empty);
            }
            if empty {
                return;
            }
            let (w, h) = self.ivars().design.get();
            let size = if w > 0.0 && h > 0.0 { NSSize::new(w, h) } else { frame };
            self.setBoundsSize(size);
        }
    }

    /// One mini app: its own AppView (engine + QuickJS) hosted in the
    /// element's fit view.
    pub struct MiniApp {
        fit: Retained<FitView>,
        view: Rc<AppView>,
        last: String,
    }

    impl MiniApp {
        pub fn new(ctx: ComponentContext) -> MiniApp {
            let mtm = MainThreadMarker::new().expect("a component is made on the main thread");
            let view = AppView::new(
                mtm,
                AppOptions {
                    title: "Rox mini app".to_owned(),
                    app_id: "dev.goldcoders.rox.mini".to_owned(),
                    size: (340, 200),
                    // `asset:/generated/assets/…` (generated images) are the
                    // server's files, served under /asset/.
                    asset_base: Some(nsh_platform_macos::AssetBase::Http(format!(
                        "{}/asset/",
                        crate::client::GEN
                    ))),
                    ..Default::default()
                },
            );
            // The size the layout takes when the style leaves one open.
            ctx.set_size(340.0, 200.0);
            let fit = FitView::new(mtm, ctx);
            // Nothing of the app draws outside the card's box.
            // SAFETY: NSView's `clipsToBounds` (macOS 14+), a BOOL setter.
            let () = unsafe { msg_send![&*fit, setClipsToBounds: true] };
            let app = view.view();
            app.setFrame(NSRect::new(NSPoint::new(0.0, 0.0), NSSize::new(340.0, 200.0)));
            app.setAutoresizingMask(
                NSAutoresizingMaskOptions::ViewWidthSizable
                    | NSAutoresizingMaskOptions::ViewHeightSizable,
            );
            fit.addSubview(&app);
            MiniApp { fit, view, last: String::new() }
        }

        /// Lays the app out at `w×h` and scales it into the frame; `None`
        /// fills the frame unscaled.
        fn set_design(&self, design: Option<(f64, f64)>) {
            let app = self.view.view();
            match design {
                Some((w, h)) if w > 0.0 && h > 0.0 => {
                    app.setAutoresizingMask(NSAutoresizingMaskOptions::ViewNotSizable);
                    app.setFrame(NSRect::new(NSPoint::new(0.0, 0.0), NSSize::new(w, h)));
                    self.fit.ivars().design.set((w, h));
                }
                _ => {
                    self.fit.ivars().design.set((0.0, 0.0));
                    app.setFrame(NSRect::new(NSPoint::new(0.0, 0.0), self.fit.frame().size));
                    app.setAutoresizingMask(
                        NSAutoresizingMaskOptions::ViewWidthSizable
                            | NSAutoresizingMaskOptions::ViewHeightSizable,
                    );
                }
            }
            self.fit.fit();
        }
    }

    impl Drop for MiniApp {
        fn drop(&mut self) {
            // Ends the measuring loop; the pending perform releases the view.
            self.fit.ivars().alive.set(false);
            self.fit.ivars().ctx.borrow_mut().take();
        }
    }

    /// The text fields under `view`, in view order.
    fn text_fields(view: &NSView, out: &mut Vec<Retained<NSView>>) {
        for sub in view.subviews().iter() {
            if sub.class().name().to_str().is_ok_and(|n| n.contains("TextField")) {
                out.push(sub.clone());
            }
            text_fields(&sub, out);
        }
    }

    /// Focuses text field `n` under `root` and inserts `text` into it.
    fn dev_type(root: &NSView, n: usize, text: &str) {
        let mut fields = Vec::new();
        text_fields(root, &mut fields);
        let (Some(field), Some(window)) = (fields.get(n), root.window()) else {
            log::warn!("dev type: no text field {n} ({} found)", fields.len());
            return;
        };
        window.makeFirstResponder(Some(field));
        let Some(editor) = window.firstResponder() else { return };
        let s = nsh_platform_macos::objc2_foundation::NSString::from_str(text);
        // SAFETY: the first responder of a focused field is its field editor (an NSTextView).
        let () = unsafe { msg_send![&*editor, insertText: &*s] };
        log::info!("dev type: {} fields, typed {text:?} into {n}", fields.len());
    }

    impl NativeComponent for MiniApp {
        fn view(&self) -> Retained<NSView> {
            Retained::into_super(self.fit.clone())
        }

        fn set_prop(&mut self, name: &str, value: Option<&JsonValue>) {
            if name == "design" {
                let size = value.and_then(|v| v.as_array()).and_then(|a| {
                    Some((a.first()?.as_f64()?, a.get(1)?.as_f64()?))
                });
                self.set_design(size);
                return;
            }
            if name == "dev_type" {
                // dev hook (`rox-agent-menu`: `type <n> <text>`): the app's
                // n-th text field takes the focus and `text` is inserted the
                // way typing ends up (`insertText:` on the field editor)
                if let Some((n, text)) = value
                    .and_then(|v| v.as_str())
                    .and_then(|v| v.split_once('|'))
                    .and_then(|(_, rest)| rest.split_once(' '))
                {
                    dev_type(&self.fit, n.parse().unwrap_or(0), text);
                }
                return;
            }
            if name != "source" {
                return;
            }
            let src = value.and_then(|v| v.as_str()).unwrap_or_default().to_owned();
            if src.is_empty() || src == self.last {
                return;
            }
            self.last = src.clone();
            self.view.load(nsh_platform_macos::App::Js(Bundle::source(src)));
        }
    }
}

/// Registers `rox/mini-app` on the app's component registry (call before
/// `run_app` so the first cards resolve it).
#[cfg(target_os = "macos")]
pub fn register_mini_app(components: &nsh_platform_macos::Components) {
    components.register(
        "rox/mini-app",
        std::rc::Rc::new(|ctx| {
            Box::new(imp::MiniApp::new(ctx)) as Box<dyn nsh_platform_macos::NativeComponent>
        }),
    );
}

#[cfg(test)]
mod tests {
    /// A representative generated widget: imports the same nsh face the
    /// server prompts for, `export default function R()`.
    const WIDGET: &str = r#"
import { signal, Text, View } from "nsh";

export default function R() {
  const n = signal(0);
  return (
    <View style="p-4 bg-white rounded">
      <Text>{`count ${n()}`}</Text>
      <Text onPress={() => n(n() + 1)}>bump</Text>
    </View>
  );
}
"#;

    #[test]
    fn compiles_a_widget() {
        let js = super::compile_widget("R999", WIDGET).expect("compile");
        assert!(js.contains("mount(R)"), "wrapper mounted:\n{js}");
        assert!(js.contains("export default"), "default export kept:\n{js}");
        assert!(js.contains("from \"nsh\""), "nsh imports kept:\n{js}");
    }
}
