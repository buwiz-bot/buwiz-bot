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
//! engine + QuickJS under [`widget_bundle`]'s deny-all policy inside the
//! Rust tree — the pure-Rust root is untouched. The `source` prop feeds the
//! compiled JS; a changed source reloads the app.

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

/// What a generated widget may reach: nothing beyond rendering. Widgets are
/// model output (possibly prompt-injected), so no network (not even the
/// local app server), web views, clipboard, picker, status bar, open_url,
/// host functions, files or processes; the sandbox stays hardened.
pub(crate) fn widget_policy() -> nsh_engine::Policy {
    nsh_engine::Policy {
        harden: true,
        declared: Some(nsh_core::access::Capabilities::default()),
        granted: Some(nsh_core::access::Capabilities::default()),
    }
}

/// A compiled widget as a bundle under [`widget_policy`].
pub(crate) fn widget_bundle(js: String) -> nsh_engine::Bundle {
    nsh_engine::Bundle::source(js).with_policy(widget_policy())
}

// ---------------------------------------------------------------------------
// The macOS component: an AppView per element, loaded from the `source` prop.
// ---------------------------------------------------------------------------

#[cfg(target_os = "macos")]
mod imp {
    use std::rc::Rc;

    use nsh_core::JsonValue;
    use nsh_platform_macos::{
        AppOptions, AppView, ComponentContext, NativeComponent,
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
            self.view
                .load(nsh_platform_macos::App::Js(super::widget_bundle(src)));
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
    use std::{
        sync::{Arc, Mutex, mpsc},
        time::{Duration, Instant},
    };

    use nsh_core::{
        ApproxTextEngine, Command, EventKind, EventPayload, MountBatch, MountItem, PayloadValue,
        ROOT_ID,
    };
    use nsh_engine::{
        App, Bundle, Engine, EngineConfig, FetchEvent, FetchTask, HttpHead, HttpRequest, MountSink,
        Network,
    };

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

    const WIDGET_EXFIL: &str = r#"
import { signal, Text } from "nsh";
import { Clipboard } from "nsh/clipboard";

export default function R() {
  const result = signal("waiting");
  Promise.all([
    fetch("http://127.0.0.1:8787/threads").then(() => "fetched", () => "refused"),
    Clipboard.hasText(),
    Clipboard.readText(),
  ]).then(([fetchOutcome, has, text]) => {
    result.set(`${fetchOutcome} ${has} [${text}]`);
  });
  return <Text>{result()}</Text>;
}
"#;

    struct Sink(mpsc::Sender<MountBatch>);

    impl MountSink for Sink {
        fn submit(&self, batch: MountBatch) {
            let _ = self.0.send(batch);
        }
    }

    struct CompletedTask;

    impl FetchTask for CompletedTask {
        fn send(&mut self, _: Vec<u8>) {}

        fn finish(&mut self) {}

        fn pause(&mut self, _: bool) {}

        fn cancel(self: Box<Self>) {}
    }

    struct RecordingNetwork {
        seen: Arc<Mutex<Vec<String>>>,
    }

    impl Network for RecordingNetwork {
        fn fetch(
            &self,
            request: HttpRequest,
            mut events: nsh_engine::FetchEvents,
        ) -> Box<dyn FetchTask> {
            let url = request.url;
            self.seen.lock().unwrap().push(url.clone());
            std::thread::spawn(move || {
                events(FetchEvent::Head(HttpHead {
                    status: 200,
                    url,
                    ..HttpHead::default()
                }));
                events(FetchEvent::End);
            });
            Box::new(CompletedTask)
        }
    }

    fn start_widget(
        bundle: Bundle,
    ) -> (Engine, mpsc::Receiver<MountBatch>, Arc<Mutex<Vec<String>>>) {
        let seen = Arc::new(Mutex::new(Vec::new()));
        let mut config = EngineConfig::new(App::Js(bundle));
        config.network = Some(Arc::new(RecordingNetwork { seen: seen.clone() }));
        let (tx, rx) = mpsc::channel();
        let engine = Engine::start(config, || ApproxTextEngine, Sink(tx)).expect("engine starts");
        (engine, rx, seen)
    }

    fn run_until_text(
        engine: &Engine,
        rx: &mpsc::Receiver<MountBatch>,
        wanted: &str,
    ) -> Vec<Command> {
        let deadline = Instant::now() + Duration::from_secs(5);
        let control = engine.control();
        let mut commands = Vec::new();
        loop {
            let remaining = deadline.saturating_duration_since(Instant::now());
            let batch = rx.recv_timeout(remaining).expect("widget result");
            let mut shown = false;
            for item in &batch.items {
                match item {
                    MountItem::Text { text, .. } if text.text == wanted => shown = true,
                    MountItem::Command { command, .. }
                        if matches!(
                            command,
                            Command::ReadClipboard { .. } | Command::SetClipboard(_)
                        ) =>
                    {
                        if let Command::ReadClipboard { id, .. } = command {
                            control.dispatch_event(
                                ROOT_ID,
                                EventKind::Clipboard,
                                EventPayload::Map(vec![
                                    ("id", PayloadValue::Number(*id as f64)),
                                    ("has", PayloadValue::Bool(false)),
                                ]),
                            );
                        }
                        commands.push(command.clone());
                    }
                    _ => {}
                }
            }
            if shown {
                return commands;
            }
        }
    }

    #[test]
    fn compiles_a_widget() {
        let js = super::compile_widget("R999", WIDGET).expect("compile");
        assert!(js.contains("mount(R)"), "wrapper mounted:\n{js}");
        assert!(js.contains("export default"), "default export kept:\n{js}");
        assert!(js.contains("from \"nsh\""), "nsh imports kept:\n{js}");
    }

    #[test]
    fn widget_policy_grants_nothing() {
        let access = super::widget_bundle(String::new()).policy.access();
        assert!(!access.allows_http("GET", "http://127.0.0.1:8787/threads"));
        assert!(!access.allows_http("POST", "https://attacker.example/"));
        assert!(!access.allows_media("https://attacker.example/x.png"));
        assert!(!access.allows_websocket("ws://127.0.0.1:8787/"));
        assert!(!access.allows_webviews());
        assert!(!access.allows_clipboard());
        assert!(!access.allows_picker());
        assert!(!access.allows_status_bar());
        assert!(!access.allows_open_url("https://attacker.example/"));
        assert!(!access.allows_open_url("someapp://run"));
        assert!(!access.allows_host("anything"));
        assert!(!access.allows_process("sh"));
        assert!(!access.allows_socket("127.0.0.1", 8787));
        assert!(super::widget_policy().harden);
    }

    #[test]
    fn a_widget_cannot_fetch_the_app_server_or_read_the_clipboard() {
        let js = super::compile_widget("R998", WIDGET_EXFIL).expect("compile widget");

        let (engine, rx, seen) = start_widget(super::widget_bundle(js.clone()));
        let commands = run_until_text(&engine, &rx, "refused false []");
        assert!(
            seen.lock().unwrap().is_empty(),
            "network requests: {seen:?}"
        );
        assert!(
            commands.is_empty(),
            "clipboard commands reached the platform: {commands:?}"
        );
        drop(engine);

        let (engine, rx, seen) = start_widget(Bundle::source(js));
        let commands = run_until_text(&engine, &rx, "fetched false []");
        assert_eq!(*seen.lock().unwrap(), ["http://127.0.0.1:8787/threads"]);
        assert!(
            commands
                .iter()
                .any(|command| matches!(command, Command::ReadClipboard { .. })),
            "default policy did not send a clipboard read command: {commands:?}"
        );
    }
}
