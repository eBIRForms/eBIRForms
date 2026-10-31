//! Several real processes sharing one SQLCipher database.
//!
//! Each test runs twice over: as the parent, it spawns this test binary
//! again (`--exact <test>`) with `BUWIZ_SHARED_DB_CHILD` naming the child's
//! part, and reads what the children print. The key comes from
//! `BUWIZ_DB_KEY` with `BUWIZ_NO_KEYCHAIN` set, so no run touches the real
//! keychain; every database lives in a temp dir.

use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdout, Command, Stdio};
use std::sync::mpsc;
use std::time::{Duration, Instant};

use bir_core::db::{ChangeWatch, Database, WorkerLease, open_live_database, with_maintenance_lock};

const CHILD: &str = "BUWIZ_SHARED_DB_CHILD";
const ARG: &str = "BUWIZ_SHARED_DB_ARG";
const DIR: &str = "BUWIZ_SHARED_DB_DIR";
const KEY: &str = "5d1f0c6a3b2e4f798a0b1c2d3e4f5a6b7c8d9e0f1a2b3c4d5e6f708192a3b4c5";

fn child_role() -> Option<String> {
    std::env::var(CHILD).ok().filter(|role| !role.is_empty())
}

fn child_dir() -> PathBuf {
    PathBuf::from(std::env::var_os(DIR).expect("child has a dir"))
}

fn child_arg() -> String {
    std::env::var(ARG).unwrap_or_default()
}

fn say(line: &str) {
    println!("SHARED:{line}");
}

/// Wait for `name` in `dir` to exist (the parent's start signal).
fn wait_for_file(path: &Path) {
    let until = Instant::now() + Duration::from_secs(20);
    while !path.exists() {
        assert!(Instant::now() < until, "{} never appeared", path.display());
        std::thread::sleep(Duration::from_millis(5));
    }
}

/// A child running `test` as `role`, its output piped back line by line.
struct Kid {
    child: Child,
    lines: mpsc::Receiver<String>,
    reader: std::thread::JoinHandle<()>,
}

impl Kid {
    fn spawn(test: &str, role: &str, arg: &str, dir: &Path) -> Kid {
        let mut child = Command::new(std::env::current_exe().expect("test binary"))
            .args(["--exact", test, "--nocapture", "--test-threads=1"])
            .env(CHILD, role)
            .env(ARG, arg)
            .env(DIR, dir)
            .env("BUWIZ_DATABASE_PATH", dir.join("bir_data.db"))
            .env("BUWIZ_DB_KEY", KEY)
            .env("BUWIZ_NO_KEYCHAIN", "1")
            .env_remove("BUWIZ_DB_KEY_FILE")
            .env_remove("BUWIZ_TEST_ENV")
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .spawn()
            .expect("spawn child test process");
        let stdout: ChildStdout = child.stdout.take().unwrap();
        let (tx, lines) = mpsc::channel();
        let reader = std::thread::spawn(move || {
            for line in BufReader::new(stdout).lines().map_while(Result::ok) {
                // libtest may print "test <name> ... " ahead of it on the line.
                if let Some(at) = line.find("SHARED:") {
                    let _ = tx.send(line[at + "SHARED:".len()..].to_string());
                } else if line.contains("Applying database migration") {
                    let _ = tx.send("migrated".to_string());
                }
            }
        });
        Kid {
            child,
            lines,
            reader,
        }
    }

    /// The next thing the child said, waiting up to `secs`.
    fn next(&self, secs: u64) -> String {
        self.lines
            .recv_timeout(Duration::from_secs(secs))
            .expect("the child answered in time")
    }

    /// Everything it said until it exited; panics when it failed.
    fn finish(mut self) -> Vec<String> {
        let status = self.child.wait().expect("child exits");
        self.reader.join().expect("output reader");
        let said: Vec<String> = self.lines.try_iter().collect();
        assert!(status.success(), "child failed ({status}); said {said:?}");
        said
    }
}

