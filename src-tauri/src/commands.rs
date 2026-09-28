//! Tauri command handlers — all the IPC entry points the frontend calls.

use crate::scanner;
use crate::sources;
use crate::state::{AgentTarget, AppState, CloneStatus, Installation, Skill, SkillGroup, Source, SourceKind, SourceMode};
use crate::symlinks::{self, ConflictPolicy};
use crate::targets;
use crate::updater;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tauri::{AppHandle, Emitter, State};
use uuid::Uuid;

#[derive(Debug, Serialize)]
pub struct StateSnapshot {
    pub sources: Vec<Source>,
    pub targets: Vec<AgentTarget>,
    pub installations: Vec<Installation>,
    pub skills: Vec<Skill>,
    pub groups: Vec<SkillGroup>,
    pub projects: Vec<crate::state::Project>,
    pub data_dir: String,
    pub state_path: String,
    pub update_report: updater::UpdateReport,
}

#[tauri::command]
pub fn get_state(state: State<'_, Arc<AppState>>) -> Result<StateSnapshot, String> {
    let snap = state.snapshot();
    let skills: Vec<Skill> = snap.skills.values().cloned().collect();
    Ok(StateSnapshot {
        sources: snap.sources,
        targets: snap.targets,
        installations: snap.installations,
        skills,
        groups: snap.groups,
        projects: snap.projects,
        data_dir: state.data_dir.display().to_string(),
        state_path: state.state_path.display().to_string(),
        update_report: updater::take_last_report(),
    })
}

#[tauri::command]
pub fn get_state_path(state: State<'_, Arc<AppState>>) -> Result<String, String> {
    Ok(state.state_path.display().to_string())
}

#[tauri::command]
pub async fn add_local_source(
    name: String,
    location: String,
    mode: Option<String>,
    auto_sync_targets: Option<Vec<String>>,
    copy_to_vault: Option<bool>,
    state: State<'_, Arc<AppState>>,
    app: AppHandle,
) -> Result<Source, String> {
    let mode = parse_mode(mode);
    let auto_sync = auto_sync_targets.unwrap_or_default();
    log::info!(
        "add_local_source: name={name:?} location={location:?} mode={mode:?} auto_sync_targets={} copy_to_vault={}",
        auto_sync.len(),
        copy_to_vault.unwrap_or(false)
    );

    // 拷贝模式：内容复制进中央仓库（设置的数据目录），来源注册的是仓库本身，
    // 原路径不进来源列表。复制是慢 IO，放 blocking 线程池。
    if copy_to_vault.unwrap_or(false) {
        let real = sources::expand_home(location.trim());
        let dir = std::path::PathBuf::from(&real);
        if !dir.is_dir() {
            return Err(format!("目录不存在或不是文件夹：{}", dir.display()));
        }
        let state_arc = state.inner().clone();
        return tauri::async_runtime::spawn_blocking(move || {
            crate::discover::install_dir_into_vault(&state_arc, &app, &dir, "")
        })
        .await
        .map_err(|e| format!("后台拷贝任务失败: {e}"))?;
    }

    let src = {
        let mut guard = state.inner.write();
        let src = sources::add_local(&mut guard, name, location, mode, auto_sync.clone()).map_err(|e| {
            log::error!("add_local_source failed: {e:#}");
            e.to_string()
        })?;
        src
    }; // release the write lock before save — save() acquires a read lock
    state.save().map_err(|e| {
        log::error!("add_local_source: save failed: {e:#}");
        e.to_string()
    })?;
    log::info!("add_local_source: registered {} — scheduling background scan", src.id);
    // Always watch local folders — file changes auto-rescan into the library.
    // (Whether a rescan also auto-installs symlinks is decided by
    // auto_sync_targets inside the scan's phase 5.)
    crate::watcher::watch(app.clone(), src.id, std::path::PathBuf::from(&src.location));
    spawn_scan(state.inner().clone(), app.clone(), src.id, "local".to_string());
    Ok(src)
}

#[tauri::command]
pub fn add_github_source(
    name: String,
    url: String,
    branch: Option<String>,
    mode: Option<String>,
    state: State<'_, Arc<AppState>>,
    app: AppHandle,
) -> Result<Source, String> {
    let mode = parse_mode(mode);
    log::info!("add_github_source: name={name:?} url={url:?} branch={branch:?} mode={mode:?}");
    let src = {
        let mut guard = state.inner.write();
        sources::add_github(&mut guard, name, url, branch, mode).map_err(|e| {
            log::error!("add_github_source failed: {e:#}");
            e.to_string()
        })?
    }; // release the write lock before save
    state.save().map_err(|e| {
        log::error!("add_github_source: save failed: {e:#}");
        e.to_string()
    })?;
    log::info!("add_github_source: registered {} — scheduling background clone", src.id);
    spawn_clone(state.inner().clone(), app.clone(), src.id);
    Ok(src)
}

/// Parse the frontend's mode string ("grouped" | "flat" | null) into the
/// enum. Unknown / null defaults to Flat (legacy behavior).
fn parse_mode(mode: Option<String>) -> SourceMode {
    match mode.as_deref() {
        Some("grouped") => SourceMode::Grouped,
        _ => SourceMode::Flat,
    }
}

/// Retry the (failed) clone of an existing GitHub source. Resets the source
/// to `Cloning` and kicks off a fresh background clone. Returns immediately.
#[tauri::command]
pub fn retry_clone_source(
    id: String,
    state: State<'_, Arc<AppState>>,
    app: AppHandle,
) -> Result<(), String> {
    let id = Uuid::parse_str(&id).map_err(|e| e.to_string())?;
    {
        let mut guard = state.inner.write();
        let src = guard
            .sources
            .iter_mut()
            .find(|s| s.id == id)
            .ok_or_else(|| format!("source not found: {id}"))?;
        if src.kind != SourceKind::Github {
            return Err("only github sources can be re-cloned".to_string());
        }
        if src.clone_status == CloneStatus::Cloning {
            return Err("clone already in progress".to_string());
        }
        src.clone_status = CloneStatus::Cloning;
        src.clone_error = None;
    }
    state.save().map_err(|e| e.to_string())?;
    log::info!("retry_clone_source: re-cloning source={id}");
    spawn_clone(state.inner().clone(), app.clone(), id);
    Ok(())
}

#[tauri::command]
pub fn rescan_source(
    id: String,
    state: State<'_, Arc<AppState>>,
    app: AppHandle,
) -> Result<Vec<Skill>, String> {
    let id = Uuid::parse_str(&id).map_err(|e| e.to_string())?;
    spawn_scan(state.inner().clone(), app.clone(), id, "manual".to_string());
    Ok(Vec::new())
}

#[tauri::command]
pub fn rescan_all(state: State<'_, Arc<AppState>>, app: AppHandle) -> Result<usize, String> {
    let ids: Vec<Uuid> = {
        let guard = state.inner.read();
        guard.sources.iter().map(|s| s.id).collect()
    };
    log::info!("rescan_all: scanning {} sources", ids.len());
    let app_for_thread = app.clone();
    let state_for_thread = state.inner().clone();
    std::thread::spawn(move || {
        for id in ids {
            run_scan_blocking(&app_for_thread, &state_for_thread, id, "all".to_string());
        }
        let _ = app_for_thread.emit("skill-dock://rescan-all-done", ());
    });
    Ok(0)
}

#[tauri::command]
pub fn scan_all(state: State<'_, Arc<AppState>>, app: AppHandle) -> Result<ScanAllResult, String> {
    let ids: Vec<Uuid> = {
        let guard = state.inner.read();
        guard.sources.iter().map(|s| s.id).collect()
    };
    log::info!("scan_all: kicking off background scan for {} sources", ids.len());
    let app_for_thread = app.clone();
    let state_for_thread = state.inner().clone();
    std::thread::spawn(move || {
        for id in ids {
            run_scan_blocking(&app_for_thread, &state_for_thread, id, "all".to_string());
        }
        let _ = app_for_thread.emit("skill-dock://scan-all-done", ());
    });
    Ok(ScanAllResult { total_skills: 0, per_source: Vec::new() })
}

/// Spawn a background thread that scans a single source, saves the result,
/// auto-syncs if configured, and emits `skill-dock://source-scanned` on
/// completion.
fn spawn_scan(state: Arc<AppState>, app: AppHandle, id: Uuid, trigger: String) {
    std::thread::spawn(move || {
        run_scan_blocking(&app, &state, id, trigger);
        let _ = app.emit("skill-dock://source-scanned", id.to_string());
    });
}

