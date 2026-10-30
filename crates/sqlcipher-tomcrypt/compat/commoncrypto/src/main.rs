//! `compat-commoncrypto write|read <db> [labels..]`: bir-core's own
//! `Database::open` (key from `BIR_DB_KEY`, its PRAGMAs, its migrations),
//! with the SQLCipher macOS ships.

#[path = "../../common.rs"]
mod common;

use bir_core::db::Database;

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let (cmd, db) = (args[0].as_str(), &args[1]);
    let database = Database::open(db).unwrap_or_else(|e| fail(&format!("open {db}: {e}")));
    let conn = database.conn_for_tests();
    let provider: String = conn.query_row("PRAGMA cipher_provider", [], |r| r.get(0)).unwrap();
    if provider != "commoncrypto" {
        fail(&format!("expected CommonCrypto, linked {provider}"));
    }
    match cmd {
        "write" => common::write_marker(conn, "commoncrypto").unwrap_or_else(|e| fail(&e.to_string())),
        "read" => common::check(conn, &args[2..]).unwrap_or_else(|e| fail(&e)),
        "settings" => common::print_settings(conn).unwrap(),
        _ => fail("usage: write|read|settings <db> [labels..]"),
    }
}

fn fail(message: &str) -> ! {
    eprintln!("commoncrypto: {message}");
    std::process::exit(1)
}
