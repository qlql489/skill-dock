//! Background updater — periodically checks github sources for new commits.
//!
//! 更新检查是**被动的**：只对远端做 `git ls-remote` 比较 SHA，绝不自动
//! pull。因为安装走的是直连软链，一旦 pull，上游删改的文件会立即传导到
//! 每个 agent —— 所以"应用更新"必须是用户在前端确认后的显式动作。

use crate::sources;
use crate::state::SourceKind;
use parking_lot::Mutex;
use std::sync::Arc;
use std::time::Duration;
use tauri::{AppHandle, Emitter, Manager};

const POLL_INTERVAL: Duration = Duration::from_secs(30 * 60); // 30 minutes

/// In-memory cache of "which sources have newer commits on the remote",
/// surfaced to the UI (概览待办页 / 来源页角标).
#[derive(Default, Debug, Clone, serde::Serialize)]
pub struct UpdateReport {
    pub generated_at: chrono::DateTime<chrono::Utc>,
    pub updates: Vec<UpdateEntry>,
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct UpdateEntry {
    pub source_id: String,
    pub source_name: String,
    pub source_kind: SourceKind,
    pub commit_changed: bool,
    pub new_commit_sha: Option<String>,
    pub previous_commit_sha: Option<String>,
}

static LAST_REPORT: once_cell::sync::Lazy<Mutex<UpdateReport>> =
    once_cell::sync::Lazy::new(|| Mutex::new(UpdateReport::default()));

pub fn take_last_report() -> UpdateReport {
    LAST_REPORT.lock().clone()
}

pub fn spawn_background(app: AppHandle) {
    std::thread::spawn(move || {
        // Startup check delayed a bit so launch stays snappy; the network
        // round-trips must never compete with the first paint.
        std::thread::sleep(Duration::from_secs(20));
        if let Err(e) = run_once(&app) {
            log::error!("update check failed: {e:#}");
        }
        loop {
            std::thread::sleep(POLL_INTERVAL);
            if let Err(e) = run_once(&app) {
                log::error!("update check failed: {e:#}");
            }
        }
    });
}

fn run_once(app: &AppHandle) -> anyhow::Result<()> {
    let state: tauri::State<Arc<crate::state::AppState>> = app.state();
    let report = check_now(&state)?;
    if !report.updates.is_empty() {
        let _ = app.emit("skill-dock://updates", &report);
    }
    Ok(())
}

/// Build the report and refresh the in-memory cache. Shared by the background
/// poller and the on-demand `check_updates` command — both must update
/// LAST_REPORT, otherwise the badges served via get_state stay stale until
/// the next 30-minute poll.
pub fn check_now(state: &crate::state::AppState) -> anyhow::Result<UpdateReport> {
    let report = build_report(state)?;
    *LAST_REPORT.lock() = report.clone();
    Ok(report)
}

/// Compare every github source's recorded sha with the remote head. Read-only:
/// no pull, no rescan, no state mutation — safe at any time.
pub fn build_report(state: &crate::state::AppState) -> anyhow::Result<UpdateReport> {
    let mut report = UpdateReport {
        generated_at: chrono::Utc::now(),
        updates: Vec::new(),
    };

    let snap = {
        let guard = state.inner.read();
        guard.clone()
    };

    for src in &snap.sources {
        if src.kind != SourceKind::Github || src.clone_status != crate::state::CloneStatus::Ready {
            continue;
        }
        match sources::ls_remote_head(src) {
            Ok(Some(remote)) => {
                if src.last_commit_sha.as_deref() != Some(remote.as_str()) {
                    report.updates.push(UpdateEntry {
                        source_id: src.id.to_string(),
                        source_name: src.name.clone(),
                        source_kind: src.kind.clone(),
                        commit_changed: true,
                        new_commit_sha: Some(remote),
                        previous_commit_sha: src.last_commit_sha.clone(),
                    });
                }
            }
            Ok(None) => { /* unresolvable ref — skip quietly */ }
            Err(e) => log::debug!("ls-remote {} failed: {e:#}", src.name),
        }
    }
    Ok(report)
}