/// Spawn a background thread that runs the (slow, network-bound) git clone
/// for a source, then on success proceeds to a normal scan. On failure the
/// source is marked `Failed` with the error surfaced to the UI. The clone
/// runs lock-free; locks are taken only briefly to read the source, to write
/// the outcome, and (on success) to apply scan results — mirroring the
/// four-phase discipline of [`run_scan_blocking`].
fn spawn_clone(state: Arc<AppState>, app: AppHandle, id: Uuid) {
    std::thread::spawn(move || {
        match run_clone_blocking(&state, id) {
            Ok(()) => {
                // Clone succeeded — scan the freshly cloned tree, then emit.
                run_scan_blocking(&app, &state, id, "post-clone".to_string());
                let _ = app.emit("skill-dock://source-scanned", id.to_string());
            }
            Err(e) => {
                log::error!("clone thread: source={id} failed: {e:#}");
                let _ = app.emit(
                    "skill-dock://source-clone-failed",
                    CloneFailedEvent {
                        source_id: id.to_string(),
                        error: e.to_string(),
                    },
                );
            }
        }
    });
}

/// Outcome carrier for the `source-clone-failed` event.
#[derive(Debug, Serialize, Clone)]
struct CloneFailedEvent {
    source_id: String,
    error: String,
}

/// Clone a source on the background thread. Returns Ok(()) on success with
/// the source already advanced to `Ready` (branch + commit recorded). On
/// Err the source is advanced to `Failed` and the error message stored. In
/// both cases the result is persisted. Returns the error so the caller can
/// emit the failed event.
fn run_clone_blocking(state: &AppState, id: Uuid) -> Result<(), anyhow::Error> {
    log::info!("clone thread: start source={id}");
    let started = std::time::Instant::now();

    // ① short read lock — clone the source record out from under the lock.
    let src = {
        let guard = state.inner.read();
        guard
            .sources
            .iter()
            .find(|s| s.id == id)
            .cloned()
            .ok_or_else(|| anyhow::anyhow!("source not found: {id}"))?
    };

    // ② lock-free — the actual git clone (the slow part). Has a hard timeout
    // inside sources::perform_clone.
    let outcome = sources::perform_clone(&src);

    match outcome {
        Ok(o) => {
            // ③ short write lock — record success.
            {
                let mut guard = state.inner.write();
                if let Some(s) = guard.sources.iter_mut().find(|s| s.id == id) {
                    s.clone_status = CloneStatus::Ready;
                    s.clone_error = None;
                    s.branch = Some(o.branch);
                    s.last_commit_sha = Some(o.commit_sha);
                }
            }
            // ④ lock-free — persist.
            if let Err(e) = state.save() {
                log::error!("clone thread: save failed for source={id}: {e:#}");
            }
            log::info!(
                "clone thread: source={id} cloned ok in {:?}",
                started.elapsed()
            );
            Ok(())
        }
        Err(e) => {
            let msg = format!("{e:#}");
            // ③ short write lock — record failure.
            {
                let mut guard = state.inner.write();
                if let Some(s) = guard.sources.iter_mut().find(|s| s.id == id) {
                    s.clone_status = CloneStatus::Failed;
                    s.clone_error = Some(msg.clone());
                }
            }
            if let Err(se) = state.save() {
                log::error!("clone thread: save failed for source={id}: {se:#}");
            }
            Err(e)
        }
    }
}

/// Scan a single source, persist the result, and — if the source has
/// `auto_sync_targets` — symlink its skills into those targets. Shared by the
/// add-local, manual-rescan, rescan-all, and watcher paths so that auto-sync
/// behavior is consistent everywhere.
///
/// Four-phase, lock-free discipline (see `AppState::save` — never save under
/// a write lock):
///   ① short read lock  — fetch the source + resolve its root path
///   ② lock-free         — the actual filesystem scan (the slow part)
///   ③ short write lock — commit results back into state
///   ④ lock-free         — persist to disk
///   ⑤ auto-sync         — install into recorded targets (short write lock)
pub fn run_scan_blocking(app: &AppHandle, state: &AppState, id: Uuid, trigger: String) {
    log::info!("scan thread: start source={id} trigger={trigger}");
    let started = std::time::Instant::now();

    let (src, root, vault) = {
        let guard = state.inner.read();
        match sources::read_scan_root(&guard, id) {
            Ok((src, root)) => {
                // 扫描的来源目录恰为中央仓库时回退到 root 本身，is_shared 仍成立。
                let vault = crate::discover::effective_vault_dir(&guard.ui.vault_path)
                    .unwrap_or_else(|| root.clone());
                (src, root, vault)
            }
            Err(e) => {
                log::error!("scan thread: source={id} read root failed: {e:#}");
                return;
            }
        }
    };

    let scanned = match scanner::scan(&src, &root, &vault) {
        Ok(s) => s,
        Err(e) => {
            log::error!("scan thread: source={id} scan failed: {e:#}");
            return;
        }
    };

    let committed = {
        let mut guard = state.inner.write();
        let committed = match sources::apply_scan_results(&mut guard, id, scanned) {
            Ok(s) => s,
            Err(e) => {
                log::error!("scan thread: source={id} apply failed: {e:#}");
                return;
            }
        };
        // Fresh skills are in — reconcile now also adopts externally-created
        // links pointing at them, so a source whose skills were hand-linked
        // into agents shows as installed immediately after 添加/重扫.
        if let Err(e) = symlinks::reconcile(&mut guard) {
            log::warn!("scan thread: source={id} post-scan reconcile failed: {e:#}");
        }
        committed
    };

    log::info!(
        "scan thread: source={id} ok — {} skills in {:?}",
        committed.len(),
        started.elapsed()
    );

    if let Err(e) = state.save() {
        log::error!("scan thread: save failed for source={id}: {e:#}");
    }

    // Phase 5: auto-sync. Read targets + skill list under a short read lock,
    // then install under a short write lock. `save()` below would deadlock
    // under a live write guard, so the guard is dropped first.
    let auto_targets = {
        let guard = state.inner.read();
        guard
            .sources
            .iter()
            .find(|s| s.id == id)
            .map(|s| s.auto_sync_targets.clone())
            .unwrap_or_default()
    };
    if auto_targets.is_empty() {
        return;
    }

    let skills: Vec<Skill> = {
        let guard = state.inner.read();
        guard
            .skills
            .values()
            .filter(|s| s.source_id == id)
            .cloned()
            .collect()
    };

    let mut installed = 0usize;
    let mut failed: Vec<String> = Vec::new();
    {
        let mut guard = state.inner.write();
        for tid in &auto_targets {
            for skill in &skills {
                match symlinks::install(&mut guard, skill, tid, ConflictPolicy::Fail) {
                    Ok(_) => installed += 1,
                    Err(e) => {
                        log::warn!(
                            "scan auto-sync: source={id} skill={} target={tid}: {e:#}",
                            skill.name
                        );
                        failed.push(format!("{}: {e}", skill.name));
                    }
                }
            }
        }
    } // release write lock before save

    if let Err(e) = state.save() {
        log::error!("scan auto-sync: save failed for source={id}: {e:#}");
    }

    log::info!(
        "scan auto-sync: source={id} — {installed} installed, {} failed",
        failed.len()
    );

    let _ = app.emit(
        "skill-dock://auto-synced",
        serde_json::json!({
            "source_id": id.to_string(),
            "installed": installed,
            "failed": failed,
        }),
    );
}

#[tauri::command]
pub fn remove_source(
    id: String,
    state: State<'_, Arc<AppState>>,
) -> Result<(), String> {
    let id = Uuid::parse_str(&id).map_err(|e| e.to_string())?;
    {
        let mut guard = state.inner.write();
        sources::remove_source(&mut guard, id).map_err(|e| e.to_string())?;
    } // release write lock before save
    // Drop any filesystem watcher for this source so we don't rescan a
    // deleted folder.
    crate::watcher::unwatch(id);
    state.save().map_err(|e| e.to_string())?;
    Ok(())
}

#[derive(Debug, Serialize)]
pub struct ScanAllResult {
    pub total_skills: usize,
    pub per_source: Vec<(String, usize)>,
}

#[derive(Debug, Deserialize)]
pub struct ReadSkillRequest {
    pub skill_id: String,
}

