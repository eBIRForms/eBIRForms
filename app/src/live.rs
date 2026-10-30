//! The bridge between `ra-jit-live` and nsh's engine thread.
//!
//! The daemon answers each edit with a patch: the functions that differ,
//! linked against this executable and written where [`Live::start`]
//! reserved room beside it. `install` polls for patches inside an engine
//! task — a point where no reactive work is half done — and on each one
//! applied, snapshots the [`keep`] signals and bumps `remount`. The
//! `dynamic` region under `remount` then rebuilds: components under it
//! never match an old element, so each is created again and its `view`
//! runs the new code. What `keep` marked holds its value through the
//! swap; nothing else is carried: a patch that changes a type the program
//! can see is answered `Restart` by the daemon, not applied.
//!
//! The app's model does not ride `keep`: [`store`] anchors the whole
//! `Store` — every signal of the filer's state — under the root owner, so a
//! remount redraws against the same data (an open return keeps its
//! values, the selected taxpayer stays selected). This file is the
//! dependency the anchor lives in: it is never re-emitted by a patch, so
//! its map is only ever the executable's one, on every platform.
//!
//! ra-jit-live is in only with the `live` feature (dev.sh, ios-dev.sh).
//! Without it `install` reports `off`, as a plain `cargo run` does without
//! the daemon, and the anchors are kept by `anchor` here.

use std::{any::Any, cell::RefCell, collections::HashMap, rc::Rc};
#[cfg(feature = "live")]
use std::time::Duration;

use nsh_core::prelude::*;

use crate::store::Store;

/// How often the engine polls for a patch: the apply itself is under a
/// millisecond, so this sets save-to-screen latency more than cost.
#[cfg(feature = "live")]
const POLL: Duration = Duration::from_millis(25);

// ---------------------------------------------------------------------------
// State anchor — how hot code reaches THE store.
//
// A `static` in patchable code is hopeless twice over: a local one has no
// name the splice can match, so each patch emits a private copy of it (the
// "empty board" bug), and a named one is written back to its initializer
// on every apply. A `thread_local` shares on macOS but cannot splice on
// the iOS simulator at all. `ra_jit_live::anchor` is the way out: a
// non-generic function in a dependency — code a patch never re-emits — so
// its map is only ever the executable's one, on every platform.
// ---------------------------------------------------------------------------

#[cfg(feature = "live")]
use ra_jit_live::anchor;

/// `ra_jit_live::anchor` without ra-jit: nothing patches the program, so a
/// plain list keeps each name's value, made by `init` on first use.
#[cfg(not(feature = "live"))]
fn anchor(name: &'static str, init: fn() -> usize) -> usize {
    static ANCHORS: std::sync::Mutex<Vec<(&str, usize)>> = std::sync::Mutex::new(Vec::new());
    let mut anchors = ANCHORS.lock().unwrap();
    if let Some(&(_, at)) = anchors.iter().find(|&&(n, _)| n == name) {
        at
    } else {
        let at = init();
        anchors.push((name, at));
        at
    }
}

/// Called once from `App`'s body — under the root owner, which never
/// remounts. `store()` anchors the model while that owner is current, so
/// its signals live as long as the app.
pub(crate) fn init() {
    // A new engine (Android remakes its activity, and with it the engine,
    // on some configuration changes) gets a new store: the old one's
    // signals belong to the engine that is gone. A patch remount never
    // comes here, so it keeps the store.
    let slot = store_slot();
    slot.set(Box::leak(Box::new(Store::new())) as *const Store as usize);
    store().load_catalog();
    store().open_database();
}

/// Where the current store's address is kept: anchored, so every patch
/// generation reads the same slot.
fn store_slot() -> &'static std::cell::Cell<usize> {
    let at = anchor("buwiz.store.slot", || {
        Box::leak(Box::new(std::cell::Cell::new(0usize))) as *const std::cell::Cell<usize> as usize
    });
    // SAFETY: the anchor holds the address of a leaked `Cell<usize>`, made
    // once and never freed; only the engine thread touches it.
    unsafe { &*(at as *const std::cell::Cell<usize>) }
}

/// The one model the engine has — every patch generation reads and writes
/// the same signals, so the open return survives remounts, not just
/// values.
pub(crate) fn store() -> &'static Store {
    let at = store_slot().get();
    assert!(at != 0, "live::init makes the store before anything reads it");
    // SAFETY: `init` stores the address of a leaked `Store`, never freed.
    unsafe { &*(at as *const Store) }
}

