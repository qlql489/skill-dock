//! 组合（skill collections）、批量安装/卸载、安装预览分类、标签管理。
//!
//! 本模块是"库组织"功能的命令层。设计要点：
//! - 所有批量操作走**单次后端调用**：一个 write lock 内完成全部 symlink
//!   增删，避免前端逐条 IPC 串行等待。
//! - 每一格（skill × target）的结果都带独立错误信息返回，UI 可以渲染
//!   完整的结果台账，而不是只有失败计数。
//! - 预览接口 [`preview_cells`] 把目标位分类成四种状态，供选择器在
//!   执行前把冲突摆到桌面上；执行接口 [`apply_cells`] 只处理预览里
//!   可行的格子。

use crate::state::{AppState, PersistedState, SkillGroup};
use crate::symlinks::{self, ConflictPolicy, Occupant};
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use tauri::State;
use uuid::Uuid;

// ---------------------------------------------------------------------------
// 组合 CRUD
// ---------------------------------------------------------------------------

#[tauri::command]
pub fn create_group(name: String, state: State<'_, Arc<AppState>>) -> Result<SkillGroup, String> {
    let name = name.trim().to_string();
    if name.is_empty() {
        return Err("组合名不能为空".into());
    }
    let group = {
        let mut guard = state.inner.write();
        if guard.groups.iter().any(|g| g.name == name) {
            return Err(format!("已存在同名组合「{name}」"));
        }
        let g = SkillGroup {
            id: Uuid::new_v4(),
            name,
            created_at: chrono::Utc::now(),
            skill_ids: Vec::new(),
        };
        guard.groups.push(g.clone());
        g
    };
    state.save().map_err(|e| e.to_string())?;
    Ok(group)
}

#[derive(Debug, Deserialize)]
pub struct GroupPatchRequest {
    pub id: String,
    pub name: Option<String>,
    /// 全量覆盖成员列表（保持传入顺序）。
    pub skill_ids: Option<Vec<String>>,
}

#[tauri::command]
pub fn update_group(req: GroupPatchRequest, state: State<'_, Arc<AppState>>) -> Result<SkillGroup, String> {
    let id = Uuid::parse_str(&req.id).map_err(|e| e.to_string())?;
    let group = {
        let mut guard = state.inner.write();
        if let Some(new_name) = &req.name {
            let new_name = new_name.trim().to_string();
            if new_name.is_empty() {
                return Err("组合名不能为空".into());
            }
            if guard.groups.iter().any(|g| g.name == new_name && g.id != id) {
                return Err(format!("已存在同名组合「{new_name}」"));
            }
        }
        let g = guard
            .groups
            .iter_mut()
            .find(|g| g.id == id)
            .ok_or_else(|| format!("group not found: {id}"))?;
        if let Some(n) = req.name {
            g.name = n.trim().to_string();
        }
        if let Some(ids) = req.skill_ids {
            g.skill_ids = ids;
        }
        g.clone()
    };
    state.save().map_err(|e| e.to_string())?;
    Ok(group)
}

/// 删除组合本身——不碰任何已装的软链。
#[tauri::command]
pub fn delete_group(id: String, state: State<'_, Arc<AppState>>) -> Result<(), String> {
    let id = Uuid::parse_str(&id).map_err(|e| e.to_string())?;
    {
        let mut guard = state.inner.write();
        let before = guard.groups.len();
        guard.groups.retain(|g| g.id != id);
        if guard.groups.len() == before {
            return Err(format!("group not found: {id}"));
        }
    }
    state.save().map_err(|e| e.to_string())?;
    Ok(())
}

// ---------------------------------------------------------------------------
// 按组合批量装 / 卸
// ---------------------------------------------------------------------------

/// 一格（skill × target）的失败详情。
#[derive(Debug, Serialize)]
pub struct CellFailure {
    pub skill_id: String,
    pub skill_name: String,
    pub target_id: String,
    pub target_name: String,
    pub error: String,
}

