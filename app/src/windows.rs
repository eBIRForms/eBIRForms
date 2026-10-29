//! Windows glue the print preview and Settings need, reached from worker
//! threads: [`SheetPdf`] (bir-print's `PdfSink` over WebView2's
//! `ICoreWebView2_7::PrintToPdf`), the PDF's print verb, and the common
//! file dialogs.
//!
//! Each call runs on a thread of its own, a COM single-threaded apartment
//! with its own message pump, so neither the engine nor nsh's UI thread
//! ever waits on WebView2 or a dialog.

use std::cell::{Cell, RefCell};
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use ::windows::Win32::Foundation::{HWND, RECT};
use ::windows::Win32::System::Com::{
    CLSCTX_INPROC_SERVER, COINIT_APARTMENTTHREADED, COINIT_DISABLE_OLE1DDE, CoCreateInstance,
    CoInitializeEx, CoTaskMemFree, CoUninitialize,
};
use ::windows::Win32::UI::Shell::Common::COMDLG_FILTERSPEC;
use ::windows::Win32::UI::Shell::{
    FOS_FILEMUSTEXIST, FOS_FORCEFILESYSTEM, FOS_OVERWRITEPROMPT, FileOpenDialog, FileSaveDialog,
    IFileDialog, IFileOpenDialog, IFileSaveDialog, IShellItem, SIGDN_FILESYSPATH, ShellExecuteW,
};
use ::windows::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DestroyWindow, DispatchMessageW, GetForegroundWindow,
    GetWindowThreadProcessId, MSG, PM_REMOVE, PeekMessageW, SW_SHOWNORMAL, TranslateMessage,
    WINDOW_EX_STYLE, WS_POPUP,
};
use ::windows::core::{BOOL, HRESULT, HSTRING, Interface, PCWSTR, w};
use bir_app_core::bir_print::pdf_sink::PdfSink;
use webview2_com::Microsoft::Web::WebView2::Win32::{
    COREWEBVIEW2_PRINT_ORIENTATION_PORTRAIT, ICoreWebView2_7, ICoreWebView2Controller,
    ICoreWebView2Environment, ICoreWebView2Environment6, ICoreWebView2PrintSettings,
};
use webview2_com::{
    CreateCoreWebView2ControllerCompletedHandler, CreateCoreWebView2EnvironmentCompletedHandler,
    NavigationCompletedEventHandler, PrintToPdfCompletedHandler,
};

use crate::desktop::{A4, Page, PdfError, file_filter, file_url, is_pdf, temp_file, with_extension, with_print_css};

// WebView2's loader: nsh-platform-windows links it statically
// (WebView2LoaderStatic), so the symbol is in the binary already.
unsafe extern "system" {
    #[link_name = "CreateCoreWebView2EnvironmentWithOptions"]
    fn create_environment(
        browser_folder: PCWSTR,
        user_data_folder: PCWSTR,
        options: *mut std::ffi::c_void,
        handler: *mut std::ffi::c_void,
    ) -> HRESULT;
}

/// How long a sheet may take to load and print.
const PDF_TIMEOUT: Duration = Duration::from_secs(60);

/// bir-print's `PdfSink` on Windows: a hidden WebView2 loads the sheet and
/// `PrintToPdf` writes it, A4, no margins, no headers or footers, one form
/// page per A4 page (the sheet CSS's page breaks).
pub(crate) struct SheetPdf {
    pub form_code: String,
}

/// One capture at a time.
static CAPTURE: Mutex<()> = Mutex::new(());

impl PdfSink for SheetPdf {
    type Error = PdfError;

    fn html_to_pdf(&self, html: &str) -> Result<Vec<u8>, PdfError> {
        let _one = CAPTURE.lock().unwrap_or_else(|e| e.into_inner());
        // From a file, not NavigateToString (2 MB at most; the sheets
        // inline their images).
        let page = temp_file(&self.form_code, "html");
        let out = temp_file(&self.form_code, "pdf");
        std::fs::write(&page, with_print_css(html)).map_err(|e| PdfError(format!("cannot write the sheet: {e}")))?;
        let (url, target) = (file_url(&page), out.clone());
        let made = std::thread::spawn(move || on_sta(|| print_to_pdf(&url, &target)))
            .join()
            .unwrap_or_else(|_| Err("the PDF thread panicked".into()));
        let bytes = made.and_then(|()| std::fs::read(&out).map_err(|e| format!("cannot read the PDF: {e}")));
        let _ = std::fs::remove_file(&page);
        let _ = std::fs::remove_file(&out);
        let bytes = bytes.map_err(PdfError)?;
        if !is_pdf(&bytes) {
            return Err(PdfError("WebView2 wrote no PDF".into()));
        }
        Ok(bytes)
    }
}

