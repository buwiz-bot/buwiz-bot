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
/// updates as the store compiles.
pub(crate) fn mini_app(app: &str, h: f32) -> impl IntoElement {
    let name = app.to_owned();
    let name2 = app.to_owned();
    native("rox/mini-app")
        .prop(
            "source",
            move || JsonValue::String(store().mini_source(&name).get()),
        )
        .prop(
            "app",
            move || JsonValue::String(store().mini_source_title(&name2).get()),
        )
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
    nsh_bundler::compile_module(&format!("{name}.tsx"), &wrapped, &sets)
}

// ---------------------------------------------------------------------------
// The macOS component: an AppView per element, loaded from the `source` prop.
// ---------------------------------------------------------------------------

#[cfg(target_os = "macos")]
mod imp {
    use std::rc::Rc;

    use nsh_core::JsonValue;
    use nsh_platform_macos::{
        AppOptions, AppView, Bundle, ComponentContext, NativeComponent,
        objc2::{MainThreadMarker, rc::Retained},
        objc2_app_kit::NSView,
    };

    /// One mini app: its own AppView (engine + QuickJS) hosted in the
    /// element's container.
    pub struct MiniApp {
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
                    ..Default::default()
                },
            );
            // The size the layout takes when the style leaves one open.
            ctx.set_size(340.0, 200.0);
            MiniApp { view, last: String::new() }
        }
    }

    impl NativeComponent for MiniApp {
        fn view(&self) -> Retained<NSView> {
            self.view.view()
        }

        fn set_prop(&mut self, name: &str, value: Option<&JsonValue>) {
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
