//! A4 print layout for the frozen BIR sheets.
//!
//! Each `.page` sits in a paper-sized `.sheet` (the wrappers are
//! written by [`crate::static_html`]), the page is zoomed to fit A4, and
//! html/body are pinned to the paper width so WebKit doesn't re-scale the
//! print layout. Without that pin a 4-page form printed as 8 pages.

pub const PAPER_WIDTH_PT: f32 = 595.28;
pub const PAPER_HEIGHT_PT: f32 = 841.89;
const SAFETY_PT: f32 = 10.0;

/// The sheet's native `@page size` (e.g. 612pt × 936pt folio).
fn document_sheet_size(html: &str) -> Option<(f32, f32)> {
    let rest = &html[html.find("@page")?..];
    let rest = &rest[rest.find("size:")? + 5..];
    let end = rest.find([';', '}'])?;
    let mut parts = rest[..end].split_whitespace();
    let w = parts.next()?.trim_end_matches("pt").parse::<f32>().ok()?;
    let h = parts.next()?.trim_end_matches("pt").parse::<f32>().ok()?;
    Some((w, h))
}

fn fit_scale(native: (f32, f32)) -> f32 {
    let sx = (PAPER_WIDTH_PT - 2.0 * SAFETY_PT) / native.0;
    let sy = (PAPER_HEIGHT_PT - 2.0 * SAFETY_PT) / native.1;
    sx.min(sy).min(1.0)
}

fn fit_css(native: (f32, f32)) -> String {
    let scale = fit_scale(native);
    format!(
        "@page{{size:{w:.2}pt {h:.2}pt;margin:0}}\
html,body{{width:{w:.2}pt;margin:0 auto;padding:0;background:#fff}}\
body>:not(.sheet){{display:none !important}}\
.sheet{{position:relative;width:{w:.2}pt;height:{sheet_h:.2}pt;overflow:hidden;background:#fff;padding-top:{safety:.0}pt;break-after:page;page-break-after:always}}\
.sheet:last-of-type{{break-after:auto;page-break-after:auto}}\
.sheet-flow{{height:auto;min-height:{sheet_h:.2}pt;overflow:visible}}\
.sheet>.page{{zoom:{scale:.4};margin:0 auto;break-after:auto;page-break-after:auto}}",
        w = PAPER_WIDTH_PT,
        h = PAPER_HEIGHT_PT,
        sheet_h = PAPER_HEIGHT_PT - 1.0,
        safety = SAFETY_PT,
    )
}

/// Screen-only extras (grey desk, gap and shadow between sheets, page label).
/// Inside `@media screen` and limited to `.sheet` margins/shadow, never its
/// height, so the print layout is exactly `fit_css`.
const SCREEN_CSS: &str = "@media screen{\
html{width:auto !important;background:#5f6368 !important}\
body{background:transparent !important;padding:24px 0 8px !important}\
.sheet{margin:0 auto 32px auto;box-shadow:0 6px 22px rgba(0,0,0,.45)}\
.sheet::after{content:attr(data-preview-label);position:absolute;right:8px;bottom:4px;\
font:600 9px -apple-system,sans-serif;color:#888;pointer-events:none}}";

/// Add the A4 paper CSS so the filled sheet prints one A4 page per form page.
/// Run [`crate::static_html::staticize`] on the result to get the `.sheet`
/// wrappers and the script-free markup.
pub fn prepare_html(html: &str) -> String {
    let native = document_sheet_size(html).unwrap_or((612.0, 936.0));
    let style = format!(
        "<style id=\"preview-paper\">{}{}</style>",
        fit_css(native),
        SCREEN_CSS
    );
    match html.rfind("</head>") {
        Some(at) => format!("{}{}{}", &html[..at], style, &html[at..]),
        None => format!("{style}{html}"),
    }
}

/// Per-form override layer: `assets/print-overrides/<slug>.css` in this crate
/// (`print-overrides/` once packaged), read at render time
/// and injected after the frozen CSS for that slug only. The frozen HTML is
/// never edited.
///
/// Lookup order:
/// 1. `BIR_PRINT_OVERRIDES` — tests and overrides always win.
/// 2. The packaged layout beside the running executable
///    ([`crate::packaged`]): `Contents/Resources/print-overrides` in a macOS
///    bundle, `print-overrides/` beside the executable in a Windows/Linux
///    install or an unpacked archive, and `/usr/share/ebirforms/print-overrides`
///    for deb/rpm/pacman installs.
/// 3. Development checkout `crates/bir-print/assets/print-overrides` — debug builds only,
///    because a shipped binary must not resolve a path baked in at build time.
pub fn overrides_dir() -> std::path::PathBuf {
    overrides_dir_from(
        std::env::var_os("BIR_PRINT_OVERRIDES"),
        std::env::current_exe().ok(),
    )
}

