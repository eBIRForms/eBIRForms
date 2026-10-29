//! Printing on Android, through JNI into Buwiz's `com.buwiz.oss.Printing`:
//! the system print dialog (PrintManager over a WebView's print adapter,
//! with Save as PDF), a PDF written by the same adapter (bir-print's
//! `PdfSink`), and the share sheet (ACTION_SEND through the app's
//! `PdfFiles` provider).
//!
//! `MainActivity` hands its activity over once it is created
//! (`Printing.nativeAttach`); the calls come from worker threads.

use std::path::PathBuf;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use buwiz::mobile::PhonePrint;
use bir_print_sink::PdfSink;
use jni::JavaVM;
use jni::objects::{GlobalRef, JClass, JObject, JValue};

use crate::bir_print_sink;

/// The VM, `Printing`'s class (found on the activity's thread, whose class
/// loader sees the app's classes) and the activity.
struct Attached {
    vm: JavaVM,
    class: GlobalRef,
    activity: GlobalRef,
}

static ATTACHED: Mutex<Option<Attached>> = Mutex::new(None);

pub(crate) fn attach(env: &mut jni::JNIEnv<'_>, class: &JClass<'_>, activity: &JObject<'_>) {
    let made = (|| -> jni::errors::Result<Attached> {
        Ok(Attached { vm: env.get_java_vm()?, class: env.new_global_ref(class)?, activity: env.new_global_ref(activity)? })
    })();
    match made {
        Ok(a) => *ATTACHED.lock().unwrap_or_else(|e| e.into_inner()) = Some(a),
        Err(e) => log::warn!("printing not attached: {e}"),
    }
}

/// Call the static `Printing.<name>(Activity, String, String)`.
fn call(name: &str, a: &str, b: &str) -> Result<(), String> {
    let guard = ATTACHED.lock().unwrap_or_else(|e| e.into_inner());
    let at = guard.as_ref().ok_or("printing is not ready yet")?;
    let mut env = at.vm.attach_current_thread().map_err(|e| e.to_string())?;
    let a = env.new_string(a).map_err(|e| e.to_string())?;
    let b = env.new_string(b).map_err(|e| e.to_string())?;
    let class: &JClass<'_> = at.class.as_obj().into();
    let result = env.call_static_method(
        class,
        name,
        "(Landroid/app/Activity;Ljava/lang/String;Ljava/lang/String;)V",
        &[JValue::Object(at.activity.as_obj()), JValue::Object(&a), JValue::Object(&b)],
    );
    if env.exception_check().unwrap_or(false) {
        let _ = env.exception_describe();
        let _ = env.exception_clear();
        return Err(format!("Printing.{name} failed"));
    }
    result.map(drop).map_err(|e| e.to_string())
}

/// The cache's `pdf` folder (`PdfFiles` serves it).
fn pdf_dir() -> PathBuf {
    let dir = std::env::temp_dir().join("pdf");
    let _ = std::fs::create_dir_all(&dir);
    dir
}

/// bir-print's `PdfSink` over the WebView print adapter.
pub(crate) struct WebViewPdf;

#[derive(Debug)]
pub(crate) struct PdfError(String);

impl std::fmt::Display for PdfError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for PdfError {}

impl PdfSink for WebViewPdf {
    type Error = PdfError;

    fn html_to_pdf(&self, html: &str) -> Result<Vec<u8>, PdfError> {
        let stamp = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_millis());
        let path = pdf_dir().join(format!("render-{stamp}.pdf"));
        let shown = path.display().to_string();
        let (done, fail) = (PathBuf::from(format!("{shown}.done")), PathBuf::from(format!("{shown}.fail")));
        call("renderPdf", html, &shown).map_err(PdfError)?;
        let started = Instant::now();
        loop {
            if done.exists() {
                let bytes = std::fs::read(&path).map_err(|e| PdfError(e.to_string()));
                let _ = std::fs::remove_file(&done);
                let _ = std::fs::remove_file(&path);
                return bytes;
            }
            if fail.exists() {
                let why = std::fs::read_to_string(&fail).unwrap_or_default();
                let _ = std::fs::remove_file(&fail);
                return Err(PdfError(format!("the PDF was not written: {why}")));
            }
            if started.elapsed() > Duration::from_secs(30) {
                return Err(PdfError("the PDF took too long".into()));
            }
            std::thread::sleep(Duration::from_millis(80));
        }
    }
}

/// What the app calls.
pub(crate) struct AndroidPrint;

impl PhonePrint for AndroidPrint {
    fn print(&self, html: &str, title: &str) -> Result<(), String> {
        call("print", html, title)
    }

    fn html_to_pdf(&self, html: &str) -> Result<Vec<u8>, String> {
        WebViewPdf.html_to_pdf(html).map_err(|e| e.to_string())
    }

    fn share_pdf(&self, pdf: &[u8], title: &str) -> Result<(), String> {
        let name: String = format!("{title}.pdf").chars().map(|c| if c.is_ascii_alphanumeric() || matches!(c, ' ' | '-' | '.' | '_') { c } else { '-' }).collect();
        std::fs::write(pdf_dir().join(&name), pdf).map_err(|e| e.to_string())?;
        call("share", &name, title)
    }
}