#[derive(Debug, Serialize)]
pub struct SkillContent {
    pub skill_id: String,
    pub exists: bool,
    pub content: String,
    pub path: String,
}

#[tauri::command]
pub fn read_skill_md(req: ReadSkillRequest, state: State<'_, Arc<AppState>>) -> Result<SkillContent, String> {
    let snap = state.snapshot();
    let skill = snap
        .skills
        .get(&req.skill_id)
        .ok_or_else(|| format!("skill not found: {}", req.skill_id))?;
    let path = skill.absolute_path.join("SKILL.md");
    let exists = path.exists();
    let content = if exists {
        std::fs::read_to_string(&path).map_err(|e| e.to_string())?
    } else {
        String::new()
    };
    Ok(SkillContent {
        skill_id: skill.id.clone(),
        exists,
        content,
        path: path.display().to_string(),
    })
}

#[derive(Debug, Deserialize)]
pub struct WriteSkillRequest {
    pub skill_id: String,
    pub content: String,
}

#[tauri::command]
pub fn write_skill_md(req: WriteSkillRequest, state: State<'_, Arc<AppState>>) -> Result<(), String> {
    {
        let mut guard = state.inner.write();
        let skill = guard
            .skills
            .get(&req.skill_id)
            .cloned()
            .ok_or_else(|| format!("skill not found: {}", req.skill_id))?;
        let path = skill.absolute_path.join("SKILL.md");
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
        }
        std::fs::write(&path, &req.content).map_err(|e| e.to_string())?;
        // Refresh hash + mtime in cache.
        if let Ok(bytes) = std::fs::read(&path) {
            if let Some(s) = guard.skills.get_mut(&req.skill_id) {
                s.content_hash = blake3::hash(&bytes).to_hex().to_string();
                s.modified_at = chrono::Utc::now();
            }
        }
    } // release the write lock before save
    state.save().map_err(|e| e.to_string())?;
    Ok(())
}

#[tauri::command]
pub fn list_targets(state: State<'_, Arc<AppState>>) -> Result<Vec<AgentTarget>, String> {
    Ok(state.snapshot().targets)
}

/// Classification for a directory discovered under an agent target.
///
/// `Managed` is intentionally stricter than merely finding an installation
/// record: the on-disk entry must still be a symlink and its resolved target
/// must match the source path recorded for that installation. If a user
/// repoints a managed symlink, it is surfaced as `ExternalSymlink` instead of
/// silently claiming ownership of it.
#[derive(Debug, Clone, Copy, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ExistingSkillKind {
    Local,
    Managed,
    ExternalSymlink,
}

/// One entry in a target's skills_dir — what's actually on disk, regardless
/// of whether SkillDock installed it. This is what the agents page shows
/// when an agent row is expanded.
#[derive(Debug, Clone, Serialize)]
pub struct TargetSkillEntry {
    pub name: String,
    /// Absolute path of this entry under the target's skills directory.
    pub path: String,
    /// Target metadata is repeated on each item so the all-target discovery
    /// command can be consumed without joining against the state snapshot.
    pub target_id: String,
    pub target_name: String,
    pub target_skills_dir: String,
    /// True if this entry is a symlink (as opposed to a real folder).
    pub is_symlink: bool,
    /// When `is_symlink` is true, the resolved destination address. For a
    /// dangling link this falls back to the absolute path implied by the raw
    /// link, so callers can still diagnose it.
    pub symlink_target: Option<String>,
    /// Whether the entry contains a SKILL.md (following symlinks).
    pub has_skill_md: bool,
    /// Whether this is a normal local directory, a symlink installed by this
    /// app, or an unmanaged/external symlink.
    pub kind: ExistingSkillKind,
}

/// Discover every directory/symlink currently present in every configured
/// target. This is deliberately read-only: it snapshots in-memory state and
/// only reads target directories; it does not reconcile or mutate records.
#[tauri::command]
pub fn discover_existing_skills(
    state: State<'_, Arc<AppState>>,
) -> Result<Vec<TargetSkillEntry>, String> {
    let snapshot = state.snapshot();
    Ok(discover_target_skills(
        &snapshot.targets,
        &snapshot.installations,
        &snapshot.skills,
    ))
}

/// Scan a target's skills_dir on disk and return every subdirectory entry,
/// flagging symlinks and their destinations. Unlike the library's skill list
/// (which comes from source scans), this shows the raw filesystem reality of
/// the agent's skills folder — including manually-created symlinks that
/// SkillDock didn't install.
#[tauri::command]
pub fn list_target_skills(
    target_id: String,
    state: State<'_, Arc<AppState>>,
) -> Result<Vec<TargetSkillEntry>, String> {
    let snapshot = state.snapshot();
    let target = snapshot
        .targets
        .iter()
        .find(|t| t.id == target_id)
        .ok_or_else(|| format!("target not found: {target_id}"))?;

    Ok(discover_target_skills(
        std::slice::from_ref(target),
        &snapshot.installations,
        &snapshot.skills,
    ))
}

/// Read one level of each target's skills directory and attach ownership
/// information from the current state. A target may be disabled or undetected
/// and is still scanned: this command reports the filesystem, not the current
/// install policy.
fn discover_target_skills(
    targets: &[AgentTarget],
    installations: &[Installation],
    skills: &BTreeMap<String, Skill>,
) -> Vec<TargetSkillEntry> {
    let mut result = Vec::new();

    for target in targets {
        // 默认目录 + 额外地址一起扫；条目的 target_skills_dir 记录实际所在目录。
        for target_dir in crate::targets::all_dirs(target) {
        let entries = match std::fs::read_dir(&target_dir) {
            Ok(entries) => entries,
            Err(e) => {
                // Missing/unreadable target directories are normal for
                // undetected agents; leave them out rather than failing all
                // other targets.
                log::debug!(
                    "discover_existing_skills: cannot read target {} at {}: {}",
                    target.id,
                    target_dir.display(),
                    e
                );
                continue;
            }
        };

        for entry in entries.flatten() {
            let name = entry.file_name().to_string_lossy().to_string();
            // Skip dotfiles (.DS_Store, .git, etc.).
            if name.starts_with('.') {
                continue;
            }

            let path = entry.path();
            // symlink_metadata does NOT follow the link — it lets us preserve
            // the distinction between a local directory and a symlink.
            let sym_meta = match std::fs::symlink_metadata(&path) {
                Ok(meta) => meta,
                Err(_) => continue,
            };
            let file_type = sym_meta.file_type();
            let is_symlink = file_type.is_symlink();
            let is_dir = file_type.is_dir();
            if !is_symlink && !is_dir {
                continue;
            }

            let resolved_target = if is_symlink {
                resolve_symlink_target(&path)
            } else {
                None
            };
            let symlink_target = resolved_target
                .as_ref()
                .map(|target| target.to_string_lossy().into_owned());
            // `is_file` follows a directory symlink, so this also tells the
            // caller whether a discovered entry looks like a real skill.
            let has_skill_md = path.join("SKILL.md").is_file();
            let kind = classify_existing_skill(
                &path,
                &target.id,
                is_symlink,
                resolved_target.as_deref(),
                installations,
                skills,
            );

            result.push(TargetSkillEntry {
                name,
                path: path.to_string_lossy().into_owned(),
                target_id: target.id.clone(),
                target_name: target.name.clone(),
                target_skills_dir: target_dir.to_string_lossy().into_owned(),
                is_symlink,
                symlink_target,
                has_skill_md,
                kind,
            });
        }
        }
    }

    result.sort_by(|a, b| {
        a.target_name
            .to_lowercase()
            .cmp(&b.target_name.to_lowercase())
            .then_with(|| a.path.to_lowercase().cmp(&b.path.to_lowercase()))
    });
    result
}

fn classify_existing_skill(
    entry_path: &Path,
    target_id: &str,
    is_symlink: bool,
    resolved_target: Option<&Path>,
    installations: &[Installation],
    skills: &BTreeMap<String, Skill>,
) -> ExistingSkillKind {
    if !is_symlink {
        return ExistingSkillKind::Local;
    }

    let Some(resolved_target) = resolved_target else {
        return ExistingSkillKind::ExternalSymlink;
    };
    let Some(installation) = installations.iter().find(|installation| {
        installation.target_id == target_id
            && same_entry_path(&installation.link_path, entry_path)
    }) else {
        return ExistingSkillKind::ExternalSymlink;
    };
    let Some(skill) = skills.get(&installation.skill_id) else {
        return ExistingSkillKind::ExternalSymlink;
    };

    if same_target_path(resolved_target, &skill.absolute_path) {
        ExistingSkillKind::Managed
    } else {
        // The record exists, but the link was repointed outside the source
        // recorded by the app. Treat it as external so discovery never grants
        // ownership based on stale metadata.
        ExistingSkillKind::ExternalSymlink
    }
}