/// The opened database, kept for the whole process: a new engine (and
/// store) takes it over instead of opening the file again, which the
/// owner lock of the first opening would refuse.
pub(crate) fn database() -> &'static std::sync::Mutex<Option<std::sync::Arc<bir_app_core::BirApp>>> {
    let at = anchor("buwiz.database", || {
        Box::leak(Box::new(std::sync::Mutex::new(None::<std::sync::Arc<bir_app_core::BirApp>>))) as *const _ as usize
    });
    // SAFETY: the anchor holds the address of a leaked mutex, made once.
    unsafe { &*(at as *const std::sync::Mutex<Option<std::sync::Arc<bir_app_core::BirApp>>>) }
}

thread_local! {
    /// The live session (patch room behind the image, the watcher's patch
    /// file). Started once, with the app.
    #[cfg(feature = "live")]
    static LIVE: RefCell<Option<ra_jit_live::Live>> = const { RefCell::new(None) };
    /// What reads each `keep` signal now: refreshed as components mount.
    static READERS: RefCell<HashMap<&'static str, Rc<dyn Fn() -> Box<dyn Any>>>> =
        RefCell::new(HashMap::new());
    /// What the last snapshot left: read once by the next `keep` of the
    /// name, which registers its own reader instead.
    static KEPT: RefCell<HashMap<&'static str, Box<dyn Any>>> = RefCell::new(HashMap::new());
}

/// A signal that survives a hot remount.
///
/// `keep("counter.count", || 0)` reads like `signal(0)` except: before the
/// hot area rebuilds, its value is written into a table under `name`, and
/// the `keep` of the name in the rebuilt tree starts from that value. The
/// name is the identity — keep the same name (and type) across edits, or
/// the old value is dropped and `init` runs.
///
/// The snapshot runs before the rebuild (nsh reconciles new elements
/// before it releases the old ones, so an `on_cleanup` on the outgoing
/// instance would run too late to be read by its replacement).
pub fn keep<T: PartialEq + Clone + 'static>(name: &'static str, init: impl FnOnce() -> T) -> Signal<T> {
    let restored = KEPT.with(|kept| {
        kept.borrow_mut()
            .remove(name)
            .and_then(|value| value.downcast::<T>().ok())
            .map(|value| *value)
    });
    let sig = signal(restored.unwrap_or_else(init));
    READERS.with(|readers| {
        readers.borrow_mut().insert(
            name,
            Rc::new(move || Box::new(sig.get_untracked()) as Box<dyn Any>),
        );
    });
    sig
}

/// Starts `ra_jit_live::Live` and polls it on the engine thread.
///
/// - `Outcome::Applied`: snapshots the `keep` signals, bumps `remount`
///   (which rebuilds the hot area), and reports it on `note`.
/// - `Outcome::Restart` / `Outcome::Declined`: reports on `note`; the app
///   runs on as it was.
///
/// Call it once, under the root, which is never remounted itself: the
/// `Live` it holds and the interval it spawns belong to that owner.
#[cfg(feature = "live")]
pub fn install(remount: Signal<u64>, note: Signal<String>) {
    // `note` is `<state>:<message>` — `live`, `applied`, `restart`,
    // `declined` or `off` (no ra-jit daemon: a plain `cargo run`) — the
    // pill shows the state as a dot and the message on hover.
    LIVE.with(|live| {
        if live.borrow().is_none() {
            *live.borrow_mut() = ra_jit_live::Live::start().ok();
        }
        note.set(if live.borrow().is_some() {
            "live:ra-jit live".to_owned()
        } else {
            "off:ra-jit is not connected (plain cargo run)".to_owned()
        });
    });
    set_interval(POLL, move || {
        let outcome =
            LIVE.with(|live| live.borrow_mut().as_mut().and_then(ra_jit_live::Live::poll));
        let Some(outcome) = outcome else {
            return;
        };
        match outcome {
            ra_jit_live::Outcome::Applied { .. } => {
                let line = outcome.to_string();
                snapshot();
                remount.update(|v| *v = v.wrapping_add(1));
                note.set(format!("applied:{line}"));
            }
            ra_jit_live::Outcome::Restart(_) => note.set(format!("restart:{outcome}")),
            ra_jit_live::Outcome::Declined(_) => note.set(format!("declined:{outcome}")),
        }
    });
}

/// Without the `live` feature there is no daemon to reach: `note` reads
/// `off`, as when a live build finds none, and nothing is polled.
#[cfg(not(feature = "live"))]
pub fn install(_remount: Signal<u64>, note: Signal<String>) {
    note.set("off:ra-jit is not connected (plain cargo run)".to_owned());
}

/// Reads every `keep` signal into the table its replacement will read.
#[cfg(feature = "live")]
fn snapshot() {
    READERS.with(|readers| {
        let readers = readers.borrow();
        KEPT.with(|kept| {
            let mut kept = kept.borrow_mut();
            for (name, read) in readers.iter() {
                kept.insert(name, read());
            }
        });
    });
}