fn overrides_dir_from(
    env_override: Option<std::ffi::OsString>,
    executable: Option<std::path::PathBuf>,
) -> std::path::PathBuf {
    if let Some(dir) = env_override {
        return std::path::PathBuf::from(dir);
    }
    if let Some(dir) = crate::packaged::dir(
        "print-overrides",
        executable.as_deref(),
        crate::packaged::directory_present,
    ) {
        return dir;
    }
    dev_overrides_dir()
}

/// The checkout's `assets/print-overrides/` for development runs and tests.
#[cfg(debug_assertions)]
fn dev_overrides_dir() -> std::path::PathBuf {
    std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("assets/print-overrides")
}

/// No packaged `print-overrides/` was found: the location it would have
/// occupied beside the executable (which simply reads as "no override").
#[cfg(not(debug_assertions))]
fn dev_overrides_dir() -> std::path::PathBuf {
    std::env::current_exe()
        .ok()
        .and_then(|exe| exe.parent().map(|dir| dir.join("print-overrides")))
        .unwrap_or_else(|| std::path::PathBuf::from("print-overrides"))
}

/// [`prepare_html`] plus `slug`'s override stylesheet (when `apply_override`
/// and the file exists).
pub fn prepare_html_for(slug: &str, html: &str, apply_override: bool) -> String {
    let html = prepare_html(html);
    if !apply_override {
        return html;
    }
    let Ok(css) = std::fs::read_to_string(overrides_dir().join(format!("{slug}.css"))) else {
        return html;
    };
    inject_override(slug, &html, &css)
}

fn inject_override(slug: &str, html: &str, css: &str) -> String {
    let tag = format!("<style id=\"form-override\" data-slug=\"{slug}\">{css}</style>");
    match html.rfind("</head>") {
        Some(at) => format!("{}{}{}", &html[..at], tag, &html[at..]),
        None => format!("{tag}{html}"),
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn folio_sheets_scale_to_fit_a4() {
        let scale = super::fit_scale((612.0, 936.0));
        assert!((scale - (841.89 - 20.0) / 936.0).abs() < 1e-4);
        // Never upscale a sheet that already fits.
        assert_eq!(super::fit_scale((400.0, 500.0)), 1.0);
    }

    #[test]
    fn reads_the_native_page_size_and_injects_paper_css() {
        let html =
            "<html><head><style>@page { size: 612pt 936pt; }</style></head><body></body></html>";
        assert_eq!(super::document_sheet_size(html), Some((612.0, 936.0)));
        let out = super::prepare_html(html);
        assert!(out.contains("id=\"preview-paper\""));
        assert!(out.find("preview-paper").unwrap() < out.find("</head>").unwrap());
    }

    #[test]
    fn override_lands_after_the_paper_css_inside_head() {
        let html = super::prepare_html(
            "<html><head><style>@page{size:612pt 936pt}</style></head><body></body></html>",
        );
        let out = super::inject_override("bir-1701", &html, ".x{top:1pt}");
        let tag = out
            .find("<style id=\"form-override\" data-slug=\"bir-1701\">.x{top:1pt}</style>")
            .unwrap();
        assert!(out.find("id=\"preview-paper\"").unwrap() < tag);
        assert!(tag < out.find("</head>").unwrap());
    }

    #[test]
    fn missing_override_file_is_a_no_op() {
        let html = "<html><head></head><body></body></html>";
        assert_eq!(
            super::prepare_html_for("no-such-slug-xyz", html, true),
            super::prepare_html(html)
        );
    }

    #[test]
    fn overrides_env_override_wins_over_the_packaged_layout() {
        let tag = format!("bir-print-overrides-{}", std::process::id());
        let root = std::env::temp_dir().join(tag);
        let _ = std::fs::remove_dir_all(&root);
        let bin = root.join("install-dir");
        std::fs::create_dir_all(bin.join("print-overrides")).expect("packaged overrides");
        let exe = bin.join("bir");
        std::fs::write(&exe, "").expect("exe");

        let chosen = std::path::PathBuf::from("/override/print-overrides");
        assert_eq!(
            super::overrides_dir_from(Some(chosen.clone().into_os_string()), Some(exe)),
            chosen
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn packaged_overrides_beside_the_executable_are_used() {
        let tag = format!("bir-print-overrides-pkg-{}", std::process::id());
        let root = std::env::temp_dir().join(tag);
        let _ = std::fs::remove_dir_all(&root);
        let bin = root.join("install-dir");
        let overrides = bin.join("print-overrides");
        std::fs::create_dir_all(&overrides).expect("packaged overrides");
        let exe = bin.join("bir");
        std::fs::write(&exe, "").expect("exe");

        // The resolver canonicalizes the executable path, so compare against
        // the resolved root (/var -> /private/var on macOS).
        let expected = overrides.canonicalize().unwrap_or(overrides);
        assert_eq!(super::overrides_dir_from(None, Some(exe)), expected);
        let _ = std::fs::remove_dir_all(&root);
    }
}