/// Resolve a symlink to an absolute address without following the final link
/// path for metadata purposes. Dangling links still return the path implied by
/// the link, which is useful to callers diagnosing external entries.
fn resolve_symlink_target(path: &Path) -> Option<PathBuf> {
    let raw_target = std::fs::read_link(path).ok()?;
    let target = if raw_target.is_absolute() {
        raw_target
    } else {
        path.parent()?.join(raw_target)
    };
    Some(std::fs::canonicalize(&target).unwrap_or(target))
}

/// Compare two directory-entry paths without canonicalizing the final entry.
/// Canonicalizing the final symlink would make two different links that point
/// to the same directory appear to be the same installation record.
fn same_entry_path(left: &Path, right: &Path) -> bool {
    if left == right {
        return true;
    }
    let (Some(left_name), Some(right_name)) = (left.file_name(), right.file_name()) else {
        return false;
    };
    if left_name != right_name {
        return false;
    }

    let left_parent = left.parent().unwrap_or_else(|| Path::new(""));
    let right_parent = right.parent().unwrap_or_else(|| Path::new(""));
    canonical_or_original(left_parent) == canonical_or_original(right_parent)
}

fn same_target_path(left: &Path, right: &Path) -> bool {
    canonical_or_original(left) == canonical_or_original(right)
}

fn canonical_or_original(path: &Path) -> PathBuf {
    std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf())
}

/// A node in a recursive file tree, used by the agent-skills file browser.
#[derive(Debug, Clone, Serialize)]
pub struct FileTreeNode {
    pub name: String,
    /// Path relative to the tree root (forward-slash separated).
    pub path: String,
    pub is_dir: bool,
    pub children: Vec<FileTreeNode>,
}

/// Recursively scan a directory and return its tree. Follows symlinks (so a
/// symlinked skill dir shows its real contents). Skips dotfiles. Depth-limited
/// to prevent runaway scans.
#[tauri::command]
pub fn read_skill_tree(root_path: String) -> Result<Vec<FileTreeNode>, String> {
    let root = std::path::PathBuf::from(&root_path);
    if !root.is_dir() {
        return Err(format!("not a directory: {}", root_path));
    }
    scan_tree_dir(&root, &root, 0).map_err(|e| e.to_string())
}

/// Max recursion depth for the file tree. 5 is enough for typical skill
/// layouts (SKILL.md + scripts/ + resources/ etc.) without scanning huge trees.
const TREE_MAX_DEPTH: usize = 5;
/// Skip these directory names (build/cache dirs).
const TREE_SKIP: &[&str] = &[
    ".git", ".hg", ".svn", "node_modules", "target", "dist", "build",
    "__pycache__", ".venv", "venv", ".idea", ".vscode",
];

fn scan_tree_dir(root: &std::path::Path, dir: &std::path::Path, depth: usize) -> anyhow::Result<Vec<FileTreeNode>> {
    let mut nodes = Vec::new();
    let entries = std::fs::read_dir(dir)?;
    // Collect + sort: dirs first, then files, alphabetically.
    let mut entries: Vec<_> = entries.flatten().collect();
    entries.sort_by(|a, b| {
        let a_dir = a.file_type().map(|t| t.is_dir()).unwrap_or(false);
        let b_dir = b.file_type().map(|t| t.is_dir()).unwrap_or(false);
        match (a_dir, b_dir) {
            (true, false) => std::cmp::Ordering::Less,
            (false, true) => std::cmp::Ordering::Greater,
            _ => a.file_name().cmp(&b.file_name()),
        }
    });

    for entry in entries {
        let name = entry.file_name().to_string_lossy().to_string();
        if name.starts_with('.') {
            continue;
        }
        if TREE_SKIP.iter().any(|s| *s == name.as_str()) {
            continue;
        }
        let entry_path = entry.path();
        // Use metadata (follows symlinks) to determine if it's a directory.
        let is_dir = std::fs::metadata(&entry_path).map(|m| m.is_dir()).unwrap_or(false);
        let relative = entry_path.strip_prefix(root).unwrap_or(&entry_path).to_string_lossy().replace('\\', "/");

        let children = if is_dir && depth < TREE_MAX_DEPTH {
            scan_tree_dir(root, &entry_path, depth + 1).unwrap_or_default()
        } else {
            Vec::new()
        };

        nodes.push(FileTreeNode {
            name,
            path: relative,
            is_dir,
            children,
        });
    }
    Ok(nodes)
}

/// Read a file's content. `root_path` is the skill directory; `file_path` is
/// relative to it. Security-checked to ensure the file stays under root.
#[tauri::command]
pub fn read_skill_file(root_path: String, file_path: String) -> Result<String, String> {
    let root = std::path::PathBuf::from(&root_path);
    let canonical_root = root.canonicalize().map_err(|e| format!("invalid root: {e}"))?;
    let target = canonical_root.join(&file_path);
    let canonical_target = target.canonicalize().map_err(|e| format!("invalid path: {e}"))?;
    if !canonical_target.starts_with(&canonical_root) {
        return Err("access denied: path outside skill directory".to_string());
    }
    let meta = std::fs::metadata(&canonical_target).map_err(|e| e.to_string())?;
    if meta.is_dir() {
        return Err("cannot read a directory".to_string());
    }
    if meta.len() > 1024 * 1024 {
        return Err("file too large (max 1 MB)".to_string());
    }
    std::fs::read_to_string(&canonical_target).map_err(|e| e.to_string())
}

/// Re-probe all targets for whether their agent is installed, and auto-enable
/// any newly-detected ones the user hasn't manually toggled. Returns the
/// number of detected targets so the UI can toast it.
#[tauri::command]
pub fn redetect_targets(state: State<'_, Arc<AppState>>) -> Result<usize, String> {
    let detected;
    {
        let mut guard = state.inner.write();
        detected = crate::detect::detect_all(&mut guard.targets);
        crate::detect::auto_enable_untouched(&mut guard.targets);
    } // release write lock before save
    state.save().map_err(|e| e.to_string())?;
    log::info!("redetect_targets: {detected} targets detected");
    Ok(detected)
}

#[derive(Debug, Deserialize)]
pub struct AddTargetRequest {
    pub name: String,
    pub skills_dir: String,
    pub description: String,
    pub tag: Option<String>,
}

#[tauri::command]
pub fn add_target(req: AddTargetRequest, state: State<'_, Arc<AppState>>) -> Result<AgentTarget, String> {
    let target = {
        let mut guard = state.inner.write();
        targets::add(&mut guard, req.name, req.skills_dir, req.description, req.tag)
            .map_err(|e| e.to_string())?
    };
    state.save().map_err(|e| e.to_string())?;
    Ok(target)
}

#[derive(Debug, Deserialize)]
pub struct UpdateTargetRequest {
    pub id: String,
    pub enabled: Option<bool>,
    pub name: Option<String>,
    pub skills_dir: Option<String>,
    /// 整表替换额外地址（None = 不变）；编辑语义见 targets::update。
    pub extra_dirs: Option<Vec<String>>,
    pub description: Option<String>,
    pub tag: Option<Option<String>>,
}

#[tauri::command]
pub fn update_target(req: UpdateTargetRequest, state: State<'_, Arc<AppState>>) -> Result<targets::UpdateOutcome, String> {
    let outcome = {
        let mut guard = state.inner.write();
        targets::update(
            &mut guard,
            &req.id,
            req.enabled,
            req.name,
            req.skills_dir,
            req.extra_dirs,
            req.description,
            req.tag,
        )
        .map_err(|e| e.to_string())?
    };
    state.save().map_err(|e| e.to_string())?;
    Ok(outcome)
}

/// 内置 Agent 的默认目录一键重置（软链随迁移）。
#[tauri::command]
pub fn reset_target_dir(id: String, state: State<'_, Arc<AppState>>) -> Result<targets::UpdateOutcome, String> {
    let outcome = {
        let mut guard = state.inner.write();
        targets::reset_dir(&mut guard, &id).map_err(|e| e.to_string())?
    };
    state.save().map_err(|e| e.to_string())?;
    Ok(outcome)
}

