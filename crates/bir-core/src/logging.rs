//! Process logging for painted `bir` and `bir-shell`.
//!
//! Both binaries write the same daily files under [`crate::log_files::log_dir`]
//! (`ebirforms.YYYY-MM-DD.log`). The Logs screen in each UI tails today's
//! file; there is no ring buffer and no log table. A stdout-only subscriber
//! leaves that file missing, which is why `bir-shell`'s Logs tab stayed empty
//! and a shipped build (no console) kept nothing.
//!
//! Lines are scrubbed before they hit the file or the terminal: OAuth tokens,
//! passwords, and the database key. Taxpayer TINs are left as written.
//! Painted `bir` already logs them in full (`tin = …` on draft creation) and
//! does not mask them in the Logs tab.

use std::io::{self, Write};
use std::path::Path;
use std::sync::{Once, OnceLock};

use regex::Regex;
use tracing_subscriber::fmt::MakeWriter;
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::util::SubscriberInitExt;
use tracing_subscriber::{EnvFilter, Registry};

use crate::log_files::{self, LOG_FILE_PREFIX, LOG_FILE_SUFFIX};

/// Level token the Logs tab filters on. Matches the painted classifier:
/// the fmt layer pads `INFO` and `WARN` so the marker is ` INFO ` / ` WARN `,
/// and `ERROR` / `FATAL` are five characters (` ERROR `).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LogLineLevel {
    Error,
    Warn,
    Info,
    Other,
}

pub fn log_line_level(line: &str) -> LogLineLevel {
    if line.contains(" ERROR ") || line.contains(" FATAL ") {
        LogLineLevel::Error
    } else if line.contains(" WARN ") {
        LogLineLevel::Warn
    } else if line.contains(" INFO ") {
        LogLineLevel::Info
    } else {
        LogLineLevel::Other
    }
}

/// `DEVELOPER_MODE=true` (read before the database is open, so it takes
/// effect at startup).
pub fn developer_mode_enabled() -> bool {
    let _ = dotenvy::dotenv();
    std::env::var("DEVELOPER_MODE")
        .map(|value| value.eq_ignore_ascii_case("true"))
        .unwrap_or(false)
}

/// Painted `bir` directives when `RUST_LOG` is unset.
pub fn desktop_filter(developer_mode: bool) -> &'static str {
    if developer_mode {
        "bir=debug,bir_desktop=debug,bir_print=debug,bir_core=info"
    } else {
        "bir=info,bir_desktop=info,bir_print=error,bir_core=info"
    }
}

/// The app's directives when `RUST_LOG` is unset.
///
/// `bir_core=info` keeps queue and cron warnings. Unlisted targets stay at
/// error, same as the painted filter.
pub fn shell_filter(developer_mode: bool) -> &'static str {
    // `panic=error` is the hook's target. A directive list with no global
    // level leaves every other target off.
    if developer_mode {
        "buwiz=debug,bir_app_core=debug,bir_core=info,bir_print=debug,panic=error"
    } else {
        "buwiz=info,bir_app_core=info,bir_core=info,bir_print=error,panic=error"
    }
}

#[derive(Debug, Clone, Copy)]
pub struct InstallOptions {
    pub developer_mode: bool,
    pub default_filter: &'static str,
}

/// Install the process subscriber and the panic hook. A second call is a no-op
/// and reports whether the first call succeeded.
pub fn install(options: InstallOptions) -> bool {
    install_with_console(options, Console::Stdout)
}

