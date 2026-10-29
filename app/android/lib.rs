//! Buwiz's `libnsh.so`: the app's root with `nsh/base` and the app's own
//! components, run by nsh's `AppActivity` (`Native.runRust`). Before the
//! root mounts, [`buwiz::mobile::prepare`] points the database at the app's
//! private files directory.

#[cfg(target_os = "android")]
nsh::ui_sets!(vec![nsh_base::registry(), buwiz::registry()]);
#[cfg(target_os = "android")]
nsh::main!({
    buwiz::mobile::prepare();
    nsh_core::element::Root::of::<buwiz::App>().app_id(buwiz::APP_ID)
});

// Off Android it is empty, as nsh-platform-android is.
#[cfg(not(target_os = "android"))]
use buwiz as _;
#[cfg(not(target_os = "android"))]
use nsh_base as _;
#[cfg(not(target_os = "android"))]
use nsh_core as _;