/// 内置目录清单：编辑弹窗显示「默认应为 X」提示 + 漂移时提供重置按钮。
#[tauri::command]
pub fn list_builtin_dirs() -> Vec<targets::BuiltinDirInfo> {
    targets::list_builtin_dirs()
}

/// 添加/编辑 Agent 弹窗的路径实时识别：归一化 + 是否与其他 target 冲突 +
/// 是否命中内置 agent 目录（精确/按目录尾缀推测）+ 目录里已有多少技能。
#[derive(Debug, Deserialize)]
pub struct ClassifyPathRequest {
    pub path: String,
    /// 编辑已有目标时传自己的 id：不算自己占用自己的旧目录。
    pub exclude_id: Option<String>,
}

#[tauri::command]
pub fn classify_target_path(
    req: ClassifyPathRequest,
    state: State<'_, Arc<AppState>>,
) -> Result<targets::PathInsight, String> {
    let snap = state.snapshot();
    Ok(targets::classify_path(&snap, &req.path, req.exclude_id.as_deref()))
}

#[tauri::command]
pub fn remove_target(id: String, state: State<'_, Arc<AppState>>) -> Result<(), String> {
    {
        let mut guard = state.inner.write();
        targets::remove(&mut guard, &id).map_err(|e| e.to_string())?;
    } // release write lock before save
    state.save().map_err(|e| e.to_string())?;
    Ok(())
}

#[derive(Debug, Deserialize)]
pub struct InstallSkillRequest {
    pub skill_id: String,
    pub target_id: String,
    pub on_conflict: Option<String>,
}

#[tauri::command]
pub fn install_skill(
    req: InstallSkillRequest,
    state: State<'_, Arc<AppState>>,
) -> Result<Installation, String> {
    let inst = {
        let mut guard = state.inner.write();
        let skill = guard
            .skills
            .get(&req.skill_id)
            .cloned()
            .ok_or_else(|| format!("skill not found: {}", req.skill_id))?;
        let policy = match req.on_conflict.as_deref() {
            Some("replace") => ConflictPolicy::Replace,
            _ => ConflictPolicy::Fail,
        };
        symlinks::install(&mut guard, &skill, &req.target_id, policy)
            .map_err(|e| e.to_string())?
    };
    state.save().map_err(|e| e.to_string())?;
    Ok(inst)
}

#[derive(Debug, Deserialize)]
pub struct UninstallSkillRequest {
    pub skill_id: String,
    pub target_id: String,
}

#[tauri::command]
pub fn uninstall_skill(
    req: UninstallSkillRequest,
    state: State<'_, Arc<AppState>>,
) -> Result<(), String> {
    {
        let mut guard = state.inner.write();
        symlinks::uninstall(&mut guard, &req.skill_id, &req.target_id).map_err(|e| e.to_string())?;
    } // release write lock before save
    state.save().map_err(|e| e.to_string())?;
    Ok(())
}

// ---------------------------------------------------------------------------
// Group / skill management commands
// ---------------------------------------------------------------------------

/// "Delete" a single skill: uninstall it from every target, then add its id
/// to the hidden set so it won't reappear on rescan. The skill's files on
/// disk are left untouched (a git-cloned source would otherwise drift out of
/// sync with its remote). Works for both flat and grouped sources.
#[derive(Debug, Deserialize)]
pub struct DeleteSkillRequest {
    pub skill_id: String,
}

#[tauri::command]
pub fn delete_skill(
    req: DeleteSkillRequest,
    state: State<'_, Arc<AppState>>,
) -> Result<(), String> {
    {
        let mut guard = state.inner.write();
        // Uninstall from every target that currently has it.
        let target_ids: Vec<String> = guard
            .installations
            .iter()
            .filter(|i| i.skill_id == req.skill_id)
            .map(|i| i.target_id.clone())
            .collect();
        for tid in target_ids {
            // Best-effort: a missing link is not fatal.
            if let Err(e) = symlinks::uninstall(&mut guard, &req.skill_id, &tid) {
                log::warn!("delete_skill: uninstall from {tid} failed: {e:#}");
            }
        }
        // Hide it from future scans and drop it from the live cache.
        guard.hidden_skills.insert(req.skill_id.clone());
        guard.skills.remove(&req.skill_id);
        // Refresh the owning source's skill_count.
        if let Some(src_id) = req.skill_id.split(':').next() {
            if let Ok(uuid) = Uuid::parse_str(src_id) {
                let count = guard.skills.keys().filter(|k| k.starts_with(&format!("{uuid}:"))).count();
                if let Some(s) = guard.sources.iter_mut().find(|s| s.id == uuid) {
                    s.skill_count = count;
                }
            }
        }
    }
    state.save().map_err(|e| e.to_string())?;
    Ok(())
}

/// Install every skill belonging to a source into one target. Intended for
/// grouped sources, but works for any source. Not transactional: on partial
/// failure the successfully-installed skills remain and the failures are
/// reported back.
#[derive(Debug, Deserialize)]
pub struct GroupTargetRequest {
    pub source_id: String,
    pub target_id: String,
}

#[derive(Debug, Serialize)]
pub struct GroupInstallResult {
    pub installed: usize,
    pub failed: Vec<String>,
}

#[tauri::command]
pub fn install_source_group(
    req: GroupTargetRequest,
    state: State<'_, Arc<AppState>>,
) -> Result<GroupInstallResult, String> {
    let id = Uuid::parse_str(&req.source_id).map_err(|e| e.to_string())?;
    let mut installed = 0usize;
    let mut failed = Vec::new();
    {
        let mut guard = state.inner.write();
        // Collect the source's current skills (clone to avoid borrow issues).
        let skills: Vec<Skill> = guard
            .skills
            .values()
            .filter(|s| s.source_id == id)
            .cloned()
            .collect();
        for skill in &skills {
            match symlinks::install(&mut guard, skill, &req.target_id, ConflictPolicy::Fail) {
                Ok(_) => installed += 1,
                Err(e) => {
                    log::warn!("install_source_group: {} failed: {e:#}", skill.id);
                    failed.push(format!("{}: {e}", skill.name));
                }
            }
        }
    }
    state.save().map_err(|e| e.to_string())?;
    Ok(GroupInstallResult { installed, failed })
}

/// Uninstall every skill belonging to a source from one target. Does NOT
/// hide them — they reappear on the next install. (Use delete_skill / the
/// group delete for permanent removal.)
#[tauri::command]
pub fn uninstall_source_group(
    req: GroupTargetRequest,
    state: State<'_, Arc<AppState>>,
) -> Result<usize, String> {
    let id = Uuid::parse_str(&req.source_id).map_err(|e| e.to_string())?;
    let mut removed = 0usize;
    {
        let mut guard = state.inner.write();
        let skill_ids: Vec<String> = guard
            .installations
            .iter()
            .filter(|i| i.target_id == req.target_id && i.skill_id.starts_with(&format!("{id}:")))
            .map(|i| i.skill_id.clone())
            .collect();
        for sid in skill_ids {
            if symlinks::uninstall(&mut guard, &sid, &req.target_id).is_ok() {
                removed += 1;
            }
        }
    }
    state.save().map_err(|e| e.to_string())?;
    Ok(removed)
}

