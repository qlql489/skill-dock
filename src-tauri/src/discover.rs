//! 本机 Skill 扫描与收编 —— 以 Skill 为中心的全局发现。
//!
//! 1. `discover_local_skills`：扫描所有启用 Agent 的 skills 目录，按
//!    "真实文件"分组（软链解析到源头聚合），并标注该源头是否已在
//!    当前添加的来源（技能库）里。
//! 2. `adopt_local_skill`（收编）：把不在技能库里的真实目录复制到
//!    指定目的地（默认中央仓库 ~/.agents/skills，或任一本地来源），
//!    然后把指向旧位置的所有软链原地改指向新副本——迁移过程不断链。

use crate::state::{AppState, CloneStatus, Installation, Source, SourceKind, SourceMode};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tauri::{AppHandle, Emitter, State};
use uuid::Uuid;

#[derive(Debug, Clone, Serialize)]
pub struct DiscoveredLink {
    pub target_id: String,
    pub target_name: String,
    /// Agent 目录里的条目名。
    pub entry_name: String,
    pub link_path: String,
    /// true = 该条目本身是软链；false = 真实目录就躺在 Agent 目录里。
    pub is_symlink: bool,
}

#[derive(Debug, Serialize)]
pub struct DiscoveredSkillGroup {
    /// 解析后的真实目录（已 canonicalize）。
    pub real_path: String,
    pub name: String,
    pub has_skill_md: bool,
    /// 已纳管时 = 技能库中对应 skill 的 id（详情路由用）；未纳管 = None。
    pub skill_id: Option<String>,
    /// 已纳管时 = 技能库中对应来源；未纳管 = None。
    pub source_id: Option<String>,
    pub source_name: Option<String>,
    pub links: Vec<DiscoveredLink>,
}

/// 未纳管 skill 的详情元数据（详情页头部与信息区用）。
#[derive(Debug, Serialize)]
pub struct LocalSkillMeta {
    pub name: String,
    pub description: String,
    pub size_bytes: u64,
    /// SKILL.md 的修改时间（ISO 8601，与库内 Skill 序列化口径一致）。
    pub modified_at: Option<chrono::DateTime<chrono::Utc>>,
}

/// 解析 Agent 目录里的一个条目 → (真实路径, 是否软链)。
fn resolve_entry(dir: &Path, name: &str) -> Option<(PathBuf, bool)> {
    let p = dir.join(name);
    let meta = std::fs::symlink_metadata(&p).ok()?;
    if meta.file_type().is_symlink() {
        let dest = std::fs::read_link(&p).ok()?;
        let real = if dest.is_absolute() { dest } else { dir.join(dest) };
        Some((real, true))
    } else if meta.is_dir() {
        Some((p, false))
    } else {
        None
    }
}

