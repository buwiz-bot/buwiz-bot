//! Runs the app on the machine's own backend.
//!
//! macOS: `cargo run` opens the window; for live edits run it through the
//! daemon (`cargo ra-jit run`, then `cargo ra-jit build` after each edit —
//! `../dev.sh` watches and does that). iOS: this binary is the `.app`'s
//! executable; `../ios.sh` builds it for the simulator, packs the bundle
//! and launches it.

#[cfg(target_os = "macos")]
use nsh_platform_macos as platform;

/// The process logger: `env_logger` as before, plus every `nsh::jank`
/// line (a frame over budget, a slow engine wake-up, and what each did —
/// logged while the performance monitor runs, ⌘⇧F) appended to
/// `~/Library/Logs/Buwiz Bot/jank.log` (`ROX_JANK_LOG` overrides). The monitor's
/// Clear (⌘⇧K) logs `nsh_engine::perf::RESET_MARK`, which empties the file:
/// what is in it is what happened since the last clear.
#[cfg(target_os = "macos")]
mod jank_log {
    use std::io::Write;
    use std::sync::Mutex;

    const TARGET: &str = "nsh::jank";

    struct Tee {
        inner: env_logger::Logger,
        path: std::path::PathBuf,
        file: Mutex<Option<std::fs::File>>,
    }

    pub(crate) fn init() {
        let inner = env_logger::Builder::from_env(
            env_logger::Env::default().default_filter_or("error,nsh::jank=warn"),
        )
        .build();
        let path = std::env::var_os("ROX_JANK_LOG")
            .map(std::path::PathBuf::from)
            .unwrap_or_else(|| {
                let home = std::env::var_os("HOME").unwrap_or_default();
                rox::identity::logs_dir(std::path::Path::new(&home)).join("jank.log")
            });
        if let Some(dir) = path.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        let max = inner.filter().max(log::LevelFilter::Warn);
        let tee = Tee { inner, path, file: Mutex::new(None) };
        if log::set_boxed_logger(Box::new(tee)).is_ok() {
            log::set_max_level(max);
        }
    }

    impl Tee {
        fn append(&self, line: &str, truncate: bool) {
            let mut file = self.file.lock().unwrap_or_else(|e| e.into_inner());
            if truncate || file.is_none() {
                *file = std::fs::OpenOptions::new()
                    .create(true)
                    .write(true)
                    .append(!truncate)
                    .truncate(truncate)
                    .open(&self.path)
                    .ok();
            }
            if let Some(f) = file.as_mut() {
                let _ = writeln!(f, "{} {line}", stamp());
            }
        }
    }

    impl log::Log for Tee {
        fn enabled(&self, meta: &log::Metadata<'_>) -> bool {
            meta.target() == TARGET || self.inner.enabled(meta)
        }

        fn log(&self, record: &log::Record<'_>) {
            if record.target() == TARGET {
                let line = record.args().to_string();
                let cleared = line == nsh_engine::perf::RESET_MARK;
                self.append(&line, cleared);
            }
            if self.inner.matches(record) {
                self.inner.log(record);
            }
        }

        fn flush(&self) {
            self.inner.flush();
        }
    }

    /// Local wall-clock time, `HH:MM:SS.mmm` (the log is read next to
    /// what you were just doing).
    fn stamp() -> String {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default();
        let secs = now.as_secs() as i64;
        // SAFETY: `localtime_r` writes the struct it is given.
        let tm = unsafe {
            let mut tm: libc_tm = std::mem::zeroed();
            localtime_r(&secs, &mut tm);
            tm
        };
        format!("{:02}:{:02}:{:02}.{:03}", tm.tm_hour, tm.tm_min, tm.tm_sec, now.subsec_millis())
    }

    #[repr(C)]
    struct libc_tm {
        tm_sec: i32,
        tm_min: i32,
        tm_hour: i32,
        tm_mday: i32,
        tm_mon: i32,
        tm_year: i32,
        tm_wday: i32,
        tm_yday: i32,
        tm_isdst: i32,
        tm_gmtoff: i64,
        tm_zone: *const i8,
    }

    unsafe extern "C" {
        fn localtime_r(time: *const i64, out: *mut libc_tm) -> *mut libc_tm;
    }
}

/// The root the runners mount: the same component every platform shows.
pub fn app_root() -> nsh_core::element::Root {
    let root = nsh_core::element::Root::of::<rox::App>().app_id(rox::identity::APP_ID);
    rox::fonts().into_iter().fold(root, |root, font| root.font(font))
}

/// The UI sets the engine serves: nsh's base components and the app's.
#[cfg(any(target_os = "macos", target_os = "ios"))]
fn ui_sets() -> Vec<nsh_core::element::Registry> {
    vec![nsh_base::registry(), rox::registry()]
}

#[cfg(target_os = "macos")]
fn main() -> impl std::process::Termination {
    jank_log::init();
    // Rox's data dir becomes Buwiz Bot's, once, before anything opens it
    let home = std::path::PathBuf::from(std::env::var_os("HOME").unwrap_or_default());
    match rox::identity::migrate_data(std::env::var_os("ROX_DATA_DIR").as_deref(), &home) {
        Ok(done) => log::info!("rox: data dir migration: {done:?}"),
        Err(e) => log::error!("rox: Rox's data dir was not moved (it is used where it is): {e}"),
    }
    // the bundled broker, when none answers and this is no test instance
    // (plan 7.2)
    rox::broker::ensure_started();
    platform::set_ui_sets(ui_sets());
    let options = platform::AppOptions {
        title: rox::app_name().to_owned(),
        app_id: rox::identity::APP_ID.to_owned(),
        size: (1100, 760),
        // `ROX_FPS=1`: nsh's performance HUD (FPS, main-thread and commit
        // costs, CPU/GPU/memory) at the window's top right.
        fps: std::env::var("ROX_FPS").is_ok_and(|v| v != "0"),
        // A test instance (`ROX_BACKGROUND=1`, which scripts/test-rox sets)
        // starts in the background: launching it never takes the focus from
        // the app in front. The owner's Rox, automation on or not, comes
        // to the front as any app does.
        activate: std::env::var("ROX_BACKGROUND").as_deref() != Ok("1"),
        ..Default::default()
    };
    rox::mini::register_mini_app(&options.components);
    rox::zoom::register_zoom(&options.components);
    rox::keys::register_keys(&options.components);
    platform::run_app(platform::AppSource::Rust(app_root()), options)
}

#[cfg(target_os = "ios")]
fn main() {
    nsh_platform_ios::set_ui_sets(ui_sets());
    nsh_ios_app::run_rust(app_root());
}

#[cfg(not(any(target_os = "macos", target_os = "ios")))]
fn main() {
    eprintln!("rox runs on macOS (`cargo run`) and iOS (packaged by ../ios.sh)");
    std::process::exit(1);
}