/// Update a whole group: for github sources pull the latest, rescan, then
/// re-install (Replace) every skill of that source that is currently
/// installed to any target. Runs on a background thread (network git fetch
/// must not hold the lock). Emits `source-scanned` on completion.
#[tauri::command]
pub fn update_source_group(
    req: GroupTargetRequest,
    state: State<'_, Arc<AppState>>,
    app: AppHandle,
) -> Result<(), String> {
    let id = Uuid::parse_str(&req.source_id).map_err(|e| e.to_string())?;
    let state_arc = state.inner().clone();
    let app_cloned = app.clone();
    std::thread::spawn(move || {
        // ① short read lock — snapshot the source.
        let mut src = {
            let guard = state_arc.inner.read();
            match guard.sources.iter().find(|s| s.id == id).cloned() {
                Some(s) => s,
                None => {
                    log::warn!("update_source_group: source {id} gone");
                    return;
                }
            }
        };
        // ② lock-free — pull (github only), with its own timeout.
        if src.kind == SourceKind::Github {
            if let Err(e) = sources::pull_github_source(&mut src) {
                log::warn!("update_source_group: pull failed: {e:#}");
            }
        }
        // ③ write back the refreshed sha, then rescan (short locks).
        {
            let mut guard = state_arc.inner.write();
            if let Some(s) = guard.sources.iter_mut().find(|s| s.id == id) {
                s.last_commit_sha = src.last_commit_sha.clone();
                s.branch = src.branch.clone();
            }
        }
        let _ = state_arc.save();
        run_scan_blocking(&app_cloned, &state_arc, id, "group-update".to_string());

        // ④ re-install (Replace) every skill of this source that is
        // currently installed to any target, so links point at fresh content.
        let to_refresh: Vec<(String, String, Skill)> = {
            let guard = state_arc.inner.read();
            guard
                .installations
                .iter()
                .filter(|i| i.skill_id.starts_with(&format!("{id}:")))
                .filter_map(|i| {
                    guard.skills.get(&i.skill_id).map(|s| (i.skill_id.clone(), i.target_id.clone(), s.clone()))
                })
                .collect()
        };
        for (_sid, tid, skill) in to_refresh {
            let mut guard = state_arc.inner.write();
            if let Err(e) = symlinks::install(&mut guard, &skill, &tid, ConflictPolicy::Fail) {
                log::warn!("update_source_group: reinstall {} -> {tid} failed: {e:#}", skill.id);
            }
        }
        let _ = state_arc.save();
        let _ = app_cloned.emit("skill-dock://source-scanned", id.to_string());
    });
    Ok(())
}

/// Delete a whole group: uninstall all of a source's skills from every
/// target AND hide them (so they don't reappear on rescan). The source
/// record itself stays — only its skills are cleared.
#[tauri::command]
pub fn delete_source_group(
    source_id: String,
    state: State<'_, Arc<AppState>>,
) -> Result<usize, String> {
    let id = Uuid::parse_str(&source_id).map_err(|e| e.to_string())?;
    let mut removed = 0usize;
    {
        let mut guard = state.inner.write();
        let prefix = format!("{id}:");
        // Uninstall from every target.
        let installs: Vec<(String, String)> = guard
            .installations
            .iter()
            .filter(|i| i.skill_id.starts_with(&prefix))
            .map(|i| (i.skill_id.clone(), i.target_id.clone()))
            .collect();
        for (sid, tid) in installs {
            if symlinks::uninstall(&mut guard, &sid, &tid).is_ok() {
                removed += 1;
            }
        }
        // Hide every skill of this source and drop them from the cache.
        let skill_ids: Vec<String> = guard
            .skills
            .keys()
            .filter(|k| k.starts_with(&prefix))
            .cloned()
            .collect();
        for sid in &skill_ids {
            guard.hidden_skills.insert(sid.clone());
            guard.skills.remove(sid);
        }
        if let Some(s) = guard.sources.iter_mut().find(|s| s.id == id) {
            s.skill_count = 0;
        }
    }
    state.save().map_err(|e| e.to_string())?;
    Ok(removed)
}

/// Toggle a source's display mode between Flat and Grouped. Pure UI semantic
/// — does not touch installed symlinks.
#[derive(Debug, Deserialize)]
pub struct SetModeRequest {
    pub source_id: String,
    pub mode: String,
}

#[tauri::command]
pub fn set_source_mode(
    req: SetModeRequest,
    state: State<'_, Arc<AppState>>,
) -> Result<(), String> {
    let id = Uuid::parse_str(&req.source_id).map_err(|e| e.to_string())?;
    let mode = parse_mode(Some(req.mode));
    {
        let mut guard = state.inner.write();
        let src = guard
            .sources
            .iter_mut()
            .find(|s| s.id == id)
            .ok_or_else(|| format!("source not found: {id}"))?;
        src.mode = mode;
    }
    state.save().map_err(|e| e.to_string())?;
    Ok(())
}

/// Set (or clear) a local source's auto-sync targets. When targets is
/// non-empty, a filesystem watcher is started so future changes to the folder
/// trigger an automatic rescan + install. Existing skills are immediately
/// installed to the targets. Only local sources support this (GitHub sources
/// don't have a watchable folder).
#[derive(Debug, Deserialize)]
pub struct SetAutoSyncRequest {
    pub source_id: String,
    pub target_ids: Vec<String>,
}

#[tauri::command]
pub fn set_source_auto_sync(
    req: SetAutoSyncRequest,
    state: State<'_, Arc<AppState>>,
    app: AppHandle,
) -> Result<(), String> {
    let id = Uuid::parse_str(&req.source_id).map_err(|e| e.to_string())?;
    // Read the source kind + path under a short read lock first (for validation
    // + watcher setup), then write the targets back under a short write lock.
    let (kind, location) = {
        let guard = state.inner.read();
        let src = guard
            .sources
            .iter()
            .find(|s| s.id == id)
            .ok_or_else(|| format!("source not found: {id}"))?;
        (src.kind.clone(), src.location.clone())
    };
    if kind != SourceKind::Local {
        return Err("auto-sync is only supported for local sources".to_string());
    }

    // Write the targets back.
    {
        let mut guard = state.inner.write();
        if let Some(src) = guard.sources.iter_mut().find(|s| s.id == id) {
            src.auto_sync_targets = req.target_ids.clone();
        }
    }
    state.save().map_err(|e| e.to_string())?;

    // Keep the watcher alive regardless of the targets — local folders are
    // always watched so changes keep flowing into the library; the targets
    // only control whether rescans also (re)install symlinks.
    crate::watcher::watch(app.clone(), id, PathBuf::from(&location));
    if req.target_ids.is_empty() {
        return Ok(());
    }

    // Immediately install existing skills into the targets (the watcher only
    // fires on future changes; the first install happens here).
    let mut installed = 0usize;
    let mut failed: Vec<String> = Vec::new();
    {
        let mut guard = state.inner.write();
        let skills: Vec<Skill> = guard
            .skills
            .values()
            .filter(|s| s.source_id == id)
            .cloned()
            .collect();
        for tid in &req.target_ids {
            for skill in &skills {
                match symlinks::install(&mut guard, skill, tid, ConflictPolicy::Fail) {
                    Ok(_) => installed += 1,
                    Err(e) => {
                        log::warn!(
                            "set_source_auto_sync: source={id} skill={} target={tid}: {e:#}",
                            skill.name
                        );
                        failed.push(format!("{}: {e}", skill.name));
                    }
                }
            }
        }
    }
    state.save().map_err(|e| e.to_string())?;

    log::info!(
        "set_source_auto_sync: source={id} — {installed} installed, {} failed",
        failed.len()
    );
    let _ = app.emit(
        "skill-dock://auto-synced",
        serde_json::json!({
            "source_id": id.to_string(),
            "installed": installed,
            "failed": failed,
        }),
    );
    Ok(())
}

/// On-demand update check. `git ls-remote` is a network round-trip per GitHub
/// source, so this must run off the main thread — sync commands execute there
/// and would freeze the whole UI for the duration of the fetches.
#[tauri::command]
pub async fn check_updates(state: State<'_, Arc<AppState>>) -> Result<updater::UpdateReport, String> {
    let state_arc = state.inner().clone();
    tauri::async_runtime::spawn_blocking(move || updater::check_now(&state_arc))
        .await
        .map_err(|e| format!("join error: {e}"))?
        .map_err(|e| e.to_string())
}

#[derive(Debug, Deserialize)]
pub struct ApplyUpdateRequest {
    pub source_id: String,
}