/// [`install`], with the console copy on stderr: for a process whose stdout
/// carries a protocol (`bir-mini-app mcp` speaks MCP there).
pub fn install_stderr(options: InstallOptions) -> bool {
    install_with_console(options, Console::Stderr)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Console {
    Stdout,
    Stderr,
}

fn install_with_console(options: InstallOptions, console: Console) -> bool {
    static INSTALLED: OnceLock<bool> = OnceLock::new();
    *INSTALLED.get_or_init(|| match subscriber(options, console).try_init() {
        Ok(()) => {
            install_panic_hook();
            tracing::info!(
                "🔍 Tracing initialized (developer_mode: {})",
                options.developer_mode
            );
            true
        }
        Err(error) => {
            eprintln!("logging subscriber not installed: {error}");
            false
        }
    })
}

/// Run `body` on a thread-local subscriber built the same way as [`install`].
///
/// Tests use this so one case does not capture every later test in the process.
pub fn with_thread_subscriber<T>(options: InstallOptions, body: impl FnOnce() -> T) -> T {
    tracing::subscriber::with_default(subscriber(options, Console::Stdout), body)
}

fn subscriber(options: InstallOptions, console: Console) -> impl tracing::Subscriber + Send + Sync {
    let _ = dotenvy::dotenv();
    let env_filter = EnvFilter::try_from_default_env()
        .or_else(|_| EnvFilter::try_new(options.default_filter))
        .unwrap_or_else(|_| EnvFilter::new("error"));
    let file_layer = file_appender(&log_files::log_dir()).map(|appender| {
        tracing_subscriber::fmt::layer()
            .with_writer(RedactingMake::new(appender))
            .with_ansi(false)
            .with_target(true)
            .with_file(options.developer_mode)
            .with_line_number(options.developer_mode)
    });
    let stdout_layer = (console == Console::Stdout).then(|| {
        tracing_subscriber::fmt::layer()
            .with_writer(RedactingMake::new(std::io::stdout))
            .with_target(true)
            .with_file(options.developer_mode)
            .with_line_number(options.developer_mode)
    });
    let stderr_layer = (console == Console::Stderr).then(|| {
        tracing_subscriber::fmt::layer()
            .with_writer(RedactingMake::new(std::io::stderr))
            .with_ansi(false)
            .with_target(true)
            .with_file(options.developer_mode)
            .with_line_number(options.developer_mode)
    });
    Registry::default()
        .with(env_filter)
        .with(stdout_layer)
        .with(stderr_layer)
        .with(file_layer)
}

fn file_appender(dir: &Path) -> Option<tracing_appender::rolling::RollingFileAppender> {
    let _ = std::fs::create_dir_all(dir);
    tracing_appender::rolling::RollingFileAppender::builder()
        .rotation(tracing_appender::rolling::Rotation::DAILY)
        .filename_prefix(LOG_FILE_PREFIX)
        .filename_suffix(LOG_FILE_SUFFIX)
        .build(dir)
        .map_err(|error| eprintln!("log file appender unavailable: {error}"))
        .ok()
}

thread_local! {
    static PANIC_HOOK_GUARD: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

fn install_panic_hook() {
    static ONCE: Once = Once::new();
    ONCE.call_once(|| {
        let previous = std::panic::take_hook();
        std::panic::set_hook(Box::new(move |info| {
            report_panic(info);
            previous(info);
        }));
    });
}

/// Record a panic on the `panic` target. The hook calls this before the
/// previous hook. Re-entry (a subscriber that panics while logging) is ignored.
pub fn report_panic(info: &std::panic::PanicHookInfo<'_>) {
    let entered = PANIC_HOOK_GUARD.with(|guard| {
        if guard.get() {
            false
        } else {
            guard.set(true);
            true
        }
    });
    if !entered {
        return;
    }
    let payload = panic_payload(info);
    let location = match info.location() {
        Some(location) => {
            format!(
                "{}:{}:{}",
                location.file(),
                location.line(),
                location.column()
            )
        }
        None => "unknown location".to_string(),
    };
    tracing::error!(target: "panic", "panic at {location}: {payload}");
    PANIC_HOOK_GUARD.with(|guard| guard.set(false));
}

fn panic_payload(info: &std::panic::PanicHookInfo<'_>) -> String {
    if let Some(message) = info.payload().downcast_ref::<&str>() {
        (*message).to_string()
    } else if let Some(message) = info.payload().downcast_ref::<String>() {
        message.clone()
    } else {
        "non-string panic payload".to_string()
    }
}

/// Replace credential material in a log line. TINs are not touched: painted
/// `bir` logs them and the Logs tab shows those lines.
pub fn redact_secrets(input: &str) -> String {
    let mut text = input.to_string();
    // Bearer and token shapes first. A labeled `Authorization: Bearer …`
    // would otherwise keep only the first word and leave the token behind.
    text = bearer()
        .replace_all(&text, "Bearer [redacted]")
        .into_owned();
    text = google_access_token()
        .replace_all(&text, "[redacted]")
        .into_owned();
    text = google_refresh_token()
        .replace_all(&text, "[redacted]")
        .into_owned();
    text = labeled_secret()
        .replace_all(&text, "$1[redacted]")
        .into_owned();
    // Dev start routes print `recovery key XXXX-XXXX-XXXX-XXXX` with a space,
    // which the labeled form (it wants `=` or `:`) does not catch.
    text = recovery_key_phrase()
        .replace_all(&text, "$1[redacted]")
        .into_owned();
    text = text.replace(TEST_MASTER_KEY, "[redacted]");
    text = labeled_hex_key()
        .replace_all(&text, "$1[redacted]")
        .into_owned();
    text
}

/// The `EBIR_TEST_ENV` SQLCipher key. It is not a taxpayer secret, but it is
/// the database key and must not land in a log a person might attach to a report.
const TEST_MASTER_KEY: &str = "0000000000000000000000000000000000000000000000000000000000000000";

fn labeled_secret() -> &'static Regex {
    static PATTERN: OnceLock<Regex> = OnceLock::new();
    PATTERN.get_or_init(|| {
        Regex::new(
            r#"(?i)(\b(?:password|passwd|pwd|secret|access_token|refresh_token|id_token|client_secret|api_key|master_key|db_key|encryption_key|app_password|imap_app_password|oauth_access_token|oauth_refresh_token|authorization|recovery_key)\b["']?\s*[=:]\s*)(?:"[^"]*"|'[^']*'|\S+)"#,
        )
        .expect("secret redaction pattern")
    })
}

fn recovery_key_phrase() -> &'static Regex {
    static PATTERN: OnceLock<Regex> = OnceLock::new();
    PATTERN.get_or_init(|| {
        Regex::new(r"(?i)(\brecovery[_ ]key\b\s*[=:]?\s*)([0-9A-Z]{4}(?:-[0-9A-Z]{4}){3})")
            .expect("recovery key pattern")
    })
}