#[test]
fn two_processes_write_the_same_database() {
    const TEST: &str = "two_processes_write_the_same_database";
    if let Some(role) = child_role() {
        assert_eq!(role, "writer");
        let me = child_arg();
        wait_for_file(&child_dir().join("go"));
        let (db, _, recovered) = open_live_database().expect("child opens the shared file");
        assert!(recovered.is_none());
        for n in 0..20 {
            db.set_setting(&format!("row.{me}.{n}"), &me)
                .expect("write");
        }
        let until = Instant::now() + Duration::from_secs(30);
        loop {
            let a = (0..20).all(|n| db.get_setting(&format!("row.A.{n}")).unwrap().is_some());
            let b = (0..20).all(|n| db.get_setting(&format!("row.B.{n}")).unwrap().is_some());
            if a && b {
                say("saw-both");
                break;
            }
            assert!(Instant::now() < until, "{me} never saw the other's rows");
            std::thread::sleep(Duration::from_millis(20));
        }
        // Stay open until the other has seen both as well.
        std::thread::sleep(Duration::from_millis(300));
        db.close()
            .expect("close while the other process may still be open");
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    let a = Kid::spawn(TEST, "writer", "A", dir.path());
    let b = Kid::spawn(TEST, "writer", "B", dir.path());
    std::fs::write(dir.path().join("go"), b"").unwrap();
    assert!(a.finish().contains(&"saw-both".to_string()));
    assert!(b.finish().contains(&"saw-both".to_string()));
}

#[test]
fn a_process_sees_another_process_change() {
    const TEST: &str = "a_process_sees_another_process_change";
    if let Some(role) = child_role() {
        let (db, _, _) = open_live_database().expect("open");
        match role.as_str() {
            "watcher" => {
                let mut watch = ChangeWatch::new(&db);
                db.set_setting("watcher", "own write").unwrap();
                assert!(!watch.poll(&db), "its own write is not another's change");
                say("ready");
                let until = Instant::now() + Duration::from_secs(20);
                while !watch.poll(&db) {
                    assert!(
                        Instant::now() < until,
                        "the other process's write never showed"
                    );
                    std::thread::sleep(Duration::from_millis(20));
                }
                assert_eq!(db.get_setting("poke").unwrap(), Some("1".into()));
                say("changed");
            }
            "poke" => db.set_setting("poke", "1").unwrap(),
            other => panic!("unknown role {other}"),
        }
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    let watcher = Kid::spawn(TEST, "watcher", "", dir.path());
    assert_eq!(watcher.next(30), "ready");
    Kid::spawn(TEST, "poke", "", dir.path()).finish();
    assert_eq!(watcher.next(30), "changed");
    watcher.finish();
}

#[test]
fn only_one_process_holds_the_worker_lease() {
    const TEST: &str = "only_one_process_holds_the_worker_lease";
    if let Some(role) = child_role() {
        let db_path = child_dir().join("bir_data.db");
        match role.as_str() {
            "holder" => {
                let lease = WorkerLease::try_acquire(&db_path).unwrap();
                assert!(lease.is_some(), "the first process takes the lease");
                say("holder");
                // Runs until the parent kills it (a crash: the OS frees the lock).
                std::thread::sleep(Duration::from_secs(60));
                drop(lease);
            }
            "follower" => {
                let first = WorkerLease::try_acquire(&db_path).unwrap();
                assert!(first.is_none(), "a second process follows");
                say("follower");
                let lease =
                    WorkerLease::acquire_or_follow(&db_path, Duration::from_millis(100), || false);
                assert!(lease.is_some());
                say("took-over");
            }
            other => panic!("unknown role {other}"),
        }
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    let mut holder = Kid::spawn(TEST, "holder", "", dir.path());
    assert_eq!(holder.next(30), "holder");
    let follower = Kid::spawn(TEST, "follower", "", dir.path());
    assert_eq!(follower.next(30), "follower");
    // Still following while the holder lives.
    assert!(
        follower
            .lines
            .recv_timeout(Duration::from_millis(500))
            .is_err()
    );
    holder.child.kill().unwrap();
    holder.child.wait().unwrap();
    assert_eq!(follower.next(30), "took-over");
    follower.finish();
}

/// A key store that, like the keychain, answers "missing" and then stores a
/// fresh key: slowly, so two unguarded first runs would both miss.
fn slow_get_or_create(key_file: &Path) -> String {
    if let Ok(key) = std::fs::read_to_string(key_file) {
        return key;
    }
    std::thread::sleep(Duration::from_millis(300));
    let key = format!("{:032x}", std::process::id() as u128 * 7919 + 1);
    std::fs::write(key_file, &key).unwrap();
    key
}

#[test]
fn two_first_runs_create_one_key() {
    const TEST: &str = "two_first_runs_create_one_key";
    if let Some(role) = child_role() {
        assert_eq!(role, "key");
        let dir = child_dir();
        wait_for_file(&dir.join("go"));
        // How `Database::open` reads or creates the master key.
        let key = with_maintenance_lock(&dir.join("bir_data.db"), || {
            Ok(slow_get_or_create(&dir.join("key.txt")))
        })
        .unwrap();
        say(&format!("key={key}"));
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    let a = Kid::spawn(TEST, "key", "", dir.path());
    let b = Kid::spawn(TEST, "key", "", dir.path());
    std::fs::write(dir.path().join("go"), b"").unwrap();
    let key_of = |said: Vec<String>| {
        said.into_iter()
            .find(|line| line.starts_with("key="))
            .unwrap()
    };
    let (ka, kb) = (key_of(a.finish()), key_of(b.finish()));
    assert_eq!(ka, kb, "two first runs must end with one key");
    let stored = std::fs::read_to_string(dir.path().join("key.txt")).unwrap();
    assert_eq!(ka, format!("key={stored}"));
}

#[test]
fn migrations_run_once_under_concurrency() {
    const TEST: &str = "migrations_run_once_under_concurrency";
    if let Some(role) = child_role() {
        assert_eq!(role, "migrate");
        let _ = tracing_subscriber::fmt()
            .with_writer(std::io::stdout)
            .with_ansi(false)
            .try_init();
        let dir = child_dir();
        wait_for_file(&dir.join("go"));
        let db = Database::open(dir.join("bir_data.db")).expect("concurrent first open");
        db.set_setting(&format!("opened.{}", child_arg()), "1")
            .unwrap();
        say("opened");
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    let kids: Vec<Kid> = (0..4)
        .map(|n| Kid::spawn(TEST, "migrate", &n.to_string(), dir.path()))
        .collect();
    std::fs::write(dir.path().join("go"), b"").unwrap();
    let mut migrations = 0;
    for kid in kids {
        let said = kid.finish();
        assert!(said.contains(&"opened".to_string()), "{said:?}");
        migrations += said.iter().filter(|line| *line == "migrated").count();
    }
    // Every schema step ran exactly once, in one of the four processes.
    let reopened = Kid::spawn(TEST, "migrate", "check", dir.path());
    std::fs::write(dir.path().join("go"), b"").unwrap();
    let after = reopened.finish();
    assert_eq!(after.iter().filter(|line| *line == "migrated").count(), 0);
    assert!(migrations > 0, "the first open migrated");
    let steps = migrations;
    let fresh = tempfile::tempdir().unwrap();
    let alone = Kid::spawn(TEST, "migrate", "alone", fresh.path());
    std::fs::write(fresh.path().join("go"), b"").unwrap();
    let alone_steps = alone
        .finish()
        .iter()
        .filter(|line| *line == "migrated")
        .count();
    assert_eq!(
        steps, alone_steps,
        "four racing opens applied the steps one open applies"
    );
}