/// Pull the latest and rescan one GitHub source. Awaited by the frontend's
/// 更新预览弹窗, so the command must complete only when the pull + scan are
/// actually done — but the network fetch runs on the blocking pool, never the
/// main thread, and never under the shared write lock (a fetch under the lock
/// would freeze the UI for its whole duration).
#[tauri::command]
pub async fn apply_updates(
    req: ApplyUpdateRequest,
    state: State<'_, Arc<AppState>>,
    app: AppHandle,
) -> Result<(), String> {
    let id = Uuid::parse_str(&req.source_id).map_err(|e| e.to_string())?;
    let state_arc = state.inner().clone();
    let app_cloned = app.clone();
    tauri::async_runtime::spawn_blocking(move || -> Result<(), String> {
        // ① short read lock — snapshot the source.
        let mut src = {
            let guard = state_arc.inner.read();
            match guard.sources.iter().find(|s| s.id == id).cloned() {
                Some(s) => s,
                None => return Err("来源不存在或已被删除".into()),
            }
        };
        // ② lock-free — network fetch (has its own timeout). A failed pull
        // leaves the clone untouched, so surface it instead of faking success.
        if src.kind == SourceKind::Github {
            if let Err(e) = sources::pull_github_source(&mut src) {
                log::warn!("apply_updates: pull failed: {e:#}");
                return Err(format!("拉取失败：{e:#}"));
            }
        }
        // ③ short write lock — write the refreshed sha/branch back.
        {
            let mut guard = state_arc.inner.write();
            if let Some(s) = guard.sources.iter_mut().find(|s| s.id == id) {
                s.last_commit_sha = src.last_commit_sha.clone();
                s.branch = src.branch.clone();
            }
        }
        // ④ lock-free — persist + scan.
        if let Err(e) = state_arc.save() {
            log::warn!("apply_updates: save after pull failed: {e:#}");
        }
        run_scan_blocking(&app_cloned, &state_arc, id, "apply-update".to_string());
        let _ = app_cloned.emit("skill-dock://source-scanned", id.to_string());
        Ok(())
    })
    .await
    .map_err(|e| format!("join error: {e}"))?
}

#[tauri::command]
pub fn reveal_in_finder(path: String, app: AppHandle) -> Result<(), String> {
    use tauri_plugin_shell::ShellExt;
    app.shell()
        .open(path, None)
        .map_err(|e| e.to_string())?;
    Ok(())
}

#[tauri::command]
pub fn open_path(path: String, app: AppHandle) -> Result<(), String> {
    use tauri_plugin_shell::ShellExt;
    app.shell()
        .open(path, None)
        .map_err(|e| e.to_string())?;
    Ok(())
}

#[tauri::command]
pub async fn pick_folder(app: AppHandle) -> Result<Option<String>, String> {
    use tauri_plugin_dialog::DialogExt;
    let (tx, rx) = std::sync::mpsc::channel();
    app.dialog().file().pick_folder(move |path| {
        let _ = tx.send(path);
    });
    let result = rx.recv().map_err(|e| e.to_string())?;
    Ok(result.map(|p| p.to_string()))
}

// ---------------------------------------------------------------------------
// 冲突详情：对比托管的 skill 目录与目标位置上的占用目录
// （逐文件 内容哈希 / 大小 / 修改时间）。供详情页冲突弹窗使用。
// ---------------------------------------------------------------------------

#[derive(Debug, Serialize)]
pub struct DirDiffRow {
    /// 相对路径（文件）。
    pub path: String,
    /// same | changed | only_source | only_target
    pub status: String,
    pub source_size: Option<u64>,
    /// epoch 毫秒。
    pub source_mtime: Option<i64>,
    pub target_size: Option<u64>,
    pub target_mtime: Option<i64>,
}

#[derive(Debug, Serialize)]
pub struct SkillTargetDiff {
    /// none | real_dir | real_file | symlink
    pub occupant: String,
    /// 实际参与对比的对方目录（软链解析后）；无法对比时为 null。
    pub compared_dir: Option<String>,
    pub rows: Vec<DirDiffRow>,
    /// 无法对比时的说明。
    pub note: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct SkillTargetDiffRequest {
    pub skill_id: String,
    pub target_id: String,
}

const DIFF_MAX_FILES: usize = 4000;

type Manifest = BTreeMap<String, (u64, i64, String)>;

/// 递归收集目录下所有文件：相对路径 → (大小, 修改时间 ms, 内容哈希)。
/// 软链条目以"链接目标字符串"参与对比（不跟随，避免环）。
fn collect_dir_manifest(root: &Path, prefix: &str, out: &mut Manifest) -> Result<(), String> {
    if out.len() > DIFF_MAX_FILES {
        return Err(format!("文件数超过 {DIFF_MAX_FILES}，目录过大，跳过对比"));
    }
    let entries = std::fs::read_dir(root).map_err(|e| format!("读取 {}：{e}", root.display()))?;
    for entry in entries {
        let entry = entry.map_err(|e| e.to_string())?;
        let name = entry.file_name().to_string_lossy().to_string();
        if name == ".DS_Store" {
            continue;
        }
        let rel = if prefix.is_empty() { name } else { format!("{prefix}/{name}") };
        let path = entry.path();
        let meta = std::fs::symlink_metadata(&path).map_err(|e| format!("{}：{e}", path.display()))?;
        let mtime = meta
            .modified()
            .ok()
            .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
            .map(|d| d.as_millis() as i64)
            .unwrap_or(0);
        if meta.file_type().is_symlink() {
            let dest = std::fs::read_link(&path)
                .map(|p| p.to_string_lossy().to_string())
                .unwrap_or_default();
            out.insert(rel, (0, mtime, format!("symlink:{dest}")));
            continue;
        }
        if meta.is_dir() {
            collect_dir_manifest(&path, &rel, out)?;
            continue;
        }
        let bytes = std::fs::read(&path).map_err(|e| format!("{}：{e}", path.display()))?;
        out.insert(rel, (bytes.len() as u64, mtime, blake3::hash(&bytes).to_hex().to_string()));
    }
    Ok(())
}

/// 详情页冲突弹窗：给出 agent 安装位上"占用者"的类型，并与托管 skill 目录
/// 做逐文件对比。真实目录/文件是用户数据，应用只读不写——对比是只读操作。
#[tauri::command]
pub fn diff_skill_target(
    req: SkillTargetDiffRequest,
    state: State<'_, Arc<AppState>>,
) -> Result<SkillTargetDiff, String> {
    let guard = state.inner.read();
    let skill = guard.skills.get(&req.skill_id).ok_or("skill not found")?;
    let target = guard
        .targets
        .iter()
        .find(|t| t.id == req.target_id)
        .ok_or("target not found")?;

    // 台账优先：安装可能落在默认目录之外的额外地址里；无记录时才按默认目录预测。
    let link_path = guard
        .installations
        .iter()
        .find(|i| i.skill_id == req.skill_id && i.target_id == req.target_id)
        .map(|i| i.link_path.clone())
        .unwrap_or_else(|| target.skills_dir.join(symlinks::link_name_for(skill)));
    let occupant = symlinks::classify(&link_path);

    let kind = match &occupant {
        symlinks::Occupant::Absent => "none",
        symlinks::Occupant::Symlink(_) => "symlink",
        symlinks::Occupant::RealDir => "real_dir",
        symlinks::Occupant::RealFile => "real_file",
    }
    .to_string();

    // 对方目录：真实目录直接用；软链解析到最终指向（指向另一个托管位置时，
    // 对比同样有意义——比如指向旧仓库副本，能看出内容漂移）。
    let other: Option<PathBuf> = match &occupant {
        symlinks::Occupant::RealDir => Some(link_path.clone()),
        symlinks::Occupant::Symlink(dest) => {
            let dest = dest.clone().unwrap_or_else(|| link_path.clone());
            let resolved = std::fs::canonicalize(&dest).unwrap_or(dest);
            if resolved.is_dir() {
                Some(resolved)
            } else {
                None
            }
        }
        _ => None,
    };

    let Some(other) = other else {
        let note = match &occupant {
            symlinks::Occupant::RealFile => "目标位置上是一个普通文件，没有目录可对比。".to_string(),
            symlinks::Occupant::Absent => "该位置现在是空的，没有冲突。".to_string(),
            _ => "占用位置无法作为目录读取。".to_string(),
        };
        return Ok(SkillTargetDiff { occupant: kind, compared_dir: None, rows: vec![], note: Some(note) });
    };

    let mut a: Manifest = BTreeMap::new();
    let mut b: Manifest = BTreeMap::new();
    collect_dir_manifest(&skill.absolute_path, "", &mut a)?;
    collect_dir_manifest(&other, "", &mut b)?;

    let mut names: std::collections::BTreeSet<String> = a.keys().cloned().collect();
    names.extend(b.keys().cloned());
    let mut rows: Vec<DirDiffRow> = Vec::new();
    for name in names {
        let sa = a.get(&name);
        let sb = b.get(&name);
        let status = match (sa, sb) {
            (Some(x), Some(y)) => {
                if x.2 == y.2 { "same" } else { "changed" }
            }
            (Some(_), None) => "only_source",
            (None, Some(_)) => "only_target",
            (None, None) => continue,
        };
        rows.push(DirDiffRow {
            path: name,
            status: status.to_string(),
            source_size: sa.map(|x| x.0),
            source_mtime: sa.map(|x| x.1),
            target_size: sb.map(|x| x.0),
            target_mtime: sb.map(|x| x.1),
        });
    }
    Ok(SkillTargetDiff {
        occupant: kind,
        compared_dir: Some(other.display().to_string()),
        rows,
        note: None,
    })
}

// Allow unused PathBuf import without warnings in some build modes.
#[allow(dead_code)]
fn _unused_pathbuf(p: PathBuf) -> std::path::PathBuf { p }

#[cfg(test)]
mod diff_tests {
    use super::*;
    use std::fs;
    use std::path::PathBuf;