/// Runs `f` with COM initialized as a single-threaded apartment.
fn on_sta<T>(f: impl FnOnce() -> T) -> T {
    // SAFETY: plain COM initialization of this thread, undone below.
    let inited = unsafe { CoInitializeEx(None, COINIT_APARTMENTTHREADED | COINIT_DISABLE_OLE1DDE) }.is_ok();
    let out = f();
    if inited {
        // SAFETY: balances the successful CoInitializeEx above.
        unsafe { CoUninitialize() };
    }
    out
}

/// WebView2's profile for printing: `%LOCALAPPDATA%\Buwiz\WebView2`.
fn user_data_folder() -> PathBuf {
    let base = std::env::var_os("LOCALAPPDATA").map(PathBuf::from).unwrap_or_else(std::env::temp_dir);
    base.join("Buwiz").join("WebView2")
}

/// What the callbacks share with the pump.
#[derive(Default)]
struct Job {
    done: RefCell<Option<Result<(), String>>>,
    controller: RefCell<Option<ICoreWebView2Controller>>,
    printing: Cell<bool>,
}

impl Job {
    fn finish(&self, result: Result<(), String>) {
        let mut done = self.done.borrow_mut();
        if done.is_none() {
            *done = Some(result);
        }
    }
}

/// Loads `url` in a hidden WebView2 and prints it to `out`; pumps this
/// thread's messages until it is done.
fn print_to_pdf(url: &str, out: &Path) -> Result<(), String> {
    let (w, h) = A4.css_px();
    // SAFETY: a plain hidden popup of the system STATIC class, destroyed below.
    let host = unsafe {
        CreateWindowExW(WINDOW_EX_STYLE(0), w!("STATIC"), w!("Buwiz print"), WS_POPUP, 0, 0, w, h, None, None, None, None)
    }
    .map_err(|e| format!("no window for WebView2: {e}"))?;
    let job = Rc::new(Job::default());
    start(&job, host, url.to_owned(), out.to_owned());
    let deadline = Instant::now() + PDF_TIMEOUT;
    let mut msg = MSG::default();
    let result = loop {
        if let Some(result) = job.done.borrow_mut().take() {
            break result;
        }
        if Instant::now() > deadline {
            break Err("the sheet took too long to print".into());
        }
        // SAFETY: the standard message pump of this thread.
        unsafe {
            while PeekMessageW(&mut msg, None, 0, 0, PM_REMOVE).as_bool() {
                let _ = TranslateMessage(&msg);
                DispatchMessageW(&msg);
            }
        }
        std::thread::sleep(Duration::from_millis(5));
    };
    if let Some(controller) = job.controller.borrow_mut().take() {
        // SAFETY: closing the controller this thread made.
        let _ = unsafe { controller.Close() };
    }
    // SAFETY: the window made above, on its own thread.
    let _ = unsafe { DestroyWindow(host) };
    result
}

/// Environment → controller → navigation → `PrintToPdf`, each step in the
/// previous one's callback.
fn start(job: &Rc<Job>, host: HWND, url: String, out: PathBuf) {
    let folder = HSTRING::from(user_data_folder().as_os_str());
    let on_env = {
        let job = job.clone();
        CreateCoreWebView2EnvironmentCompletedHandler::create(Box::new(move |result, env| {
            let env = match (result, env) {
                (Ok(()), Some(env)) => env,
                (Err(e), _) => {
                    job.finish(Err(format!("WebView2 is not available: {e}")));
                    return Ok(());
                }
                _ => {
                    job.finish(Err("WebView2 is not available".into()));
                    return Ok(());
                }
            };
            make_controller(&job, host, env, url, out);
            Ok(())
        }))
    };
    // SAFETY: valid strings; the handler is an owned COM object the loader
    // keeps until it calls it.
    let started = unsafe {
        create_environment(PCWSTR::null(), PCWSTR(folder.as_ptr()), std::ptr::null_mut(), on_env.as_raw())
    };
    if let Err(e) = started.ok() {
        job.finish(Err(format!("cannot start WebView2 (is the WebView2 Runtime installed?): {e}")));
    }
}