#[derive(Debug, Serialize)]
pub struct ApplyOutcome {
    pub installed: usize,
    pub removed: usize,
    pub skipped: usize,
    pub failures: Vec<CellFailure>,
}

#[derive(Debug, Deserialize)]
pub struct ApplyGroupRequest {
    pub group_id: String,
    pub target_ids: Vec<String>,
    /// true = 应用（缺的补装）；false = 停用（已装的移除）。
    pub activate: bool,
}

/// 组合级一键应用/停用。activate=true 时对每个勾选 target × 组合内每个
/// skill 补齐缺失的软链（幂等）；false 时反向拆除。行为模仿用户手动逐个
/// 操作的最终状态，但只花一次锁 + 一次 save。
#[tauri::command]
pub fn apply_group(
    req: ApplyGroupRequest,
    state: State<'_, Arc<AppState>>,
) -> Result<ApplyOutcome, String> {
    let gid = Uuid::parse_str(&req.group_id).map_err(|e| e.to_string())?;
    let outcome = {
        let mut guard = state.inner.write();
        let member_skills: Vec<crate::state::Skill> = {
            let group = guard.groups.iter().find(|g| g.id == gid).ok_or("group not found")?;
            group
                .skill_ids
                .iter()
                .filter_map(|sid| guard.skills.get(sid))
                .cloned()
                .collect()
        };
        let mut out = ApplyOutcome { installed: 0, removed: 0, skipped: 0, failures: Vec::new() };

        for tid in &req.target_ids {
            let tname = guard.targets.iter().find(|t| t.id == *tid).map(|t| t.name.clone()).unwrap_or_else(|| tid.clone());
            for skill in &member_skills {
                if req.activate {
                    // Already linked (healthy record)? Skip silently.
                    if guard.installations.iter().any(|i| {
                        i.skill_id == skill.id && i.target_id == *tid && i.status != "conflict"
                    }) {
                        out.skipped += 1;
                        continue;
                    }
                    match symlinks::install(&mut guard, skill, tid, ConflictPolicy::Fail) {
                        Ok(_) => out.installed += 1,
                        Err(e) => out.failures.push(CellFailure {
                            skill_id: skill.id.clone(),
                            skill_name: skill.name.clone(),
                            target_id: tid.clone(),
                            target_name: tname.clone(),
                            error: format!("{e:#}"),
                        }),
                    }
                } else {
                    let has = guard.installations.iter().any(|i| i.skill_id == skill.id && i.target_id == *tid);
                    if !has {
                        continue;
                    }
                    match symlinks::uninstall(&mut guard, &skill.id, tid) {
                        Ok(_) => out.removed += 1,
                        Err(e) => out.failures.push(CellFailure {
                            skill_id: skill.id.clone(),
                            skill_name: skill.name.clone(),
                            target_id: tid.clone(),
                            target_name: tname.clone(),
                            error: format!("{e:#}"),
                        }),
                    }
                }
            }
        }
        out
    };
    state.save().map_err(|e| e.to_string())?;
    Ok(outcome)
}

// ---------------------------------------------------------------------------
// 安装预览（四态分类）+ 逐格执行
// ---------------------------------------------------------------------------

/// 目标位状态。比"已装/未装"细：
/// - absent          空位，可直接装
/// - linked          已链接本 skill（点一下=卸载）
/// - occupied        名字被别人的软链占着（可显式替换）
/// - blocked         真实目录/文件占位，拒绝自动覆盖
/// - stale           我们自己的记录还在、但链接指向旧位置（会自动修复）
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum CellState {
    Absent,
    Linked,
    Stale,
    Occupied,
    Blocked,
}

#[derive(Debug, Serialize)]
pub struct PreviewCell {
    pub skill_id: String,
    pub skill_name: String,
    pub target_id: String,
    pub target_name: String,
    pub state: CellState,
    /// occupied 时：现链指向哪里；blocked 时：占用类型说明。
    pub detail: Option<String>,
}

