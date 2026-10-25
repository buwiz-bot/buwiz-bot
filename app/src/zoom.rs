//! Grok Bot's page zoom (⌘= / ⌘- / ⌘0): the whole window scales —
//! sidebar width, avatars, icons, the composer's text — not just the
//! messages. As a browser zooms, the app is laid out at the window's size
//! divided by the zoom and drawn that much larger: the window's content
//! view keeps its frame while its bounds shrink, so AppKit scales drawing
//! and hit-testing alike, and nsh's layers draw at the scale they show
//! (`backing_scale_of`), so text stays sharp.
//!
//! `rox/zoom` is a 0×0 native element at the app's root that carries the
//! level (`store().ui_zoom`); its view finds the window, applies it, and
//! applies it again whenever the content view's frame changes (a window
//! resize keeps the bounds' scale, but not the root's frame inside).

use nsh_core::JsonValue;
use nsh_core::ui::prelude::*;

use crate::live::store;

/// The element that applies `store().ui_zoom` to its window. 0×0: nsh
/// hands the mouse to the hosted view its own picking lands on, so one
/// covering the window took every click.
pub(crate) fn zoom_host() -> impl IntoElement {
    native("rox/zoom")
        .prop("zoom", move || JsonValue::from(store().ui_zoom.get() as f64))
        .absolute()
        .top(px(0.0))
        .left(px(0.0))
        .size(px(0.0))
}

#[cfg(target_os = "macos")]
mod imp {
    use std::cell::Cell;

    use nsh_core::JsonValue;
    use nsh_platform_macos::{
        ComponentContext, NativeComponent,
        objc2::{
            DefinedClass, MainThreadMarker, MainThreadOnly, define_class, msg_send, rc::Retained,
            runtime::{AnyObject, NSObjectProtocol},
            sel,
        },
        objc2_app_kit::{NSAutoresizingMaskOptions, NSView, NSViewFrameDidChangeNotification},
        objc2_foundation::{NSNotificationCenter, NSPoint, NSRect, NSSize},
    };

    pub struct ZoomIvars {
        zoom: Cell<f64>,
        /// The zoom the layers were last redrawn at.
        drawn: Cell<f64>,
        /// Whether it observes its window's content view's frame.
        observing: Cell<bool>,
    }

    define_class!(
        /// Invisible and never hit: it only watches its window.
        #[unsafe(super(NSView))]
        #[thread_kind = MainThreadOnly]
        #[ivars = ZoomIvars]
        #[name = "RoxZoomHost"]
        struct ZoomHost;

        unsafe impl NSObjectProtocol for ZoomHost {}

        impl ZoomHost {
            #[unsafe(method_id(hitTest:))]
            fn hit_test(&self, _point: NSPoint) -> Option<Retained<NSView>> {
                None
            }

            #[unsafe(method(viewDidMoveToWindow))]
            fn did_move_to_window(&self) {
                // SAFETY: NSView's own `viewDidMoveToWindow`.
                let () = unsafe { msg_send![super(self), viewDidMoveToWindow] };
                self.observe();
                self.apply();
            }

            #[unsafe(method(roxContentResized:))]
            fn content_resized(&self, _note: Option<&AnyObject>) {
                self.apply();
            }
        }
    );

    impl ZoomHost {
        fn new(mtm: MainThreadMarker) -> Retained<Self> {
            let this = Self::alloc(mtm).set_ivars(ZoomIvars {
                zoom: Cell::new(1.0),
                drawn: Cell::new(1.0),
                observing: Cell::new(false),
            });
            // SAFETY: NSView's designated `init` on a fresh allocation.
            unsafe { msg_send![super(this), init] }
        }

        /// Watches the window's content view's frame (once).
        fn observe(&self) {
            if self.ivars().observing.get() {
                return;
            }
            let Some(content) = self.window().and_then(|w| w.contentView()) else {
                return;
            };
            content.setPostsFrameChangedNotifications(true);
            // SAFETY: a selector-based observer; `roxContentResized:` takes
            // the notification. The center holds it weakly (macOS 10.11+).
            unsafe {
                NSNotificationCenter::defaultCenter().addObserver_selector_name_object(
                    self,
                    sel!(roxContentResized:),
                    Some(NSViewFrameDidChangeNotification),
                    Some(&content),
                );
            }
            self.ivars().observing.set(true);
        }

        /// Scales the window's content view by the zoom: bounds = frame /
        /// zoom, and the views that fill it (the surface's root) take the
        /// bounds' size, so the engine lays out at the smaller size.
        fn apply(&self) {
            let Some(content) = self.window().and_then(|w| w.contentView()) else {
                return;
            };
            let zoom = self.ivars().zoom.get().clamp(0.5, 3.0);
            let frame = content.frame().size;
            let want = NSSize::new(frame.width / zoom, frame.height / zoom);
            let near = |a: NSSize| (a.width - want.width).abs() < 0.5 && (a.height - want.height).abs() < 0.5;
            if !near(content.bounds().size) {
                content.setBoundsOrigin(NSPoint::new(0.0, 0.0));
                content.setBoundsSize(want);
            }
            let fills =
                NSAutoresizingMaskOptions::ViewWidthSizable | NSAutoresizingMaskOptions::ViewHeightSizable;
            for sub in content.subviews().iter() {
                if sub.autoresizingMask().contains(fills) && !near(sub.frame().size) {
                    sub.setFrame(NSRect::new(NSPoint::new(0.0, 0.0), want));
                }
            }
            // Every layer draws again at the scale it now shows.
            if (self.ivars().drawn.get() - zoom).abs() > 1e-3 {
                self.ivars().drawn.set(zoom);
                rescale(&content);
                content.setNeedsDisplay(true);
            }
        }
    }

    fn rescale(view: &NSView) {
        // SAFETY: NSView's `viewDidChangeBackingProperties`, no arguments;
        // nsh's views redraw their content at `backing_scale_of` there.
        let () = unsafe { msg_send![view, viewDidChangeBackingProperties] };
        for sub in view.subviews().iter() {
            rescale(&sub);
        }
    }

    pub struct Zoom {
        host: Retained<ZoomHost>,
    }

    impl Zoom {
        pub fn new(_ctx: ComponentContext) -> Zoom {
            let mtm = MainThreadMarker::new().expect("a component is made on the main thread");
            Zoom { host: ZoomHost::new(mtm) }
        }
    }

    impl NativeComponent for Zoom {
        fn view(&self) -> Retained<NSView> {
            Retained::into_super(self.host.clone())
        }

        fn set_prop(&mut self, name: &str, value: Option<&JsonValue>) {
            if name != "zoom" {
                return;
            }
            let zoom = value.and_then(|v| v.as_f64()).filter(|z| *z > 0.0).unwrap_or(1.0);
            self.host.ivars().zoom.set(zoom);
            self.host.apply();
        }
    }
}

/// Registers `rox/zoom` (call before `run_app`, as `register_mini_app`).
#[cfg(target_os = "macos")]
pub fn register_zoom(components: &nsh_platform_macos::Components) {
    components.register(
        "rox/zoom",
        std::rc::Rc::new(|ctx| {
            Box::new(imp::Zoom::new(ctx)) as Box<dyn nsh_platform_macos::NativeComponent>
        }),
    );
}