fn make_controller(job: &Rc<Job>, host: HWND, env: ICoreWebView2Environment, url: String, out: PathBuf) {
    let on_controller = {
        let job = job.clone();
        let env = env.clone();
        CreateCoreWebView2ControllerCompletedHandler::create(Box::new(move |result, controller| {
            let controller = match (result, controller) {
                (Ok(()), Some(c)) => c,
                (Err(e), _) => {
                    job.finish(Err(format!("no WebView2: {e}")));
                    return Ok(());
                }
                _ => {
                    job.finish(Err("no WebView2".into()));
                    return Ok(());
                }
            };
            *job.controller.borrow_mut() = Some(controller.clone());
            if let Err(e) = navigate(&job, &controller, env, &url, out) {
                job.finish(Err(format!("cannot load the sheet: {e}")));
            }
            Ok(())
        }))
    };
    // SAFETY: a documented call on the live environment, on its thread.
    if let Err(e) = unsafe { env.CreateCoreWebView2Controller(host, &on_controller) } {
        job.finish(Err(format!("no WebView2: {e}")));
    }
}

fn navigate(
    job: &Rc<Job>,
    controller: &ICoreWebView2Controller,
    env: ICoreWebView2Environment,
    url: &str,
    out: PathBuf,
) -> ::windows::core::Result<()> {
    let (w, h) = A4.css_px();
    // SAFETY: documented calls on the controller just made, on its thread.
    unsafe {
        controller.SetBounds(RECT { left: 0, top: 0, right: w, bottom: h })?;
        let webview = controller.CoreWebView2()?;
        let on_loaded = {
            let job = job.clone();
            NavigationCompletedEventHandler::create(Box::new(move |sender, args| {
                if job.printing.replace(true) {
                    return Ok(());
                }
                let mut ok = BOOL(0);
                if let Some(args) = &args {
                    let _ = args.IsSuccess(&mut ok);
                }
                if !ok.as_bool() {
                    job.finish(Err("the sheet did not load".into()));
                    return Ok(());
                }
                let Some(webview) = sender else {
                    job.finish(Err("the web view went away".into()));
                    return Ok(());
                };
                if let Err(e) = print(&job, &webview.cast()?, &env, &out) {
                    job.finish(Err(format!("PrintToPdf failed (WebView2 Runtime 87 or later?): {e}")));
                }
                Ok(())
            }))
        };
        let mut token = 0i64;
        webview.add_NavigationCompleted(&on_loaded, &mut token)?;
        webview.Navigate(&HSTRING::from(url))?;
    }
    Ok(())
}

fn print(
    job: &Rc<Job>,
    webview: &ICoreWebView2_7,
    env: &ICoreWebView2Environment,
    out: &Path,
) -> ::windows::core::Result<()> {
    let settings = print_settings(env, &A4)?;
    let on_printed = {
        let job = job.clone();
        PrintToPdfCompletedHandler::create(Box::new(move |result, ok| {
            job.finish(match (result, ok) {
                (Ok(()), true) => Ok(()),
                (Err(e), _) => Err(format!("PrintToPdf failed: {e}")),
                _ => Err("PrintToPdf failed".into()),
            });
            Ok(())
        }))
    };
    // SAFETY: a documented call on the live web view, on its thread.
    unsafe { webview.PrintToPdf(&HSTRING::from(out.as_os_str()), &settings, &on_printed) }
}

/// WebView2 print settings for `page`: portrait, its size in inches, its
/// margins, scale 1, backgrounds on, no header or footer.
fn print_settings(env: &ICoreWebView2Environment, page: &Page) -> ::windows::core::Result<ICoreWebView2PrintSettings> {
    let env6: ICoreWebView2Environment6 = env.cast()?;
    let margin = page.margin_pt / 72.0;
    // SAFETY: documented setters on a settings object just made.
    unsafe {
        let s = env6.CreatePrintSettings()?;
        s.SetOrientation(COREWEBVIEW2_PRINT_ORIENTATION_PORTRAIT)?;
        s.SetPageWidth(page.width_in())?;
        s.SetPageHeight(page.height_in())?;
        s.SetMarginTop(margin)?;
        s.SetMarginBottom(margin)?;
        s.SetMarginLeft(margin)?;
        s.SetMarginRight(margin)?;
        s.SetScaleFactor(1.0)?;
        s.SetShouldPrintBackgrounds(true)?;
        s.SetShouldPrintHeaderAndFooter(false)?;
        s.SetShouldPrintSelectionOnly(false)?;
        Ok(s)
    }
}

