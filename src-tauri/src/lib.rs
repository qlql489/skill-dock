//! SkillDock — Tauri backend
//!
//! Manages AI agent skills from local folders and GitHub repositories.
//! All skills are installed to target agent directories via symlinks.

mod state;
mod scanner;
mod sources;
mod targets;
mod symlinks;
mod updater;
mod paths;
mod commands;
mod detect;
mod watcher;
mod groups;
mod archive;
mod source_updates;
mod discover;
mod market;
mod projects;

use state::AppState;
use std::sync::Arc;
use tauri::Manager;

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    // Honor RUST_LOG; default to info if not set.
    if std::env::var("RUST_LOG").is_err() {
        std::env::set_var("RUST_LOG", "skill_dock_lib=info,info");
    }
    let _ = env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("skill_dock_lib=info,info"))
        .format_timestamp_secs()
        .try_init();

    eprintln!("[skill-dock] starting (log level: {})",
        std::env::var("RUST_LOG").unwrap_or_else(|_| "info".to_string()));

    tauri::Builder::default()
        // 单实例守卫：必须最先注册。两个实例共用 SkillDock 数据状态，
        // 双开会互相用各自的内存状态覆盖对方落盘，造成静默丢数据。
        .plugin(tauri_plugin_single_instance::init(|app, _args, _cwd| {
            if let Some(w) = app.get_webview_window("main") {
                let _ = w.show();
                let _ = w.set_focus();
            }
        }))
        .plugin(tauri_plugin_shell::init())
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_fs::init())
        .plugin(tauri_plugin_process::init())
        .setup(|app| {
            // Resolve the current data directory (or the legacy directory for upgrades).
            let data_dir = paths::data_dir()?;
            std::fs::create_dir_all(&data_dir)?;
            std::fs::create_dir_all(data_dir.join("repos"))?;

            let state = Arc::new(AppState::load(data_dir)?);

            // Probe installed agents and auto-enable the ones the user hasn't
            // manually touched. Runs once on startup; the "redetect_targets"
            // command re-runs it on demand. Done before manage() so the first
            // frontend get_state already reflects detection results.
            {
                let mut guard = state.inner.write();
                detect::detect_all(&mut guard.targets);
                detect::auto_enable_untouched(&mut guard.targets);
                // Tag stale/conflicted installation records so the 概览 page
                // can surface them (previously these were silently dropped).
                match symlinks::reconcile(&mut guard) {
                    Ok(n) if n > 0 => log::info!("startup reconcile: {n} status change(s)"),
                    Ok(_) => {}
                    Err(e) => log::warn!("startup reconcile failed: {e:#}"),
                }
            }
            // Persist the detection-driven enable changes (best-effort).
            let _ = state.save();

            // Start filesystem watchers before managing, while we still own a
            // borrowed reference to the state. watchers only need a snapshot.
            watcher::init_all(app.handle(), &state);
            app.manage(state);

            // App self-update (tauri-plugin-updater). Desktop only — the
            // plugin has no mobile implementation.
            #[cfg(desktop)]
            app.handle()
                .plugin(tauri_plugin_updater::Builder::new().build())?;

            // Fire background updater (every 30 min)
            updater::spawn_background(app.handle().clone());

            // One-shot cleanup: remove orphaned clone directories under
            // repos/ that no source record claims. These accumulate when a
            // past clone failed halfway (leaving an empty `.git` shell) or
            // when a source was removed but its clone dir survived a crash.
            // Only touches dirs NOT referenced by any source.clone_path.
            cleanup_orphan_repos(app.state::<Arc<AppState>>().inner.read().sources.clone());

            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            commands::get_state,
            commands::add_local_source,
            commands::add_github_source,
            commands::retry_clone_source,
            commands::remove_source,
            commands::rescan_source,
            commands::rescan_all,
            commands::scan_all,
            commands::read_skill_md,
            commands::write_skill_md,
            commands::list_targets,
            commands::list_target_skills,
            commands::discover_existing_skills,
            commands::read_skill_tree,
            commands::read_skill_file,
            commands::redetect_targets,
            commands::add_target,
            commands::update_target,
            commands::classify_target_path,
            commands::reset_target_dir,
            commands::list_builtin_dirs,
            commands::remove_target,
            commands::install_skill,
            commands::uninstall_skill,
            commands::delete_skill,
            commands::diff_skill_target,
            commands::install_source_group,
            commands::uninstall_source_group,
            commands::update_source_group,
            commands::delete_source_group,
            commands::set_source_mode,
            commands::set_source_auto_sync,
            commands::check_updates,
            commands::apply_updates,
            commands::reveal_in_finder,
            commands::open_path,
            commands::pick_folder,
            commands::get_state_path,
            groups::create_group,
            groups::update_group,
            groups::delete_group,
            groups::apply_group,
            groups::preview_cells,
            groups::apply_cells,
            groups::hide_source_skills,
            groups::unhide_source_skills,
            groups::pending_items,
            groups::cleanup_installations,
            groups::resolve_skill_by_path,
            groups::reorder_targets,
            groups::get_accent_color,
            groups::set_accent_color,
            groups::get_add_default_install_targets,
            groups::set_add_default_install_targets,
            groups::delete_skill_forever,
            archive::add_zip_source,
            archive::add_zip_bytes,
            archive::pick_archive_file,
            source_updates::preview_source_update,
            source_updates::source_file_patch,
            discover::discover_local_skills,
            discover::adopt_local_skill,
            discover::local_skill_meta,
            discover::get_vault_path,
            discover::set_vault_path,
            market::market_search,
            market::market_leaderboard,
            market::market_registry,
            market::install_market_skill,
            projects::get_projects,
            projects::add_project,
            projects::remove_project,
            projects::scan_project_roots,
            projects::get_project_slots,
            projects::get_project_skills,
            projects::get_project_skill_doc,
            projects::toggle_project_skill,
            projects::delete_project_skill,
            projects::adopt_project_skill,
            projects::export_skill_to_project,
            projects::update_project_skill_from_center,
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}

/// Remove orphaned directories under the app data `repos/` directory that no source
/// claims as its `clone_path`. These pile up from crashed/failed clones. Only
/// touches the top level of `repos/`, and only deletes a directory whose name
/// does not match any source's UUID — so it never touches live clones.
fn cleanup_orphan_repos(sources: Vec<crate::state::Source>) {
    let repos = match paths::repos_dir() {
        Ok(p) => p,
        Err(_) => return,
    };
    let entries = match std::fs::read_dir(&repos) {
        Ok(e) => e,
        Err(_) => return,
    };
    // Collect the set of clone dirs the live state owns.
    let claimed: std::collections::HashSet<std::path::PathBuf> = sources
        .iter()
        .filter_map(|s| s.clone_path.clone())
        .collect();

    let mut removed = 0;
    for entry in entries.flatten() {
        let path = entry.path();
        if !path.is_dir() {
            continue;
        }
        if claimed.contains(&path) {
            continue;
        }
        // Orphan — best-effort delete.
        match std::fs::remove_dir_all(&path) {
            Ok(()) => {
                removed += 1;
                log::info!("startup cleanup: removed orphan {}", path.display());
            }
            Err(e) => log::warn!("startup cleanup: could not remove {}: {e}", path.display()),
        }
    }
    if removed > 0 {
        log::info!("startup cleanup: removed {removed} orphan repo dir(s)");
    }
}
