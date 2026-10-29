//! Runs the app on the machine's own backend.
//!
//! macOS: `cargo run` opens the window; for live edits run it through the
//! daemon (`cargo ra-jit run`, then `cargo ra-jit build` after each edit;
//! `../dev.sh` watches and does that). Windows and Linux come in phase P6.

#[cfg(target_os = "macos")]
use nsh_platform_macos as platform;

/// The root the runners mount: the same component every platform shows.
pub fn app_root() -> nsh_core::element::Root {
    nsh_core::element::Root::of::<buwiz::App>().app_id(buwiz::APP_ID)
}

#[cfg(target_os = "macos")]
fn main() -> impl std::process::Termination {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("warn")).init();
    platform::set_ui_sets(vec![nsh_base::registry(), buwiz::registry()]);
    let options = platform::AppOptions {
        title: buwiz::APP_NAME.to_owned(),
        app_id: buwiz::APP_ID.to_owned(),
        size: (1180, 800),
        // `BUWIZ_FPS=1`: nsh's performance HUD at the window's top right.
        fps: std::env::var("BUWIZ_FPS").is_ok_and(|v| v != "0"),
        ..Default::default()
    };
    platform::run_app(platform::AppSource::Rust(app_root()), options)
}

#[cfg(not(target_os = "macos"))]
fn main() {
    eprintln!("buwiz runs on macOS for now; Windows and Linux arrive in phase P6");
    std::process::exit(1);
}
