use std::{cell::{Cell, RefCell}, rc::Rc};

use nsh_core::JsonValue;
use nsh_platform_windows::{
    App, AppOptions, AppView, Appearance, AssetBase, ComponentContext, NativeComponent,
    windows::{
        core::w,
        Win32::{
            Foundation::{HWND, RECT},
            Graphics::Direct2D::ID2D1DeviceContext,
            UI::WindowsAndMessaging::{
                CreateWindowExW, DestroyWindow, GetClientRect, GetDesktopWindow, SetWindowPos,
                ShowWindow, SW_HIDE, SW_SHOWNOACTIVATE, SWP_NOACTIVATE, SWP_NOZORDER,
                WINDOW_EX_STYLE, WS_CHILD, WS_CLIPCHILDREN, WS_CLIPSIBLINGS,
            },
        },
    },
};

use crate::mini_v1::{native::{self, V1Open}, settle};

pub struct MiniApp {
    ctx: ComponentContext,
    window: HWND,
    view: Option<Rc<AppView>>,
    last: Option<(String, String, Option<String>)>,
    v1: Option<V1Open>,
    viewport_sink: Option<Box<dyn Fn(JsonValue)>>,
    size: Cell<(f32, f32)>,
    placement: String,
    theme: String,
    paused: bool,
    settle: Rc<RefCell<settle::OpenSettle>>,
    timer: RefCell<Option<native::Timer>>,
    wait_ready: bool,
}

impl MiniApp {
    pub fn new(ctx: ComponentContext) -> Self {
        ctx.set_size(340.0, 200.0);
        // SAFETY: nsh reparents this hidden container to the element's parent HWND.
        let window = unsafe {
            CreateWindowExW(
                WINDOW_EX_STYLE::default(), w!("STATIC"), w!(""),
                WS_CHILD | WS_CLIPCHILDREN | WS_CLIPSIBLINGS,
                0, 0, 340, 200, Some(GetDesktopWindow()), None, None, None,
            )
        }.expect("mini app container");
        Self {
            ctx, window, view: None, last: None, v1: None, viewport_sink: None,
            size: Cell::new((340.0, 200.0)), placement: "full".into(),
            theme: String::new(), paused: false, settle: Rc::default(), timer: RefCell::new(None), wait_ready: false,
        }
    }

    fn appearance(&self) -> Appearance {
        match super::mini_appearance(&self.theme) {
            Some(true) => Appearance::Dark,
            Some(false) => Appearance::Light,
            None => Appearance::System,
        }
    }

    fn viewport(&self) -> JsonValue {
        let (w, h) = self.size.get();
        serde_json::json!({ "width": w.round(), "height": h.round(), "placement": self.placement })
    }

    fn clear(&mut self) {
        self.timer.get_mut().take();
        self.settle.borrow_mut().cancel();
        self.viewport_sink.take();
        self.v1.take();
        if let Some(view) = self.view.take() {
            view.stop();
            // SAFETY: this component owns the child, on its UI thread.
            let _ = unsafe { DestroyWindow(view.hwnd()) };
        }
    }

    fn finish(&self) {
        let Some(open) = self.settle.borrow().current() else { return };
        let done = self.settle.borrow_mut().settle(open);
        if done {
            self.timer.borrow_mut().take();
            self.ctx.emit("settled", JsonValue::Null);
        }
    }

    fn arm_timer(&self) {
        if self.placement != "inline" || self.view.is_none() || self.timer.borrow().is_some() {
            return;
        }
        let Some(open) = self.settle.borrow().current() else { return };
        let hold = Rc::downgrade(&self.settle);
        let ctx = self.ctx.clone();
        let cap = if self.wait_ready { settle::CAP_READY } else { settle::CAP };
        let timer = native::after(cap, move || {
            let Some(hold) = hold.upgrade() else { return };
            let done = hold.borrow_mut().settle(open);
            if done {
                ctx.emit("settled", JsonValue::Null);
            }
        });
        if timer.is_none() {
            self.finish();
        }
        *self.timer.borrow_mut() = timer;
    }