/// 扫描 Agent 的 skills 目录，按真实文件聚合成组。
/// 默认只扫启用的 Agent（全局视角）；`include_disabled` = true 时扫全部
/// （本机skill管理的 agents 视角需要看到并管理未启用 Agent 里的内容）。
#[tauri::command]
pub fn discover_local_skills(
    state: State<'_, Arc<AppState>>,
    include_disabled: Option<bool>,
) -> Result<Vec<DiscoveredSkillGroup>, String> {
    let snap = state.snapshot();
    let scan_all = include_disabled.unwrap_or(false);
    let mut groups: BTreeMap<String, DiscoveredSkillGroup> = BTreeMap::new();

    for t in snap.targets.iter().filter(|t| scan_all || t.enabled) {
        // 默认目录 + 额外地址一起扫；软链解析到源头后全局聚合，条目仍按
        // 所属目录记录 link_path。
        for dir in crate::targets::all_dirs(t) {
            let entries = match std::fs::read_dir(&dir) {
                Ok(e) => e,
                Err(_) => continue,
            };
            for entry in entries.flatten() {
                let name = entry.file_name().to_string_lossy().to_string();
                if name.starts_with('.') {
                    continue;
                }
                let Some((real, is_link)) = resolve_entry(&dir, &name) else {
                    continue;
                };
                if !real.join("SKILL.md").is_file() {
                    continue; // 没有 SKILL.md 的目录不算 skill
                }
                let canonical = std::fs::canonicalize(&real).unwrap_or_else(|_| real.clone());
                let key = canonical.to_string_lossy().to_string();
                let g = groups.entry(key.clone()).or_insert_with(|| DiscoveredSkillGroup {
                    real_path: key.clone(),
                    name: canonical
                        .file_name()
                        .map(|n| n.to_string_lossy().to_string())
                        .unwrap_or_else(|| name.clone()),
                    has_skill_md: true,
                    skill_id: None,
                    source_id: None,
                    source_name: None,
                    links: Vec::new(),
                });
                g.links.push(DiscoveredLink {
                    target_id: t.id.clone(),
                    target_name: t.name.clone(),
                    entry_name: name.clone(),
                    link_path: dir.join(&name).to_string_lossy().to_string(),
                    is_symlink: is_link,
                });
            }
        }
    }

    // 标注归属：真实路径命中技能库里某个 skill 的 absolute_path 即"已纳管"。
    let mut canon_cache: BTreeMap<String, PathBuf> = BTreeMap::new();
    for g in groups.values_mut() {
        let rp = PathBuf::from(&g.real_path);
        for s in snap.skills.values() {
            let ap = s.absolute_path.to_string_lossy().to_string();
            let apc = canon_cache
                .entry(ap.clone())
                .or_insert_with(|| std::fs::canonicalize(&s.absolute_path).unwrap_or_else(|_| s.absolute_path.clone()));
            if *apc == rp {
                g.skill_id = Some(s.id.clone());
                g.source_id = Some(s.source_id.to_string());
                g.source_name = snap.sources.iter().find(|x| x.id == s.source_id).map(|x| x.name.clone());
                break;
            }
        }
    }

    Ok(groups.into_values().collect())
}