fn classify_cell(guard: &PersistedState, skill: &crate::state::Skill, target: &crate::state::AgentTarget) -> (CellState, Option<String>) {
    if !target.enabled {
        return (CellState::Blocked, Some("agent 已停用".into()));
    }
    let owned = guard
        .installations
        .iter()
        .any(|i| i.skill_id == skill.id && i.target_id == target.id);
    let link_path = target.skills_dir.join(crate::symlinks::link_name_for(skill));
    match symlinks::classify(&link_path) {
        Occupant::Absent => (
            if owned { CellState::Stale } else { CellState::Absent },
            owned.then(|| "记录仍在但软链被删除".to_string()),
        ),
        Occupant::Symlink(dest) => {
            let points_here = dest
                .as_deref()
                .map(|d| d == skill.absolute_path || links_same(d, &skill.absolute_path))
                .unwrap_or(false);
            if points_here && owned {
                (CellState::Linked, None)
            } else if owned {
                (CellState::Stale, dest.map(|d| format!("当前指向 {}", d.display())))
            } else {
                (
                    CellState::Occupied,
                    Some(dest.map(|d| format!("软链 → {}", d.display())).unwrap_or_else(|| "未知软链".into())),
                )
            }
        }
        Occupant::RealDir => (CellState::Blocked, Some("真实目录占位".into())),
        Occupant::RealFile => (CellState::Blocked, Some("普通文件占位".into())),
    }
}

fn links_same(a: &std::path::Path, b: &std::path::Path) -> bool {
    if a == b {
        return true;
    }
    match (std::fs::canonicalize(a), std::fs::canonicalize(b)) {
        (Ok(ca), Ok(cb)) => ca == cb,
        _ => false,
    }
}

#[derive(Debug, Deserialize)]
pub struct PreviewRequest {
    pub skill_ids: Vec<String>,
    pub target_ids: Vec<String>,
}

/// 对 (skills × targets) 的每一格做无损分类，供选择器在提交前展示。
#[tauri::command]
pub fn preview_cells(req: PreviewRequest, state: State<'_, Arc<AppState>>) -> Result<Vec<PreviewCell>, String> {
    let guard = state.inner.read();
    let mut cells = Vec::new();
    for sid in &req.skill_ids {
        let Some(skill) = guard.skills.get(sid) else { continue };
        for tid in &req.target_ids {
            let Some(target) = guard.targets.iter().find(|t| t.id == *tid) else { continue };
            let (st, detail) = classify_cell(&guard, skill, target);
            cells.push(PreviewCell {
                skill_id: skill.id.clone(),
                skill_name: skill.name.clone(),
                target_id: target.id.clone(),
                target_name: target.name.clone(),
                state: st,
                detail,
            });
        }
    }
    Ok(cells)
}

#[derive(Debug, Deserialize)]
pub struct ApplyCellsRequest {
    /// 要链接的格子，["skillId","targetId"] 对。
    pub install: Vec<(String, String)>,
    /// 要卸载的格子，["skillId","targetId"] 对。
    pub remove: Vec<(String, String)>,
    /// 显式替换他人软链的格子（仅软链；真实目录依旧拒绝）。
    #[serde(default)]
    pub replace: Vec<(String, String)>,
}

#[derive(Debug, Serialize)]
pub struct BatchResult {
    pub installed: usize,
    pub removed: usize,
    pub failures: Vec<CellFailure>,
}