    fn unique_test_dir() -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "skill-dock-diff-test-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn dir_manifest_and_diff_rows_classify_files() {
        let root = unique_test_dir();
        let a = root.join("repo");   // 托管 skill 目录
        let b = root.join("agent");  // agent 侧占用目录
        fs::create_dir_all(a.join("sub")).unwrap();
        fs::create_dir_all(b.join("sub")).unwrap();
        fs::write(a.join("SKILL.md"), "hello").unwrap();
        fs::write(a.join("sub/x.sh"), "echo hi").unwrap();
        // agent 侧：一份一致、一份改过、一份仅 agent 有
        fs::write(b.join("SKILL.md"), "hello").unwrap();
        fs::write(b.join("sub/x.sh"), "echo changed").unwrap();
        fs::write(b.join("extra.txt"), "only in b").unwrap();

        let mut ma: Manifest = BTreeMap::new();
        let mut mb: Manifest = BTreeMap::new();
        collect_dir_manifest(&a, "", &mut ma).unwrap();
        collect_dir_manifest(&b, "", &mut mb).unwrap();
        assert_eq!(ma.len(), 2);
        assert_eq!(mb.len(), 3);
        assert_eq!(ma["SKILL.md"].2, mb["SKILL.md"].2, "一致文件哈希相同");
        assert_ne!(ma["sub/x.sh"].2, mb["sub/x.sh"].2, "改动文件哈希不同");
        assert!(mb.contains_key("extra.txt") && !ma.contains_key("extra.txt"));

        // 汇总成行：与 diff_skill_target 的行分类一致。
        let mut names: std::collections::BTreeSet<String> = ma.keys().cloned().collect();
        names.extend(mb.keys().cloned());
        let mut statuses = vec![];
        for name in &names {
            let (sa, sb) = (ma.get(name), mb.get(name));
            statuses.push(match (sa, sb) {
                (Some(x), Some(y)) => if x.2 == y.2 { "same" } else { "changed" },
                (Some(_), None) => "only_source",
                (None, Some(_)) => "only_target",
                (None, None) => unreachable!(),
            });
        }
        assert_eq!(statuses.iter().filter(|s| **s == "same").count(), 1);
        assert_eq!(statuses.iter().filter(|s| **s == "changed").count(), 1);
        assert_eq!(statuses.iter().filter(|s| **s == "only_target").count(), 1);
        assert_eq!(statuses.iter().filter(|s| **s == "only_source").count(), 0);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;
    use std::fs;
    use std::time::{SystemTime, UNIX_EPOCH};

    #[cfg(unix)]
    #[test]
    fn discovers_local_managed_and_external_symlink_entries() {
        let root = unique_test_dir();
        let target_dir = root.join("target");
        let managed_source = root.join("managed-source");
        let external_source = root.join("external-source");
        fs::create_dir_all(&target_dir).unwrap();
        fs::create_dir_all(&managed_source).unwrap();
        fs::create_dir_all(&external_source).unwrap();
        fs::write(managed_source.join("SKILL.md"), "# managed").unwrap();
        fs::write(external_source.join("SKILL.md"), "# external").unwrap();

        let local_dir = target_dir.join("local-skill");
        fs::create_dir_all(&local_dir).unwrap();
        fs::write(local_dir.join("SKILL.md"), "# local").unwrap();

        let managed_link = target_dir.join("managed-skill");
        let external_link = target_dir.join("external-skill");
        std::os::unix::fs::symlink(&managed_source, &managed_link).unwrap();
        std::os::unix::fs::symlink(&external_source, &external_link).unwrap();

        let target = AgentTarget {
            id: "target".to_string(),
            name: "Test Agent".to_string(),
            skills_dir: target_dir.clone(),
            extra_dirs: Vec::new(),
            enabled: true,
            description: String::new(),
            tag: None,
            detected: true,
            user_touched: false,
        };
        let skill = Skill {
            id: "managed-skill-id".to_string(),
            source_id: Uuid::new_v4(),
            name: "managed-skill".to_string(),
            description: String::new(),
            relative_path: PathBuf::from("managed-skill"),
            absolute_path: managed_source.clone(),
            has_skill_md: true,
            content_hash: String::new(),
            modified_at: Utc::now(),
            size_bytes: 0,
            is_shared: false,
            is_symlink: false,
            symlink_target: None,
        };
        let installation = Installation {
            skill_id: skill.id.clone(),
            target_id: target.id.clone(),
            link_path: managed_link.clone(),
            installed_at: Utc::now(),
            status: String::new(),
        };
        let mut skills = BTreeMap::new();
        skills.insert(skill.id.clone(), skill);

        let entries = discover_target_skills(
            std::slice::from_ref(&target),
            &[installation],
            &skills,
        );

        assert_eq!(entries.len(), 3);
        let by_name: std::collections::HashMap<_, _> = entries
            .iter()
            .map(|entry| (entry.name.as_str(), entry))
            .collect();
        assert_eq!(by_name["local-skill"].kind, ExistingSkillKind::Local);
        assert!(!by_name["local-skill"].is_symlink);
        assert_eq!(
            by_name["managed-skill"].kind,
            ExistingSkillKind::Managed
        );
        assert_eq!(
            by_name["managed-skill"].symlink_target.as_deref().map(PathBuf::from),
            Some(canonical_or_original(&managed_source))
        );
        assert_eq!(
            by_name["external-skill"].kind,
            ExistingSkillKind::ExternalSymlink
        );
        assert_eq!(by_name["managed-skill"].target_id, "target");
        assert_eq!(by_name["managed-skill"].target_name, "Test Agent");
        assert_eq!(by_name["managed-skill"].target_skills_dir, target_dir.to_string_lossy());

        fs::remove_dir_all(root).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn repointed_record_is_not_reported_as_managed() {
        let root = unique_test_dir();
        let target_dir = root.join("target");
        let recorded_source = root.join("recorded-source");
        let repointed_source = root.join("repointed-source");
        fs::create_dir_all(&target_dir).unwrap();
        fs::create_dir_all(&recorded_source).unwrap();
        fs::create_dir_all(&repointed_source).unwrap();
        fs::write(recorded_source.join("SKILL.md"), "# recorded").unwrap();
        fs::write(repointed_source.join("SKILL.md"), "# repointed").unwrap();

        let link = target_dir.join("skill");
        std::os::unix::fs::symlink(&repointed_source, &link).unwrap();
        let target = AgentTarget {
            id: "target".to_string(),
            name: "Test Agent".to_string(),
            skills_dir: target_dir,
            extra_dirs: Vec::new(),
            enabled: true,
            description: String::new(),
            tag: None,
            detected: true,
            user_touched: false,
        };
        let skill = Skill {
            id: "skill-id".to_string(),
            source_id: Uuid::new_v4(),
            name: "skill".to_string(),
            description: String::new(),
            relative_path: PathBuf::from("skill"),
            absolute_path: recorded_source,
            has_skill_md: true,
            content_hash: String::new(),
            modified_at: Utc::now(),
            size_bytes: 0,
            is_shared: false,
            is_symlink: false,
            symlink_target: None,
        };
        let installation = Installation {
            skill_id: skill.id.clone(),
            target_id: target.id.clone(),
            link_path: link,
            installed_at: Utc::now(),
            status: String::new(),
        };
        let mut skills = BTreeMap::new();
        skills.insert(skill.id.clone(), skill);

        let entries = discover_target_skills(std::slice::from_ref(&target), &[installation], &skills);
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].kind, ExistingSkillKind::ExternalSymlink);

        fs::remove_dir_all(root).unwrap();
    }

    fn unique_test_dir() -> PathBuf {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let path = std::env::temp_dir().join(format!(
            "skill-dock-discovery-{}-{nanos}",
            std::process::id()
        ));
        fs::create_dir_all(&path).unwrap();
        path
    }
}