// ---------------------------------------------------------------------------
// 收编（adopt）
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
pub struct AdoptRequest {
    pub real_path: String,
    /// None = 默认中央仓库 ~/.agents/skills；Some = 某个本地来源的 id。
    pub dest_source_id: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct AdoptResult {
    pub new_path: String,
    /// 改指向的软链数量。
    pub repointed: usize,
    /// 原本"真身"躺在 Agent 目录里、被移入废纸篓的条目数。
    pub moved_original: usize,
    pub source_id: String,
    /// true = 中央仓库尚未注册来源，本次自动注册了一个。
    pub source_registered: bool,
}

/// 递归复制目录，跳过 .git/.DS_Store 等点文件与内部符号链接。
pub(crate) fn copy_dir_filtered(src: &Path, dst: &Path) -> Result<(), String> {
    std::fs::create_dir_all(dst).map_err(|e| format!("create {}: {e}", dst.display()))?;
    for entry in std::fs::read_dir(src).map_err(|e| format!("read {}: {e}", src.display()))? {
        let entry = entry.map_err(|e| e.to_string())?;
        let name = entry.file_name().to_string_lossy().to_string();
        if name.starts_with('.') {
            continue;
        }
        let p = entry.path();
        let Ok(meta) = std::fs::symlink_metadata(&p) else { continue };
        if meta.file_type().is_symlink() {
            continue; // 内部软链不落地（防逃逸）
        }
        if meta.is_dir() {
            copy_dir_filtered(&p, &dst.join(&name))?;
        } else {
            std::fs::copy(&p, &dst.join(&name)).map_err(|e| format!("copy {}: {e}", p.display()))?;
        }
    }
    Ok(())
}

/// 解析生效的中央仓库目录：配置了 ui.vault_path 用配置值（绝对路径），
/// 否则回落内置默认。scanner 的 is_shared 判定也走这里，保证两处口径一致。
pub(crate) fn effective_vault_dir(ui_vault_path: &str) -> Option<PathBuf> {
    let trimmed = ui_vault_path.trim();
    if !trimmed.is_empty() {
        return Some(PathBuf::from(trimmed));
    }
    crate::paths::data_dir().ok().map(|dir| dir.join("skills"))
}

fn vault_dir(state: &AppState) -> Option<PathBuf> {
    effective_vault_dir(&state.snapshot().ui.vault_path)
}

/// 找到「中央仓库」来源（local 且 location == vault），没有就注册一个。
/// 压缩包导入、文件夹拷贝导入、收编三条路径共用的注册口径。
pub(crate) fn find_or_register_vault_source(
    state: &AppState,
    vault: &Path,
) -> Result<Source, String> {
    let mut guard = state.inner.write();
    if let Some(s) = guard
        .sources
        .iter()
        .find(|s| s.kind == SourceKind::Local && Path::new(&s.location) == vault)
    {
        return Ok(s.clone());
    }
    let src = Source {
        id: Uuid::new_v4(),
        kind: SourceKind::Local,
        name: "中央仓库".to_string(),
        location: vault.to_string_lossy().to_string(),
        clone_path: None,
        branch: None,
        created_at: chrono::Utc::now(),
        last_scanned_at: None,
        last_commit_sha: None,
        skill_count: 0,
        clone_status: CloneStatus::Ready,
        clone_error: None,
        mode: SourceMode::Flat,
        auto_sync_targets: Vec::new(),
    };
    guard.sources.push(src.clone());
    Ok(src)
}

/// 拷贝落位 + 来源注册（纯逻辑，不碰 watcher/扫描线程，便于单测）。
/// 返回（中央仓库来源, 仓库根目录）。
fn install_dir_into_vault_core(
    state: &AppState,
    src_dir: &Path,
    preferred_name: &str,
) -> Result<(Source, PathBuf), String> {
    let vault = vault_dir(state).ok_or("无法确定主目录")?;
    std::fs::create_dir_all(&vault).map_err(|e| format!("create {}: {e}", vault.display()))?;

    let mut folder = preferred_name.trim().to_string();
    if folder.is_empty() || folder.starts_with('.') {
        folder = src_dir
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_else(|| "import".to_string());
    }

    let mut dest = vault.join(&folder);
    let mut n = 2;
    while dest.exists() {
        dest = vault.join(format!("{folder}-{n}"));
        n += 1;
    }
    copy_dir_filtered(src_dir, &dest)?;
    log::info!("install_dir_into_vault: copied {} -> {}", src_dir.display(), dest.display());

    let src = find_or_register_vault_source(state, &vault)?;
    Ok((src, vault))
}

/// 把 `src_dir` 拷贝为 `<中央仓库>/<文件夹名>`（重名自动 -2/-3），复用或注册
/// 「中央仓库」来源，然后触发后台扫描。压缩包导入与「本地文件夹 → 拷贝到
/// 中央仓库」共用这条落库路径。慢 IO（复制）由调用方放在 spawn_blocking 里，
/// 本函数内部的锁只覆盖注册瞬间。
/// `preferred_name` 为空时回落 `src_dir` 本身的目录名。
pub(crate) fn install_dir_into_vault(
    state: &Arc<AppState>,
    app: &AppHandle,
    src_dir: &Path,
    preferred_name: &str,
) -> Result<Source, String> {
    let (src, vault) = install_dir_into_vault_core(state, src_dir, preferred_name)?;
    state.save().map_err(|e| e.to_string())?;

    // 中央仓库内容变化需要被感知：确保 watcher 挂上（watch 按 id 幂等替换）。
    crate::watcher::watch(app.clone(), src.id, vault);
    let state_for_thread = state.clone();
    let app_for_thread = app.clone();
    let source_id = src.id;
    std::thread::spawn(move || {
        crate::commands::run_scan_blocking(&app_for_thread, &state_for_thread, source_id, "vault-import".to_string());
        let _ = app_for_thread.emit("skill-dock://source-scanned", source_id.to_string());
    });

    Ok(src)
}

/// 收编一个本机 skill：复制 → 重写软链 → （需要时）注册中央仓库来源 → 触发扫描。
#[tauri::command]
pub fn adopt_local_skill(
    req: AdoptRequest,
    state: State<'_, Arc<AppState>>,
    app: AppHandle,
) -> Result<AdoptResult, String> {
    let real = PathBuf::from(req.real_path.trim());
    if !real.is_dir() {
        return Err(format!("目录不存在：{}", real.display()));
    }
    let name = real
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .ok_or("无法解析目录名")?;

    // ① 解析目的地根目录 + 来源 id。
    let (root, source_id, source_registered): (PathBuf, Uuid, bool) = match &req.dest_source_id {
        Some(id) => {
            let uid = Uuid::parse_str(id).map_err(|e| e.to_string())?;
            let snap = state.snapshot();
            let src = snap
                .sources
                .iter()
                .find(|s| s.id == uid && s.kind == SourceKind::Local)
                .ok_or("指定的本地来源不存在")?;
            (PathBuf::from(&src.location), uid, false)
        }
        None => {
            let vault = vault_dir(&state).ok_or("无法确定主目录")?;
            let snap = state.snapshot();
            match snap
                .sources
                .iter()
                .find(|s| s.kind == SourceKind::Local && Path::new(&s.location) == vault)
            {
                Some(s) => (vault, s.id, false),
                None => (vault, Uuid::new_v4(), true),
            }
        }
    };
    std::fs::create_dir_all(&root).map_err(|e| format!("create {}: {e}", root.display()))?;

    let canonical = std::fs::canonicalize(&real).unwrap_or_else(|_| real.clone());
    let root_canonical = std::fs::canonicalize(&root).unwrap_or_else(|_| root.clone());
    // 真身本就躺在目的地里（比如来源刚注册、扫描还没索引到它）：
    // 不复制也不改链，只补注册 + 扫描，否则会"自己收编自己"撞出 -2 副本。
    let already_in_dest = canonical.starts_with(&root_canonical);

    // ② 复制（重名自动 -2/-3）。
    let dest = if already_in_dest {
        canonical.clone()
    } else {
        let mut dest = root.join(&name);
        let mut n = 2;
        while dest.exists() {
            dest = root.join(format!("{name}-{n}"));
            n += 1;
        }
        copy_dir_filtered(&real, &dest)?;
        log::info!("adopt: copied {} -> {}", real.display(), dest.display());
        dest
    };

    // ③ 单锁内完成：重写软链 / 迁移真身 + 预登记安装记录 + （需要时）注册中央仓库来源。
    let mut repointed = 0usize;
    let mut moved_original = 0usize;
    let mut source: Option<Source> = None;
    {
        let mut guard = state.inner.write();
        let snap = guard.clone();

        if source_registered {
            let now = chrono::Utc::now();
            let src = Source {
                id: source_id,
                kind: SourceKind::Local,
                name: "中央仓库".to_string(),
                location: root.to_string_lossy().to_string(),
                clone_path: None,
                branch: None,
                created_at: now,
                last_scanned_at: None,
                last_commit_sha: None,
                skill_count: 0,
                clone_status: CloneStatus::Ready,
                clone_error: None,
                mode: SourceMode::Flat,
                auto_sync_targets: Vec::new(),
            };
            guard.sources.push(src.clone());
            source = Some(src);
        }

        // 扫全部 Agent（含未启用）：agents 视角对未启用 Agent 里的落点
        // 收编时，软链同样要改指向新副本，否则会留下断链。
        // 默认目录与额外地址都参与；真身已在目的地里时无需改链。
        for t in snap.targets.iter().filter(|_| !already_in_dest) {
            for t_dir in crate::targets::all_dirs(t) {
            let entries = match std::fs::read_dir(&t_dir) {
                Ok(e) => e,
                Err(_) => continue,
            };
            for entry in entries.flatten() {
                let entry_name = entry.file_name().to_string_lossy().to_string();
                if entry_name.starts_with('.') {
                    continue;
                }
                let p = t_dir.join(&entry_name);
                let Ok(meta) = std::fs::symlink_metadata(&p) else { continue };
                let (link_real, is_symlink) = if meta.file_type().is_symlink() {
                    match std::fs::read_link(&p) {
                        Ok(d) => {
                            let r = if d.is_absolute() { d } else { t_dir.join(d) };
                            (r, true)
                        }
                        Err(_) => continue,
                    }
                } else if meta.is_dir() {
                    (p.clone(), false)
                } else {
                    continue;
                };
                let lc = std::fs::canonicalize(&link_real).unwrap_or_else(|_| link_real.clone());
                if lc != canonical {
                    continue;
                }
                // 命中：改写为指向新副本的软链。
                if is_symlink {
                    std::fs::remove_file(&p).map_err(|e| format!("remove {}: {e}", p.display()))?;
                } else {
                    crate::groups::trash_dir(&p)?;
                    moved_original += 1;
                }
                #[cfg(unix)]
                {
                    std::os::unix::fs::symlink(&dest, &p)
                        .map_err(|e| format!("link {} -> {}: {e}", p.display(), dest.display()))?;
                }
                repointed += 1;
                // 预登记安装记录：扫描完成后该 skill 的 id 是可预测的
                // （source_id:相对路径），登记后用户再点徽章不会撞"已被占用"。
                let rel = dest.strip_prefix(&root).unwrap_or(&dest);
                let skill_id = format!("{}:{}", source_id, rel.to_string_lossy());
                guard.installations.retain(|i| !(i.skill_id == skill_id && i.target_id == t.id));
                guard.installations.push(Installation {
                    skill_id,
                    target_id: t.id.clone(),
                    link_path: p,
                    installed_at: chrono::Utc::now(),
                    status: "ok".to_string(),
                });
            }
            }
        }
    } // release write lock

    if let Some(src) = &source {
        log::info!("adopt: registered central vault source {}", src.id);
    }
    state.save().map_err(|e| e.to_string())?;

    // ④ 后台扫描该来源，把新副本索引进技能库。
    let state_for_thread = state.inner().clone();
    let app_for_thread = app.clone();
    std::thread::spawn(move || {
        crate::commands::run_scan_blocking(&app_for_thread, &state_for_thread, source_id, "adopt".to_string());
        let _ = app_for_thread.emit("skill-dock://source-scanned", source_id.to_string());
    });

    Ok(AdoptResult {
        new_path: dest.to_string_lossy().to_string(),
        repointed,
        moved_original,
        source_id: source_id.to_string(),
        source_registered,
    })
}

// ---------------------------------------------------------------------------
// 中央仓库路径（收编默认目的地）
// ---------------------------------------------------------------------------

#[tauri::command]
pub fn get_vault_path(state: State<'_, Arc<AppState>>) -> Result<String, String> {
    let configured = state.snapshot().ui.vault_path;
    effective_vault_dir(&configured)
        .map(|p| p.to_string_lossy().to_string())
        .ok_or_else(|| "无法确定主目录".to_string())
}

/// 设置中央仓库路径。空串 = 恢复内置默认目录。
/// 接受 `~` 开头的路径，落库前展开为绝对路径；只改配置不迁移文件，
/// 已注册的来源不受影响。
#[tauri::command]
pub fn set_vault_path(path: String, state: State<'_, Arc<AppState>>) -> Result<(), String> {
    let trimmed = path.trim();
    let expanded = if trimmed.is_empty() {
        String::new()
    } else if trimmed == "~" || trimmed.starts_with("~/") {
        let home = dirs::home_dir().ok_or("无法确定主目录")?;
        home.join(trimmed[1..].trim_start_matches('/'))
            .to_string_lossy()
            .to_string()
    } else {
        trimmed.to_string()
    };
    let resolved = effective_vault_dir(&expanded).ok_or("无法确定主目录")?;
    if !expanded.is_empty() && !resolved.is_absolute() {
        return Err(format!("请填写绝对路径：{}", resolved.display()));
    }
    std::fs::create_dir_all(&resolved)
        .map_err(|e| format!("create {}: {e}", resolved.display()))?;
    {
        let mut guard = state.inner.write();
        guard.ui.vault_path = expanded;
    }
    state.save().map_err(|e| e.to_string())?;
    Ok(())
}

// ---------------------------------------------------------------------------
// 未纳管 skill 的详情元数据
// ---------------------------------------------------------------------------

/// 按磁盘路径读取一个（未纳管）skill 的名称/描述/大小/修改时间。
/// 口径与扫描器一致：描述取 SKILL.md frontmatter，大小只累加顶层文件。
#[tauri::command]
pub fn local_skill_meta(root_path: String) -> Result<LocalSkillMeta, String> {
    let dir = PathBuf::from(root_path.trim());
    let skill_md = dir.join("SKILL.md");
    if !skill_md.is_file() {
        return Err(format!("目录不存在或没有 SKILL.md：{}", dir.display()));
    }
    let (fm_name, description) = crate::scanner::parse_frontmatter(&skill_md);
    let name = if fm_name.is_empty() {
        dir.file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or(fm_name)
    } else {
        fm_name
    };
    let size_bytes = crate::scanner::skill_size(&dir).unwrap_or(0);
    let modified_at = crate::scanner::file_modified(&skill_md);
    Ok(LocalSkillMeta {
        name,
        description,
        size_bytes,
        modified_at,
    })
}

#[cfg(test)]
mod vault_source_tests {
    use super::*;