/// 选择器的统一执行入口：一次调用完成全部增删。真实目录占位永远拒绝，
/// 保护逻辑在后端 [`crate::symlinks`] 里，无法绕过；`replace` 分区允许
/// 顶掉"他人软链"（只删链接本身），对应预览里 occupied 且用户确认过的格
/// 子。
#[tauri::command]
pub fn apply_cells(req: ApplyCellsRequest, state: State<'_, Arc<AppState>>) -> Result<BatchResult, String> {
    let mut result = BatchResult { installed: 0, removed: 0, failures: Vec::new() };
    {
        let mut guard = state.inner.write();

        // ① 卸载分区。
        for (sid, tid) in req.remove {
            let tname = guard.targets.iter().find(|t| t.id == tid).map(|t| t.name.clone()).unwrap_or_else(|| tid.clone());
            let sname = guard.skills.get(&sid).map(|s| s.name.clone()).unwrap_or_else(|| sid.clone());
            match symlinks::uninstall(&mut guard, &sid, &tid) {
                Ok(_) => result.removed += 1,
                Err(e) => result.failures.push(CellFailure {
                    skill_id: sid,
                    skill_name: sname,
                    target_id: tid.clone(),
                    target_name: tname,
                    error: format!("{e:#}"),
                }),
            }
        }

        // 辅助：把 Skill/Target 从 guard 里取快照。
        let take = |g: &PersistedState, sid: &str, tid: &str| -> Option<(crate::state::Skill, crate::state::AgentTarget)> {
            let skill = g.skills.get(sid).cloned()?;
            let target = g.targets.iter().find(|t| t.id == tid).cloned()?;
            Some((skill, target))
        };

        // ② 常规安装分区（Fail 策略——预览里的 absent/stale 格子）。
        for (sid, tid) in req.install {
            let Some((skill, target)) = take(&guard, &sid, &tid) else { continue };
            match symlinks::install(&mut guard, &skill, &tid, ConflictPolicy::Fail) {
                Ok(_) => result.installed += 1,
                Err(e) => result.failures.push(CellFailure {
                    skill_id: sid,
                    skill_name: skill.name,
                    target_id: tid.clone(),
                    target_name: target.name,
                    error: format!("{e:#}"),
                }),
            }
        }

        // ③ 显式替换分区（Replace 策略——预览里 occupied 且用户勾选的格子）。
        for (sid, tid) in req.replace {
            let Some((skill, target)) = take(&guard, &sid, &tid) else { continue };
            match symlinks::install(&mut guard, &skill, &tid, ConflictPolicy::Replace) {
                Ok(_) => result.installed += 1,
                Err(e) => result.failures.push(CellFailure {
                    skill_id: sid,
                    skill_name: skill.name,
                    target_id: tid.clone(),
                    target_name: target.name,
                    error: format!("{e:#}"),
                }),
            }
        }
    } // release write lock before save
    state.save().map_err(|e| e.to_string())?;
    Ok(result)
}

// ---------------------------------------------------------------------------
// 导入选择：隐藏 / 恢复 skill（git 预览导入用）
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
pub struct HideSkillsRequest {
    pub source_id: String,
    pub hide_skill_paths: Vec<String>,
}

/// 把某个 source 里用户取消勾选的 skill 路径加入隐藏集（相对路径拼接成
/// 完整 id）。下一次 rescan 后这些 skill 不再出现在库中。
#[tauri::command]
pub fn hide_source_skills(req: HideSkillsRequest, state: State<'_, Arc<AppState>>) -> Result<(), String> {
    let sid = Uuid::parse_str(&req.source_id).map_err(|e| e.to_string())?;
    {
        let mut guard = state.inner.write();
        for rel in &req.hide_skill_paths {
            let full = format!("{sid}:{rel}");
            guard.hidden_skills.insert(full.clone());
            guard.skills.remove(&full);
        }
        // Drop installations referencing newly hidden skills.
        let hidden: Vec<String> = req.hide_skill_paths.iter().map(|r| format!("{sid}:{r}")).collect();
        guard.installations.retain(|i| !hidden.contains(&i.skill_id));
        let count = guard.skills.keys().filter(|k| k.starts_with(&format!("{sid}:"))).count();
        if let Some(s) = guard.sources.iter_mut().find(|s| s.id == sid) {
            s.skill_count = count;
        }
    }
    state.save().map_err(|e| e.to_string())?;
    Ok(())
}

/// 恢复某 source 的全部隐藏 skill（从隐藏集移除并立即重扫该源由前端触发）。
#[tauri::command]
pub fn unhide_source_skills(source_id: String, state: State<'_, Arc<AppState>>) -> Result<(), String> {
    let sid = Uuid::parse_str(&source_id).map_err(|e| e.to_string())?;
    {
        let mut guard = state.inner.write();
        let prefix = format!("{sid}:");
        guard.hidden_skills.retain(|h| !h.starts_with(&prefix));
    }
    state.save().map_err(|e| e.to_string())?;
    Ok(())
}

