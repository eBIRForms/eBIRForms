//! `ebirdesk-mcp`: see the library docs.
//!
//! `mcp` is served here so the signed manifest comes from this plugin's own
//! variable (`BUWIZ_EBIRDESK_MANIFEST`); `manifest`, `app-source` and
//! `--version` are the SDK's command line.

use std::process::ExitCode;

use buwiz_plugin::{Plugin, Server, serve_stdio};
use ebirdesk_mcp::{Ebirdesk, NAME, manifest};

fn usage() -> String {
    format!(
        "{NAME}: the desktop app without a window, as the eBIRForms Desktop mini app ({id}) for the Buwiz Bot.

Usage: {NAME} mcp                       speak MCP (JSON-RPC 2.0, one message per line) on stdin/stdout,
                                     Mini App Protocol 2.0 (what the Bot spawns)
       {NAME} manifest [--key-id <id>]  print the unsigned manifest as JCS text, for signing, under
                                     publisher key id {key} unless a test id is named
       {NAME} app-source <id>           print a TSX app's source exactly as served
       {NAME} --version | --help

It works on the eBIRForms desktop app's taxpayer database, and shares it with the
desktop app while both run.

Environment:
  {env}  the signed manifest to serve (default: {file} next to
                           this executable; without either the unsigned one is served, and the
                           Bot refuses the plugin)
  BUWIZ_DATABASE_PATH      the database file (default: the desktop app's data directory)
  BUWIZ_DB_KEY, BUWIZ_DB_KEY_FILE
                           the database key (default: the desktop app's keychain entry)
  BUWIZ_API_URL            the Buwiz server sending goes through (default: the app's setting)
",
        id = Ebirdesk::ID,
        key = Ebirdesk::RELEASE_KEY_ID,
        env = manifest::MANIFEST_ENV,
        file = manifest::file_name(),
    )
}

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args
        .iter()
        .map(String::as_str)
        .collect::<Vec<_>>()
        .as_slice()
    {
        ["mcp"] => run(),
        ["manifest"] | ["manifest", "--key-id", _] | ["app-source", _] | ["--version" | "-V"] => {
            // the SDK's command line; it opens nothing for these
            buwiz_plugin::cli::main::<Ebirdesk>(|| Err("not served here".into()))
        }
        ["--help" | "-h"] => {
            print!("{}", usage());
            ExitCode::SUCCESS
        }
        _ => {
            eprint!("{}", usage());
            ExitCode::from(2)
        }
    }
}

fn run() -> ExitCode {
    // stdout carries MCP: the console copy of the log goes to stderr (and
    // the app's rolling log file, as the app writes it).
    use bir_app_core::bir_core::logging;
    let developer_mode = logging::developer_mode_enabled();
    let _ = logging::install_stderr(logging::InstallOptions {
        developer_mode,
        default_filter: if developer_mode {
            "ebirdesk_mcp=debug,bir_app_core=debug,bir_core=info,bir_submit=debug,panic=error"
        } else {
            "ebirdesk_mcp=info,bir_app_core=info,bir_core=info,bir_submit=info,panic=error"
        },
    });
    let served = manifest::served();
    if let Some(problem) = manifest::staleness(&served) {
        tracing::warn!("{problem}");
    }
    let mut server = Server::with_manifest(Ebirdesk::open_live(), served);
    let outcome = serve_stdio(&mut server);
    server.plugin_mut().finish();
    match outcome {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("{NAME}: {error}");
            ExitCode::from(1)
        }
    }
}
