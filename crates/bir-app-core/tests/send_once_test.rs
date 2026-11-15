//! The one-shot send (`BirApp::send_queue_once`, what ebirdesk-mcp runs after
//! queueing a return) puts its tick on a thread with a deep stack, so the
//! calling thread's own stack does not matter.

use bir_app_core::BirApp;
use bir_app_core::bir_submit::secrets::MemoryStore;
use bir_app_core::bir_submit::{Submission, SubmitConfig};

const TEST_KEY: &str = "0000000000000000000000000000000000000000000000000000000000000000";

#[test]
fn the_one_shot_send_runs_from_a_thread_with_a_small_stack() {
    // SAFETY: tests set the key before any database opens.
    unsafe { std::env::set_var("BUWIZ_DB_KEY", TEST_KEY) };
    let dir = tempfile::tempdir().unwrap();
    let app = BirApp::open_at(dir.path().join("bir_data.db")).expect("open");
    let lease = app.try_queue_lease().unwrap().expect("no other worker");
    // Nothing listens here and nothing is queued: the tick finds no return.
    let submission = Submission::with(
        SubmitConfig::new(Some("http://127.0.0.1:9")),
        Box::new(MemoryStore::default()),
    );
    let caller = std::thread::Builder::new()
        .name("small-stack-caller".into())
        .stack_size(64 * 1024)
        .spawn(move || {
            app.send_queue_once(&lease, &submission);
            app.send_queue_once(&lease, &submission);
        })
        .unwrap();
    caller
        .join()
        .expect("the send ran to its end on the small stack");
}