    fn test_state(tmp: &Path) -> AppState {
        let mut persisted = crate::state::PersistedState::current();
        persisted.ui.vault_path = tmp.join("vault").to_string_lossy().to_string();
        AppState {
            data_dir: tmp.to_path_buf(),
            state_path: tmp.join("state.json"),
            inner: parking_lot::RwLock::new(persisted),
        }
    }

    #[test]
    fn vault_source_registers_once_and_reuses() {
        let tmp = std::env::temp_dir().join(format!("sm-vault-src-{}", Uuid::new_v4()));
        std::fs::create_dir_all(&tmp).unwrap();
        let state = test_state(&tmp);
        let vault = effective_vault_dir(&state.snapshot().ui.vault_path).unwrap();

        let first = find_or_register_vault_source(&state, &vault).unwrap();
        assert_eq!(first.name, "中央仓库");
        assert_eq!(first.mode, SourceMode::Flat);
        assert_eq!(Path::new(&first.location), vault);
        assert_eq!(state.inner.read().sources.len(), 1);

        let second = find_or_register_vault_source(&state, &vault).unwrap();
        assert_eq!(second.id, first.id, "第二次应复用已注册的中央仓库来源");
        assert_eq!(state.inner.read().sources.len(), 1);

        std::fs::remove_dir_all(&tmp).unwrap();
    }

