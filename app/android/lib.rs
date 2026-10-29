//! Buwiz's `libnsh.so`: the app's root with `nsh/base` and the app's own
//! components, run by Buwiz's `MainActivity` (nsh's `AppActivity` plus
//! printing). Before the root mounts, [`buwiz::mobile::prepare`] points the
//! database at the app's private files directory and the phone's printing
//! ([`print`]) is registered.

#[cfg(target_os = "android")]
mod print;
#[cfg(target_os = "android")]
use buwiz::bir_print_sink;

#[cfg(target_os = "android")]
nsh::ui_sets!(vec![nsh_base::registry(), buwiz::registry()]);
#[cfg(target_os = "android")]
nsh::main!({
    buwiz::mobile::prepare();
    buwiz::mobile::set_printer(Box::new(print::AndroidPrint));
    nsh_core::element::Root::of::<buwiz::App>().app_id(buwiz::APP_ID)
});

/// `Printing.nativeAttach(activity)`: the activity printing opens its
/// dialogs from.
#[cfg(target_os = "android")]
#[unsafe(no_mangle)]
pub extern "system" fn Java_com_buwiz_oss_Printing_nativeAttach<'local>(
    mut env: jni::JNIEnv<'local>,
    class: jni::objects::JClass<'local>,
    activity: jni::objects::JObject<'local>,
) {
    print::attach(&mut env, &class, &activity);
}

// Off Android it is empty, as nsh-platform-android is.
#[cfg(not(target_os = "android"))]
use buwiz as _;
#[cfg(not(target_os = "android"))]
use nsh_base as _;
#[cfg(not(target_os = "android"))]
use nsh_core as _;