fn bearer() -> &'static Regex {
    static PATTERN: OnceLock<Regex> = OnceLock::new();
    PATTERN.get_or_init(|| Regex::new(r"(?i)\bBearer\s+\S+").expect("bearer pattern"))
}

fn google_access_token() -> &'static Regex {
    static PATTERN: OnceLock<Regex> = OnceLock::new();
    PATTERN.get_or_init(|| Regex::new(r"\bya29\.[0-9A-Za-z\-._~+/]+").expect("ya29 pattern"))
}

fn google_refresh_token() -> &'static Regex {
    static PATTERN: OnceLock<Regex> = OnceLock::new();
    PATTERN.get_or_init(|| Regex::new(r"\b1//[A-Za-z0-9_\-./]+").expect("refresh token pattern"))
}

fn labeled_hex_key() -> &'static Regex {
    static PATTERN: OnceLock<Regex> = OnceLock::new();
    PATTERN.get_or_init(|| {
        Regex::new(
            r"(?i)(\b(?:master_key|db_key|encryption_key|bir_key)\b[^0-9A-Fa-f]{0,24})([0-9A-Fa-f]{64})\b",
        )
        .expect("hex key pattern")
    })
}

struct RedactingMake<M> {
    inner: M,
}

impl<M> RedactingMake<M> {
    fn new(inner: M) -> Self {
        Self { inner }
    }
}

struct RedactingWriter<W: Write> {
    inner: W,
    pending: Vec<u8>,
}

impl<W: Write> RedactingWriter<W> {
    fn emit(&mut self) -> io::Result<()> {
        if self.pending.is_empty() {
            return Ok(());
        }
        let text = String::from_utf8_lossy(&self.pending);
        let mut redacted = String::new();
        for piece in text.split_inclusive('\n') {
            let (body, newline) = match piece.strip_suffix('\n') {
                Some(body) => (body, true),
                None => (piece, false),
            };
            let body = body.strip_suffix('\r').unwrap_or(body);
            redacted.push_str(&redact_secrets(body));
            if newline {
                redacted.push('\n');
            }
        }
        self.pending.clear();
        self.inner.write_all(redacted.as_bytes())
    }
}

impl<W: Write> Write for RedactingWriter<W> {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.pending.extend_from_slice(buf);
        Ok(buf.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        self.emit()?;
        self.inner.flush()
    }
}

impl<W: Write> Drop for RedactingWriter<W> {
    fn drop(&mut self) {
        let _ = self.emit();
        let _ = self.inner.flush();
    }
}