    #[test]
    fn vault_copy_lands_under_vault_with_dedup_and_keeps_original() {
        let tmp = std::env::temp_dir().join(format!("sm-vault-copy-{}", Uuid::new_v4()));
        std::fs::create_dir_all(tmp.join("vault")).unwrap();
        let state = test_state(&tmp);
        let vault = effective_vault_dir(&state.snapshot().ui.vault_path).unwrap();

        // 原始文件夹：含一个 SKILL.md 的技能目录。
        let origin = tmp.join("origin").join("demo");
        std::fs::create_dir_all(&origin).unwrap();
        std::fs::write(origin.join("SKILL.md"), "---\nname: demo\n---\nbody").unwrap();

        let (src, copied_vault) = install_dir_into_vault_core(&state, &origin, "").unwrap();
        assert_eq!(copied_vault, vault);
        assert_eq!(src.name, "中央仓库");
        assert!(vault.join("demo").join("SKILL.md").is_file());
        assert!(origin.join("SKILL.md").is_file(), "拷贝模式不能动原文件夹");

        // 同名再导一次：-2 避让，来源复用。
        let (src2, _) = install_dir_into_vault_core(&state, &origin, "").unwrap();
        assert_eq!(src2.id, src.id);
        assert!(vault.join("demo-2").join("SKILL.md").is_file());
        assert_eq!(state.inner.read().sources.len(), 1);

        std::fs::remove_dir_all(&tmp).unwrap();
    }
}
