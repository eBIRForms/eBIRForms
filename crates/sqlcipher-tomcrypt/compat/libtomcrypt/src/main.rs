//! `compat-libtomcrypt write|read <db> [labels..]`: the SQLCipher Android
//! ships, opened as bir-core's `Database::open` opens a database (the same
//! key PRAGMA, busy timeout, WAL, synchronous and key check).

#[path = "../../common.rs"]
mod common;

use rusqlite::Connection;
use sqlcipher_tomcrypt as _;

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let (cmd, db) = (args[0].as_str(), &args[1]);
    let conn = open(db).unwrap_or_else(|e| fail(&format!("open {db}: {e}")));
    let provider: String = conn.query_row("PRAGMA cipher_provider", [], |r| r.get(0)).unwrap();
    if provider != "libtomcrypt" {
        fail(&format!("expected LibTomCrypt, linked {provider}"));
    }
    match cmd {
        "write" => common::write_marker(&conn, "libtomcrypt").unwrap_or_else(|e| fail(&e.to_string())),
        "read" => common::check(&conn, &args[2..]).unwrap_or_else(|e| fail(&e)),
        "settings" => common::print_settings(&conn).unwrap(),
        _ => fail("usage: write|read|settings <db> [labels..]"),
    }
}

/// bir-core's `Database::open`, minus migrations.
fn open(path: &str) -> rusqlite::Result<Connection> {
    let key_hex = std::env::var("BUWIZ_DB_KEY").expect("BUWIZ_DB_KEY");
    let conn = Connection::open(path)?;
    conn.execute_batch(&format!("PRAGMA key = \"x'{}'\";", key_hex))?;
    conn.busy_timeout(std::time::Duration::from_secs(5))?;
    conn.execute_batch(
        "PRAGMA journal_mode = WAL;
         PRAGMA synchronous = NORMAL;",
    )?;
    conn.query_row("SELECT count(*) FROM sqlite_master", [], |r| r.get::<_, i64>(0))?;
    Ok(conn)
}

fn fail(message: &str) -> ! {
    eprintln!("libtomcrypt: {message}");
    std::process::exit(1)
}
