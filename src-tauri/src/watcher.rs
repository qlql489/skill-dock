//! Filesystem watcher — monitors local source folders for changes.
//!
//! EVERY local source is watched with `notify`. After a debounce window, the
//! watcher triggers a rescan of that source via [`commands::run_scan_blocking`],
//! so file changes are picked up automatically. Whether the rescan also
//! (re)installs symlinks into agents is decided by the source's
//! `auto_sync_targets` — the scan's phase 5 handles that; watching itself is
//! unconditional.
//!
//! Watchers are held alive in a global `Mutex<HashMap>` keyed by source id —
//! dropping the debouncer would stop watching, so ownership must outlive the
//! callback. [`unwatch`] removes a watcher when its source is deleted.

use crate::commands::run_scan_blocking;
use crate::state::{AppState, SourceKind};
use notify::{RecommendedWatcher, Watcher};
use notify_debouncer_full::{new_debouncer, DebouncedEvent, Debouncer, FileIdMap};
use parking_lot::Mutex;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;
use tauri::{AppHandle, Emitter, Manager};
use uuid::Uuid;

/// Debounce window. Editors that save by atomic rename fire several events in
/// quick succession; 2s collapses them into one rescan.
const DEBOUNCE: Duration = Duration::from_secs(2);

/// Global registry of live watchers, keyed by source id. Holding the
/// `Debouncer` keeps its underlying `RecommendedWatcher` alive; dropping it
/// stops watching.
static WATCHERS: once_cell::sync::Lazy<Mutex<HashMap<Uuid, Debouncer<RecommendedWatcher, FileIdMap>>>> =
    once_cell::sync::Lazy::new(|| Mutex::new(HashMap::new()));

/// At startup, (re)start watchers for EVERY local source. Watching is decoupled
/// from auto-sync: a watched folder always gets rescanned on change (so new
/// skills appear in the library), while `auto_sync_targets` only controls
/// whether the rescan also (re)installs symlinks into agents.
pub fn init_all(app: &AppHandle, state: &AppState) {
    let snap = state.snapshot();
    let mut count = 0;
    for src in &snap.sources {
        if src.kind == SourceKind::Local {
            let path = PathBuf::from(&src.location);
            watch(app.clone(), src.id, path);
            count += 1;
        }
    }
    if count > 0 {
        log::info!("watcher: started watching {count} local source(s)");
    }
}

/// Start (or replace) a watcher for a single local source. Safe to call
/// repeatedly; an existing watcher for the same id is dropped first.
pub fn watch(app: AppHandle, id: Uuid, path: PathBuf) {
    // Validate the path exists before subscribing — notify will error on a
    // missing dir, and we'd rather log + no-op than leak a broken watcher.
    if !path.is_dir() {
        log::warn!("watcher: skip source={id}: not a directory: {}", path.display());
        return;
    }

    let app_for_cb = app.clone();
    let id_for_cb = id;
    let callback = move |res: Result<Vec<DebouncedEvent>, Vec<notify::Error>>| {
        if res.is_err() {
            return;
        }
        log::debug!("watcher: change detected for source={id_for_cb}, triggering rescan");
        // Run the scan + auto-sync on a dedicated thread so the watcher
        // callback (which runs on notify's dispatch thread) never blocks on
        // filesystem IO. `run_scan_blocking` already performs the four-phase
        // low-lock scan AND the auto-sync install when targets are set.
        let app2 = app_for_cb.clone();
        std::thread::spawn(move || {
            let state: tauri::State<'_, Arc<AppState>> = app2.state();
            run_scan_blocking(&app2, state.inner(), id_for_cb, "watcher".to_string());
            let _ = app2.emit("skill-dock://source-scanned", id_for_cb.to_string());
        });
    };

    let mut debouncer = match new_debouncer(DEBOUNCE, None, callback) {
        Ok(d) => d,
        Err(e) => {
            log::error!("watcher: failed to create debouncer for source={id}: {e}");
            return;
        }
    };

    if let Err(e) = debouncer.watcher().watch(&path, notify::RecursiveMode::Recursive) {
        log::error!(
            "watcher: failed to watch {} for source={id}: {e}",
            path.display()
        );
        return;
    }

    // Replace any prior watcher for this id (e.g. path changed). Dropping the
    // old debouncer stops its watch thread — intended.
    let mut map = WATCHERS.lock();
    map.insert(id, debouncer);
    log::info!("watcher: now watching source={id} at {}", path.display());
}

/// Stop and drop the watcher for a source (e.g. source deleted). No-op if
/// there was no watcher for this id.
pub fn unwatch(id: Uuid) {
    let removed = WATCHERS.lock().remove(&id);
    if removed.is_some() {
        log::info!("watcher: stopped watching source={id}");
    }
}