/// Hands the PDF at `path` to the system's printing: the PDF handler's
/// print verb, or opens it (where Print is one click) when it has none.
pub(crate) fn print_pdf(path: &Path) -> Result<(), String> {
    let file = HSTRING::from(path.as_os_str());
    std::thread::spawn(move || {
        on_sta(|| {
            for verb in [w!("print"), w!("open")] {
                // SAFETY: valid strings; ShellExecuteW returns > 32 on success.
                let code = unsafe { ShellExecuteW(None, verb, &file, PCWSTR::null(), PCWSTR::null(), SW_SHOWNORMAL) };
                if code.0 as isize > 32 {
                    return Ok(());
                }
            }
            Err("no app on this PC opens PDF files".to_owned())
        })
    })
    .join()
    .unwrap_or_else(|_| Err("the print thread panicked".into()))
}

/// A save dialog suggesting `name` (with `ext`); the chosen path, `None`
/// when cancelled.
pub(crate) fn save_panel(name: &str, ext: &'static str) -> Option<PathBuf> {
    let name = with_extension(name, ext);
    std::thread::spawn(move || {
        on_sta(|| {
            // SAFETY: the shell's documented file dialog, on this STA thread.
            unsafe {
                let dialog: IFileSaveDialog = CoCreateInstance(&FileSaveDialog, None, CLSCTX_INPROC_SERVER).ok()?;
                let _ = dialog.SetFileName(&HSTRING::from(name.as_str()));
                let _ = dialog.SetDefaultExtension(&HSTRING::from(ext));
                let _ = dialog.SetOptions(dialog.GetOptions().ok()? | FOS_OVERWRITEPROMPT | FOS_FORCEFILESYSTEM);
                run_dialog(&dialog.cast().ok()?, ext)
            }
        })
    })
    .join()
    .ok()
    .flatten()
}

/// An open dialog for one file with extension `ext`.
pub(crate) fn open_panel(ext: &'static str) -> Option<PathBuf> {
    std::thread::spawn(move || {
        on_sta(|| {
            // SAFETY: the shell's documented file dialog, on this STA thread.
            unsafe {
                let dialog: IFileOpenDialog = CoCreateInstance(&FileOpenDialog, None, CLSCTX_INPROC_SERVER).ok()?;
                let _ = dialog.SetOptions(dialog.GetOptions().ok()? | FOS_FILEMUSTEXIST | FOS_FORCEFILESYSTEM);
                run_dialog(&dialog.cast().ok()?, ext)
            }
        })
    })
    .join()
    .ok()
    .flatten()
}

/// Filters `dialog` to `ext`, shows it over the app's window and returns
/// the chosen file's path (`None` when cancelled).
///
/// # Safety
/// On a COM STA thread.
unsafe fn run_dialog(dialog: &IFileDialog, ext: &str) -> Option<PathBuf> {
    let (label, pattern) = file_filter(ext);
    let (label, pattern) = (HSTRING::from(label), HSTRING::from(pattern));
    let spec = [COMDLG_FILTERSPEC { pszName: PCWSTR(label.as_ptr()), pszSpec: PCWSTR(pattern.as_ptr()) }];
    // SAFETY: the caller's STA thread; the strings outlive the calls.
    unsafe {
        let _ = dialog.SetFileTypes(&spec);
        // Cancelled: an error (ERROR_CANCELLED).
        dialog.Show(app_window()).ok()?;
        let item: IShellItem = dialog.GetResult().ok()?;
        let text = item.GetDisplayName(SIGDN_FILESYSPATH).ok()?;
        let path = text.to_string().ok();
        CoTaskMemFree(Some(text.0 as *const _));
        path.map(PathBuf::from)
    }
}

/// The foreground window when it is this app's (the dialog's owner).
fn app_window() -> Option<HWND> {
    // SAFETY: plain queries about a window handle.
    unsafe {
        let hwnd = GetForegroundWindow();
        let mut pid = 0u32;
        GetWindowThreadProcessId(hwnd, Some(&mut pid as *mut u32));
        (!hwnd.is_invalid() && pid == std::process::id()).then_some(hwnd)
    }
}