    fn load(&mut self) {
        let Some((module, js, call)) = self.last.clone() else { return };
        self.clear();
        self.settle.borrow_mut().start();
        self.wait_ready = false;
        let mut options = AppOptions {
            title: "Buwiz Bot mini app".into(),
            app_id: crate::identity::MINI_APP_ID.into(),
            size: (340, 200),
            appearance: self.appearance(),
            data_dir: crate::identity::nsh_data_dir_in(buwiz_protocol::env::var_os("BUWIZ_DATA_DIR").as_deref()),
            asset_base: Some(AssetBase::Http(format!("{}/asset/", crate::client::backend()))),
            ..Default::default()
        };
        let prepared = if crate::mini_v1::is_v1(&module) {
            let Some(prepared) = crate::mini_v1::prepared_js(&module, &js) else {
                let key = module;
                return crate::mini_v1::to_store(move |st| {
                    st.mini_err(&key).set("Could not open this app: it was not ready. Close it and open it again.".into())
                });
            };
            options.title = prepared.title.clone();
            options.app_id = crate::identity::MINI_V1_APP_ID.into();
            options.asset_base = None;
            options.limits = nsh_engine::Limits {
                memory_bytes: 256 << 20, stack_bytes: 4 << 20,
                load_budget: std::time::Duration::from_secs(10),
                call_budget: std::time::Duration::from_secs(2),
                ..Default::default()
            };
            options.storage = Some(crate::mini_v1::storage::engine_storage(
                &crate::mini_v1::plugins_dir(), &prepared.plugin.id, &prepared.key.app,
            ));
            self.wait_ready = crate::mini_v1::cards::wait_ready(
                &prepared.plugin.app(&prepared.key.app).cloned().unwrap_or_default(),
            );
            Some(prepared)
        } else {
            None
        };
        let view = match AppView::new(Some(self.window), options) {
            Ok(view) => view,
            Err(e) => {
                let key = module;
                let message = format!("Could not open this app: {e}");
                return crate::mini_v1::to_store(move |st| st.mini_err(&key).set(message));
            }
        };
        let channel = view.host();
        if let Some(prepared) = prepared {
            let native = prepared.native.is_some();
            let open = crate::mini_v1::native::start(prepared, &view, module, self.viewport());
            if native {
                self.viewport_sink = Some(Box::new(open.viewport_sink()));
            }
            self.v1 = Some(open);
        } else {
            view.load(App::Js(super::mini_bundle(&module, js, call.as_deref())));
        }
        if self.viewport_sink.is_none() {
            self.viewport_sink = Some(Box::new(move |v| { let _ = channel.send("buwiz.viewport", v); }));
        }
        self.view = Some(view);
        self.arm_timer();
        self.ctx.redraw();
    }
}

impl NativeComponent for MiniApp {
    fn window(&self) -> Option<HWND> {
        Some(self.window)
    }

    fn draw(&self, _dc: &ID2D1DeviceContext, size: (f32, f32)) {
        let Some(view) = &self.view else { return };
        let changed = self.size.replace(size) != size;
        let mut rect = RECT::default();
        // SAFETY: both windows belong to this component on the UI thread.
        unsafe {
            if GetClientRect(self.window, &mut rect).is_ok() {
                let _ = SetWindowPos(view.hwnd(), None, 0, 0, rect.right, rect.bottom, SWP_NOZORDER | SWP_NOACTIVATE);
            }
            let _ = ShowWindow(view.hwnd(), if self.paused { SW_HIDE } else { SW_SHOWNOACTIVATE });
        }
        if changed && let Some(send) = &self.viewport_sink {
            send(self.viewport());
        }
        if !self.paused && !self.wait_ready {
            self.finish();
        }
    }

    fn set_prop(&mut self, name: &str, value: Option<&JsonValue>) {
        match name {
            "theme" => {
                let theme = value.and_then(JsonValue::as_str).unwrap_or_default();
                if self.theme != theme {
                    self.theme = theme.into();
                    self.load();
                }
            }
            "paused" => {
                self.paused = value.and_then(JsonValue::as_bool).unwrap_or(false);
                self.ctx.redraw();
            }
            "placement" => {
                self.placement = if value.and_then(JsonValue::as_str) == Some("inline") { "inline" } else { "full" }.into();
                if self.placement != "inline" {
                    self.timer.borrow_mut().take();
                }
                self.arm_timer();
                if let Some(send) = &self.viewport_sink { send(self.viewport()); }
            }
            "design" => {
                let design = value.and_then(JsonValue::as_array)
                    .and_then(|a| Some((a.first()?.as_f64()? as f32, a.get(1)?.as_f64()? as f32)))
                    .filter(|(w, h)| w.is_finite() && h.is_finite() && *w > 0.0 && *h > 0.0);
                let (w, h) = design.unwrap_or((340.0, 200.0));
                self.ctx.set_size(w, h);
            }
            "ready" => {
                if value.and_then(JsonValue::as_u64).is_some_and(|n| self.v1.as_ref().is_some_and(|o| o.n() == n)) {
                    self.wait_ready = false;
                    self.finish();
                    self.ctx.redraw();
                }
            }
            "hold" => {
                if value.and_then(JsonValue::as_bool) == Some(true) {
                    self.arm_timer();
                    self.ctx.redraw();
                }
            }
            "source" => {
                let Some(source) = value.and_then(JsonValue::as_object) else { return };
                let module = source.get("module").and_then(JsonValue::as_str).unwrap_or_default();
                let js = source.get("js").and_then(JsonValue::as_str).unwrap_or_default();
                if js.is_empty() || self.last.as_ref().is_some_and(|(m, j, _)| m == module && j == js) {
                    return;
                }
                let call = source.get("call").and_then(JsonValue::as_str).map(str::to_owned);
                self.last = Some((module.into(), js.into(), call));
                self.load();
            }
            _ => {}
        }
    }
}

impl Drop for MiniApp {
    fn drop(&mut self) {
        self.clear();
        // SAFETY: nsh releases the component on the UI thread.
        let _ = unsafe { DestroyWindow(self.window) };
    }
}