// ---------------------------------------------------------------------------
// 待办概览
// ---------------------------------------------------------------------------

#[derive(Debug, Serialize)]
pub struct PendingEntry {
    pub skill_id: String,
    pub skill_name: String,
    pub target_id: Option<String>,
    pub target_name: Option<String>,
    pub detail: String,
}

#[derive(Debug, Serialize)]
pub struct PendingReport {
    /// 从未安装到任何启用目标的 skill。
    pub uninstalled: Vec<PendingEntry>,
    /// 失效软链（missing）/ 记录冲突（conflict）。
    pub broken_links: Vec<PendingEntry>,
    /// 有远端更新未应用的 github 来源。
    pub outdated_sources: Vec<String>,
    /// 启用了但本机没检测到的 agent。
    pub undetected_targets: Vec<String>,
}

/// 汇总所有"等待用户处理"的事项。纯读操作，基于当前快照 +
/// 最近一次后台更新报告（如果有）。
#[tauri::command]
pub fn pending_items(state: State<'_, Arc<AppState>>) -> Result<PendingReport, String> {
    let snap = state.snapshot();
    let enabled_targets: Vec<&crate::state::AgentTarget> =
        snap.targets.iter().filter(|t| t.enabled).collect();

    let mut report = PendingReport {
        uninstalled: Vec::new(),
        broken_links: Vec::new(),
        outdated_sources: Vec::new(),
        undetected_targets: Vec::new(),
    };

    for skill in snap.skills.values() {
        let installed_to = snap
            .installations
            .iter()
            .filter(|i| i.skill_id == skill.id)
            .count();
        if installed_to == 0 {
            report.uninstalled.push(PendingEntry {
                skill_id: skill.id.clone(),
                skill_name: skill.name.clone(),
                target_id: None,
                target_name: None,
                detail: "尚未安装到任何 agent".into(),
            });
        }
    }

    for inst in &snap.installations {
        if inst.status == "ok" || inst.status.is_empty() {
            continue;
        }
        let sname = snap.skills.get(&inst.skill_id).map(|s| s.name.clone()).unwrap_or_else(|| inst.skill_id.clone());
        let tname = snap.targets.iter().find(|t| t.id == inst.target_id).map(|t| t.name.clone()).unwrap_or_else(|| inst.target_id.clone());
        report.broken_links.push(PendingEntry {
            skill_id: inst.skill_id.clone(),
            skill_name: sname,
            target_id: Some(inst.target_id.clone()),
            target_name: Some(tname),
            detail: if inst.status == "missing" {
                format!("软链丢失：{}", inst.link_path.display())
            } else {
                format!("位置被其他内容占用：{}", inst.link_path.display())
            },
        });
    }

    // 有可用更新的来源来自缓存的后台检查报告。
    let cached = crate::updater::take_last_report();
    for u in &cached.updates {
        report.outdated_sources.push(u.source_name.clone());
    }

    for t in &enabled_targets {
        if !t.detected {
            report.undetected_targets.push(t.name.clone());
        }
    }

    Ok(report)
}

/// 清掉指定状态的失效安装记录（概览页"清理"按钮）。
#[tauri::command]
pub fn cleanup_installations(statuses: Vec<String>, state: State<'_, Arc<AppState>>) -> Result<usize, String> {
    let refs: Vec<&str> = statuses.iter().map(|s| s.as_str()).collect();
    let n = {
        let mut guard = state.inner.write();
        symlinks::drop_statuses(&mut guard, &refs)
    };
    if n > 0 {
        state.save().map_err(|e| e.to_string())?;
    }
    Ok(n)
}

// ---------------------------------------------------------------------------
// 真实删除：卸载全部软链 + 删除磁盘目录
// ---------------------------------------------------------------------------