impl<'a, M> MakeWriter<'a> for RedactingMake<M>
where
    M: MakeWriter<'a>,
{
    type Writer = RedactingWriter<M::Writer>;

    fn make_writer(&'a self) -> Self::Writer {
        RedactingWriter {
            inner: self.inner.make_writer(),
            pending: Vec::new(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn desktop_filter_strings_are_the_painted_directives() {
        assert_eq!(
            desktop_filter(false),
            "bir=info,bir_desktop=info,bir_print=error,bir_core=info"
        );
        assert_eq!(
            desktop_filter(true),
            "bir=debug,bir_desktop=debug,bir_print=debug,bir_core=info"
        );
    }

    #[test]
    fn shell_filter_keeps_core_and_app_warnings() {
        let filter = shell_filter(false);
        assert!(filter.contains("bir_core=info"));
        assert!(filter.contains("buwiz=info"));
        assert!(filter.contains("bir_app_core=info"));
        assert!(filter.contains("panic=error"));
        assert!(shell_filter(true).contains("bir_app_core=debug"));
        assert!(EnvFilter::try_new(filter).is_ok());
        assert!(EnvFilter::try_new(shell_filter(true)).is_ok());
    }

    #[test]
    fn line_levels_match_the_painted_logs_tab() {
        assert_eq!(
            log_line_level("2026-09-12T00:00:00Z  WARN x: y"),
            LogLineLevel::Warn
        );
        assert_eq!(
            log_line_level("2026-09-12T00:00:00Z ERROR x: y"),
            LogLineLevel::Error
        );
        assert_eq!(
            log_line_level("2026-09-12T00:00:00Z FATAL x: y"),
            LogLineLevel::Error
        );
        assert_eq!(
            log_line_level("2026-09-12T00:00:00Z  INFO x: y"),
            LogLineLevel::Info
        );
        assert_eq!(
            log_line_level("continuation of a wrapped message"),
            LogLineLevel::Other
        );
    }

    #[test]
    fn redaction_strips_credentials_and_keeps_tins_and_ordinary_text() {
        let tin = "12345678800000";
        let line = format!(
            "draft tin={tin} password=s3cret-token access_token=\"ya29.SUPERSECRET\" Authorization: Bearer abc.def master_key={TEST_MASTER_KEY} refresh 1//0refreshTOKEN recovery key ABCD-EFGH-IJKL-MNOP Failed to access OS keyring"
        );
        let redacted = redact_secrets(&line);
        assert!(redacted.contains(tin), "{redacted}");
        assert!(
            redacted.contains("Failed to access OS keyring"),
            "{redacted}"
        );
        assert!(!redacted.contains("s3cret-token"), "{redacted}");
        assert!(!redacted.contains("ya29.SUPERSECRET"), "{redacted}");
        assert!(!redacted.contains("abc.def"), "{redacted}");
        assert!(!redacted.contains(TEST_MASTER_KEY), "{redacted}");
        assert!(!redacted.contains("1//0refreshTOKEN"), "{redacted}");
        assert!(!redacted.contains("ABCD-EFGH-IJKL-MNOP"), "{redacted}");
        assert!(redacted.contains("[redacted]"), "{redacted}");
        let digest = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";
        assert_eq!(redact_secrets(digest), digest);
        assert_eq!(redact_secrets(&line), redact_secrets(&redacted));
    }

    #[test]
    fn shell_subscriber_writes_errors_to_the_daily_file_and_drops_debug_noise() {
        let dir = tempfile::tempdir().unwrap();
        let options = InstallOptions {
            developer_mode: false,
            default_filter: shell_filter(false),
        };
        temp_env::with_vars(
            [
                ("BIR_LOG_DIR", Some(dir.path().as_os_str())),
                ("RUST_LOG", None),
            ],
            || {
                with_thread_subscriber(options, || {
                    tracing::warn!(
                        target: "bir_core::email",
                        "IMAP login failed for inbox@example.com password=s3cret-token"
                    );
                    tracing::error!(
                        target: "buwiz::screens",
                        "screen: render failed token=ya29.SUPERSECRET"
                    );
                    tracing::info!(
                        target: "bir_core",
                        "Creating new draft tin=12345678800000"
                    );
                    tracing::debug!(
                        target: "bir_core",
                        "hidden-debug-marker-9f3a"
                    );
                    tracing::info!(
                        target: "unrelated_crate",
                        "hidden-other-info-9f3a"
                    );
                    tracing::error!(
                        target: "unrelated_crate",
                        "hidden-other-error-9f3a"
                    );
                    let previous = std::panic::take_hook();
                    std::panic::set_hook(Box::new(|info| {
                        report_panic(info);
                    }));
                    let panicked = std::panic::catch_unwind(|| {
                        panic!("probe-panic-9f3a password=panic-secret");
                    });
                    std::panic::set_hook(previous);
                    assert!(panicked.is_err());
                });
                let path = log_files::today_log_path(dir.path());
                let text = std::fs::read_to_string(&path)
                    .unwrap_or_else(|error| panic!("read {}: {error}", path.display()));
                assert!(text.contains("IMAP login failed"), "{text}");
                assert!(text.contains("screen: render failed"), "{text}");
                assert!(text.contains("12345678800000"), "{text}");
                assert!(text.contains("probe-panic-9f3a"), "{text}");
                assert!(!text.contains("s3cret-token"), "{text}");
                assert!(!text.contains("ya29.SUPERSECRET"), "{text}");
                assert!(!text.contains("panic-secret"), "{text}");
                assert!(!text.contains("hidden-debug-marker-9f3a"), "{text}");
                assert!(!text.contains("hidden-other-info-9f3a"), "{text}");
                // No global level: an error from a crate the filter does not
                // name stays out, matching painted `bir`.
                assert!(!text.contains("hidden-other-error-9f3a"), "{text}");
                assert_eq!(
                    log_line_level(
                        text.lines()
                            .find(|line| line.contains("IMAP login failed"))
                            .unwrap()
                    ),
                    LogLineLevel::Warn
                );
                assert_eq!(
                    log_line_level(
                        text.lines()
                            .find(|line| line.contains("probe-panic-9f3a"))
                            .unwrap()
                    ),
                    LogLineLevel::Error
                );
            },
        );
    }
}
