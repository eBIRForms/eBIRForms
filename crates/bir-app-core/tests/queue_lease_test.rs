//! A process that sends what it just queued (ebirdesk-mcp) runs the queue
//! only with the worker lease: never while another app holds it.

use bir_app_core::BirApp;
use bir_app_core::bir_core::db::WorkerLease;

const TEST_KEY: &str = "0000000000000000000000000000000000000000000000000000000000000000";

#[test]
fn the_queue_lease_is_refused_while_another_app_runs_the_queue() {
    // SAFETY: tests set the key before any database opens.
    unsafe { std::env::set_var("BUWIZ_DB_KEY", TEST_KEY) };
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("bir_data.db");
    let app = BirApp::open_at(&path).expect("open");

    let worker = WorkerLease::try_acquire(&path)
        .unwrap()
        .expect("the other app's worker");
    assert!(
        app.try_queue_lease().unwrap().is_none(),
        "another app holds the lease"
    );
    drop(worker);

    let lease = app
        .try_queue_lease()
        .unwrap()
        .expect("no other worker: this process may send");
    assert!(
        WorkerLease::try_acquire(&path).unwrap().is_none(),
        "held while this process sends"
    );
    drop(lease);
    assert!(
        WorkerLease::try_acquire(&path).unwrap().is_some(),
        "let go after"
    );
}