/// 把目录移入 macOS 废纸篓（可恢复）。跨卷移动失败时退回直接删除 ——
/// 调用前必须已有用户确认。`trash_root` 供测试注入，生产传 None 用 ~/.Trash。
pub(crate) fn trash_dir(dir: &std::path::Path) -> Result<(), String> {
    trash_or_remove(dir)
}

fn trash_or_remove(dir: &std::path::Path) -> Result<(), String> {
    let home_trash = dirs::home_dir().map(|h| h.join(".Trash"));
    trash_or_remove_into(dir, home_trash.as_deref())
}

fn trash_or_remove_into(dir: &std::path::Path, trash_root: Option<&std::path::Path>) -> Result<(), String> {
    if !dir.exists() {
        return Ok(());
    }
    if let Some(trash) = trash_root {
        let _ = std::fs::create_dir_all(trash);
        let name = dir
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_else(|| "skill".to_string());
        let dest = trash.join(format!("{}-{}", name, uuid::Uuid::new_v4().simple()));
        match std::fs::rename(dir, &dest) {
            Ok(()) => {
                log::info!("deleted skill moved to Trash: {}", dest.display());
                return Ok(());
            }
            Err(e) => log::warn!("rename to Trash failed ({}), falling back to remove_dir_all", e),
        }
    }
    std::fs::remove_dir_all(dir).map_err(|e| format!("删除目录失败：{e}"))
}

/// 彻底删除一个 skill：先卸掉它在所有 agent 上的软链，再把源目录
/// 移入废纸篓（跨卷失败则直接删除），并从库中移除。返回移除的软链数。
#[tauri::command]
pub fn delete_skill_forever(req: crate::commands::DeleteSkillRequest, state: State<'_, Arc<AppState>>) -> Result<usize, String> {
    let (skill, removed_links) = {
        let mut guard = state.inner.write();
        let skill = guard
            .skills
            .get(&req.skill_id)
            .cloned()
            .ok_or_else(|| format!("skill not found: {}", req.skill_id))?;
        let target_ids: Vec<String> = guard
            .installations
            .iter()
            .filter(|i| i.skill_id == req.skill_id)
            .map(|i| i.target_id.clone())
            .collect();
        let mut removed = 0usize;
        for tid in target_ids {
            if symlinks::uninstall(&mut guard, &req.skill_id, &tid).is_ok() {
                removed += 1;
            }
        }
        // 彻底删除后目录已进废纸篓，重扫不会再出现；要把可能残留的隐藏记录
        // 清掉而不是再塞墓碑——否则将来同名路径重新入库会被静默隐藏。
        guard.hidden_skills.remove(&req.skill_id);
        guard.skills.remove(&req.skill_id);
        // 组合成员里残留的引用：组合应用时本来就按存在性过滤，这里顺手清掉。
        for g in guard.groups.iter_mut() {
            g.skill_ids.retain(|id| id != &req.skill_id);
        }
        (skill, removed)
    }; // release write lock before FS ops / save

    if skill.absolute_path.is_dir() {
        trash_or_remove(&skill.absolute_path)?;
    }
    state.save().map_err(|e| e.to_string())?;
    Ok(removed_links)
}

// ---------------------------------------------------------------------------
// UI 偏好（主题色等）
// ---------------------------------------------------------------------------

fn is_hex_color(s: &str) -> bool {
    s.len() == 7
        && s.starts_with('#')
        && s[1..].chars().all(|c| c.is_ascii_hexdigit())
}

#[tauri::command]
pub fn get_accent_color(state: State<'_, Arc<AppState>>) -> Result<String, String> {
    Ok(state.snapshot().ui.accent_color)
}

#[tauri::command]
pub fn set_accent_color(color: String, state: State<'_, Arc<AppState>>) -> Result<(), String> {
    let c = color.trim().to_string();
    if !c.is_empty() && !is_hex_color(&c) {
        return Err(format!("非法颜色值：{c}（需要 #rrggbb）"));
    }
    {
        let mut guard = state.inner.write();
        guard.ui.accent_color = c;
    }
    state.save().map_err(|e| e.to_string())?;
    Ok(())
}

