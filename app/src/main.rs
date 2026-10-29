//! Runs the app on the machine's own backend.
//!
//! iOS: `../ios.sh` builds this for the simulator. Android builds
//! the library in `android/` instead.
//!
//! macOS: `cargo run` opens the window; for live edits run it through the
//! daemon (`cargo ra-jit run`, then `cargo ra-jit build` after each edit;
//! `../dev.sh` watches and does that). Windows and Linux come in phase P6.

#[cfg(target_os = "macos")]
use nsh_platform_macos as platform;

/// The process logger: `env_logger` as before, plus every `nsh::jank`
/// line (a frame over budget, a slow engine wake-up, and what each did —
/// logged while the performance monitor runs, ⌘⇧F) appended to
/// `~/Library/Logs/Buwiz/jank.log` (`BUWIZ_JANK_LOG` overrides). The monitor's
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
            env_logger::Env::default().default_filter_or("warn"),
        )
        .build();
        let path = std::env::var_os("BUWIZ_JANK_LOG")
            .map(std::path::PathBuf::from)
            .unwrap_or_else(|| {
                let home = std::env::var_os("HOME").unwrap_or_default();
                std::path::Path::new(&home).join("Library/Logs/Buwiz/jank.log")
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
    nsh_core::element::Root::of::<buwiz::App>().app_id(buwiz::APP_ID)
}

#[cfg(target_os = "macos")]
fn main() -> impl std::process::Termination {
    // ⌘⇧F (Window ▸ Performance Monitor, nsh's app menu) shows the
    // frame monitor; its over-budget frames land in the jank log.
    jank_log::init();
    platform::set_ui_sets(vec![nsh_base::registry(), buwiz::registry()]);
    let options = platform::AppOptions {
        title: buwiz::APP_NAME.to_owned(),
        app_id: buwiz::APP_ID.to_owned(),
        size: (1180, 800),
        // `BUWIZ_FPS=1`: nsh's performance HUD at the window's top right.
        fps: std::env::var("BUWIZ_FPS").is_ok_and(|v| v != "0"),
        // `BUWIZ_TRACE=1 RUST_LOG=nsh::perf=info`: each commit's time,
        // ops and mount items in the log.
        trace: std::env::var("BUWIZ_TRACE").is_ok_and(|v| v != "0"),
        ..Default::default()
    };
    platform::run_app(platform::AppSource::Rust(app_root()), options)
}

/// The iOS simulator: `../ios.sh` packs this binary as an app.
#[cfg(target_os = "ios")]
fn main() {
    buwiz::mobile::prepare();
    nsh_platform_ios::set_ui_sets(vec![nsh_base::registry(), buwiz::registry()]);
    nsh_ios_app::run_rust(app_root());
}

#[cfg(not(any(target_os = "macos", target_os = "ios")))]
fn main() {
    eprintln!("buwiz runs on macOS and iOS (app/ios.sh); Android builds a library (app/android.sh)");
    std::process::exit(1);
}