#[tauri::command]
pub fn get_add_default_install_targets(state: State<'_, Arc<AppState>>) -> Result<bool, String> {
    Ok(state.snapshot().ui.add_default_install_targets)
}

#[tauri::command]
pub fn set_add_default_install_targets(
    enabled: bool,
    state: State<'_, Arc<AppState>>,
) -> Result<(), String> {
    {
        let mut guard = state.inner.write();
        guard.ui.add_default_install_targets = enabled;
    }
    state.save().map_err(|e| e.to_string())?;
    Ok(())
}

// ---------------------------------------------------------------------------
// 目标排序
// ---------------------------------------------------------------------------

/// 按传入的 id 顺序重排 targets（其它页面里 agent 的展示顺序随之统一）。
/// 未列出的 id 保持在尾部原相对顺序；不存在的 id 忽略。
#[tauri::command]
pub fn reorder_targets(ordered_ids: Vec<String>, state: State<'_, Arc<AppState>>) -> Result<(), String> {
    {
        let mut guard = state.inner.write();
        let mut remaining: Vec<crate::state::AgentTarget> = guard.targets.drain(..).collect();
        let mut next = Vec::with_capacity(remaining.len());
        for id in &ordered_ids {
            if let Some(pos) = remaining.iter().position(|t| &t.id == id) {
                next.push(remaining.remove(pos));
            }
        }
        // 未在列表中的按原相对顺序补到后面。
        next.extend(remaining);
        guard.targets = next;
    }
    state.save().map_err(|e| e.to_string())?;
    Ok(())
}

// ---------------------------------------------------------------------------
// 按路径反查库内 skill（Agent 页点击跳转到 Skill 详情页用）
// ---------------------------------------------------------------------------

#[derive(Debug, Serialize)]
pub struct SkillMatch {
    pub id: String,
    pub name: String,
}

/// 给定磁盘上的一个目录（agent 里软链的指向，或条目本身路径），找出它对应
/// 的库内 skill。先做字面量比对，再做 canonicalize 比对（大小写不敏感卷、
/// `..`、别名路径都能命中）。找不到返回 None —— 调用方自行回退。
#[tauri::command]
pub fn resolve_skill_by_path(path: String, state: State<'_, Arc<AppState>>) -> Result<Option<SkillMatch>, String> {
    let target = std::path::PathBuf::from(&path);
    if !target.exists() {
        return Ok(None);
    }
    let canon = target.canonicalize().ok();
    let guard = state.inner.read();
    for s in guard.skills.values() {
        if s.absolute_path == target {
            return Ok(Some(SkillMatch { id: s.id.clone(), name: s.name.clone() }));
        }
    }
    if let Some(c) = &canon {
        for s in guard.skills.values() {
            if let Ok(sc) = s.absolute_path.canonicalize() {
                if &sc == c {
                    return Ok(Some(SkillMatch { id: s.id.clone(), name: s.name.clone() }));
                }
            }
        }
    }
    Ok(None)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmpdir(tag: &str) -> std::path::PathBuf {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let p = std::env::temp_dir().join(format!("sm-groups-{tag}-{}-{nanos}", std::process::id()));
        std::fs::create_dir_all(&p).unwrap();
        p
    }

    #[test]
    fn trash_moves_dir_into_trash_root() {
        let root = tmpdir("trash");
        let trash = root.join("trash-bin");
        let skill_dir = root.join("my-skill");
        std::fs::create_dir_all(&skill_dir).unwrap();
        std::fs::write(skill_dir.join("SKILL.md"), "x").unwrap();

        trash_or_remove_into(&skill_dir, Some(&trash)).unwrap();
        assert!(!skill_dir.exists(), "原目录应已不在");
        assert_eq!(std::fs::read_dir(&trash).unwrap().count(), 1, "废纸篓里应有一项");
        std::fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn missing_dir_is_a_noop() {
        let root = tmpdir("noop");
        trash_or_remove_into(&root.join("nonexistent"), Some(&root.join("t"))).unwrap();
        std::fs::remove_dir_all(&root).unwrap();
    }
}
