//! 项目级 Skill 管理 —— 以项目为中心的工作区（对标 skills-manager 的
//! 项目工作区模型）。
//!
//! 一个「项目」= 用户注册的项目根目录（如某个 git 仓库）。项目里各 agent
//! 的项目级 skills 目录（如 `<project>/.claude/skills`）中的技能可以被：
//! - 扫描盘点：与中央技能库匹配，判定五态同步状态
//!   （一致 / 项目较新 / 中央较新 / 内容分叉 / 仅项目）；
//! - 禁用/启用：移动到同级 `<skills>-disabled` 目录（rename，非元数据）；
//! - 纳入管理：复制进中央仓库（默认应用数据目录下的 `skills`），成为库内技能；
//! - 安装：把库内技能导出为项目内副本或软链；
//! - 更新：中央较新时把库内副本拉回项目（项目较新时拒绝，防覆盖编辑）。
//!
//! 技能实况永远以磁盘扫描为准，state.json 里的 `projects` 只存注册元数据
//! （对标竞品：扫描结果不落库）。安全模型沿用全局侧：真实目录永不被静默
//! 覆盖，删除走废纸篓，所有用户输入的相对路径过 Normal-component 校验。

use crate::state::{AgentTarget, AppState, CloneStatus, PersistedState, Project, Skill, Source, SourceKind, SourceMode};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashSet};
use std::path::{Component, Path, PathBuf};
use std::sync::Arc;
use tauri::{AppHandle, Emitter, State};
use uuid::Uuid;

// ---------------------------------------------------------------------------
// Agent 槽位：项目内相对 skills 目录
// ---------------------------------------------------------------------------

/// 一个「agent 槽位」= 项目内某个相对 skills 目录（如 `.claude/skills`）。
/// 多个 agent 共用同一相对目录时合并为一个槽位（display 用 " / " 连接），
/// 命令层用 `dir` 作为 agent 标识。
#[derive(Debug, Clone, Serialize)]
pub struct AgentSlot {
    /// 项目内相对 skills 目录，如 ".claude/skills"。
    pub dir: String,
    /// 展示名，多 agent 合并时用 " / " 连接。
    pub display: String,
}

/// 项目级目录与用户级不同的 agent：显式映射（对标竞品适配器表）。
/// 未命中的按「去掉 home 前缀」推导（~/.claude/skills → .claude/skills）。
const PROJECT_DIR_OVERRIDES: &[(&str, &str)] = &[
    ("pi", ".pi/skills"),
    ("opencode", ".opencode/skills"),
];

/// 从 target 目录推导项目级相对 skills 目录。只收「最后一段叫 skills」的
/// 目录（~/.minimax/agents 这类 agent 根目录不参与项目映射）。
fn project_relative_dir(t: &AgentTarget, home: &Path) -> Option<String> {
    if let Some((_, rel)) = PROJECT_DIR_OVERRIDES.iter().find(|(id, _)| *id == t.id) {
        return Some(rel.to_string());
    }
    let rel = t.skills_dir.strip_prefix(home).ok()?;
    let last = rel.components().next_back()?;
    if last.as_os_str() != "skills" {
        return None;
    }
    Some(rel.to_string_lossy().replace('\\', "/"))
}

pub(crate) fn agent_slots(targets: &[AgentTarget]) -> Vec<AgentSlot> {
    let Some(home) = dirs::home_dir() else { return Vec::new() };
    let mut merged: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for t in targets {
        if let Some(rel) = project_relative_dir(t, &home) {
            merged.entry(rel).or_default().push(t.name.clone());
        }
    }
    merged
        .into_iter()
        .map(|(dir, names)| AgentSlot { dir, display: names.join(" / ") })
        .collect()
}

// ---------------------------------------------------------------------------
// 扫描
// ---------------------------------------------------------------------------

/// 项目内一个技能实例的扫描结果（含同步判定所需字段）。
#[derive(Debug, Clone)]
struct ScannedSkill {
    /// SKILL.md frontmatter name（空则用目录名）。
    name: String,
    /// 技能目录名（匹配兜底用）。
    dir_name: String,
    /// 相对 skills 根的路径，"/" 分隔（支持嵌套布局 category/sub/skill）。
    relative_path: String,
    abs: PathBuf,
    /// SKILL.md 的 blake3 —— 与库内 Skill.content_hash 同一口径。
    hash: String,
    /// SKILL.md 的 mtime（毫秒）。与库内 modified_at 同源，五态判定用。
    mtime_ms: Option<i64>,
    /// 技能目录本身是软链（如手工链到中央仓库）。
    is_symlink: bool,
}

const PROJECT_SCAN_DEPTH: usize = 4;

/// 扫描一个 skills 根（启用根或 -disabled 根），返回其中的技能实例。
/// 算法与 scanner.rs 同构：含 SKILL.md 的目录是技能（叶子，不再下钻）；
/// 不含的目录递归（点开头 / 构建缓存目录跳过）；visited 集合防软链环。
fn scan_skills_root(root: &Path) -> Vec<ScannedSkill> {
    let mut out = Vec::new();
    if !root.is_dir() {
        return out;
    }
    let mut visited: HashSet<PathBuf> = HashSet::new();
    if let Ok(real) = std::fs::canonicalize(root) {
        visited.insert(real);
    }
    scan_into(root, root, 0, &mut visited, &mut out);
    out.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()));
    out
}

fn scan_into(root: &Path, dir: &Path, depth: usize, visited: &mut HashSet<PathBuf>, out: &mut Vec<ScannedSkill>) {
    if depth > PROJECT_SCAN_DEPTH {
        return;
    }
    let skill_md = dir.join("SKILL.md");
    if skill_md.is_file() {
        let (fm_name, _) = crate::scanner::parse_frontmatter(&skill_md);
        let dir_name = dir
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_default();
        let is_symlink = std::fs::symlink_metadata(dir)
            .map(|m| m.file_type().is_symlink())
            .unwrap_or(false);
        let hash = blake3::hash(&std::fs::read(&skill_md).unwrap_or_default()).to_hex().to_string();
        let mtime_ms = std::fs::metadata(&skill_md)
            .ok()
            .and_then(|m| m.modified().ok())
            .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
            .map(|d| d.as_millis() as i64);
        let relative_path = dir
            .strip_prefix(root)
            .unwrap_or(dir)
            .to_string_lossy()
            .replace('\\', "/");
        out.push(ScannedSkill {
            name: if fm_name.is_empty() { dir_name.clone() } else { fm_name },
            dir_name,
            relative_path,
            abs: dir.to_path_buf(),
            hash,
            mtime_ms,
            is_symlink,
        });
        return; // 技能是叶子，不再下钻
    }

    let entries = match std::fs::read_dir(dir) {
        Ok(e) => e,
        Err(_) => return,
    };
    for entry in entries.flatten() {
        let name = entry.file_name();
        let name_str = name.to_string_lossy().to_string();
        if name_str.starts_with('.') || crate::scanner::should_skip_dir(&name_str) {
            continue;
        }
        let child = entry.path();
        // metadata() 跟随软链：链到目录的条目照常下钻；断链跳过。
        let Ok(meta) = std::fs::metadata(&child) else { continue };
        if !meta.is_dir() {
            continue;
        }
        let Ok(real) = std::fs::canonicalize(&child) else { continue };
        if !visited.insert(real) {
            continue; // 软链环
        }
        scan_into(root, &child, depth + 1, visited, out);
    }
}

// ---------------------------------------------------------------------------
// 中央库匹配 + 五态同步判定
// ---------------------------------------------------------------------------

/// 匹配打分：canonical 路径相等（3）> 名称相等（2）> 内容哈希相等（1），
/// 取最高分。路径相等意味着「就是同一份文件」（软链指进库/项目本身被
/// 注册为来源）；名称相等是身份匹配；哈希兜底识别改名后的同内容技能。
fn best_center_match(snap: &PersistedState, proj: &ScannedSkill, canon_cache: &mut BTreeMap<PathBuf, PathBuf>) -> Option<(u8, Skill)> {
    let mut canon_of = |p: &Path| -> PathBuf {
        canon_cache
            .entry(p.to_path_buf())
            .or_insert_with(|| std::fs::canonicalize(p).unwrap_or_else(|_| p.to_path_buf()))
            .clone()
    };
    let proj_canon = canon_of(&proj.abs);
    let proj_key = proj.name.to_lowercase();
    let dir_key = proj.dir_name.to_lowercase();

    let mut best: Option<(u8, Skill)> = None;
    for s in snap.skills.values() {
        let mut score = 0u8;
        if canon_of(&s.absolute_path) == proj_canon {
            score = 3;
        } else {
            let skey = s.name.to_lowercase();
            if skey == proj_key || skey == dir_key {
                score = 2;
            } else if !s.content_hash.is_empty() && s.content_hash == proj.hash {
                score = 1;
            }
        }
        if score > 0 && best.as_ref().map_or(true, |(bs, _)| score > *bs) {
            best = Some((score, s.clone()));
        }
    }
    best
}

/// 五态：一致 / 项目较新 / 中央较新 / 分叉 / 仅项目。
/// 判定顺序：路径命中（3 分）或哈希命中（1 分）→ 一致（同一份内容）；
/// 名称命中（2 分）→ 哈希相等即一致，否则比两侧 SKILL.md 的磁盘 mtime
/// （1 秒容差，都在容差内 = 分叉）。绝不拿数据库时间戳当「中央时间」——
/// 库内 modified_at 与项目侧一样取自 SKILL.md 的 mtime，两边同尺。
fn classify_status(matched: Option<&(u8, Skill)>, proj: &ScannedSkill) -> String {
    let Some((score, skill)) = matched else {
        return "project_only".into();
    };
    if *score != 2 {
        return "in_sync".into();
    }
    if !proj.hash.is_empty() && skill.content_hash == proj.hash {
        return "in_sync".into();
    }
    let c_ms = skill.modified_at.timestamp_millis();
    let Some(p_ms) = proj.mtime_ms else {
        return "diverged".into();
    };
    const TOLERANCE_MS: i64 = 1_000;
    if p_ms > c_ms + TOLERANCE_MS {
        "project_newer".into()
    } else if c_ms > p_ms + TOLERANCE_MS {
        "center_newer".into()
    } else {
        "diverged".into()
    }
}

// ---------------------------------------------------------------------------
// 对外 DTO + 查询命令
// ---------------------------------------------------------------------------

/// 同步健康度汇总（项目列表卡片用）。
#[derive(Debug, Default, Serialize)]
pub struct SyncHealth {
    pub in_sync: usize,
    pub project_newer: usize,
    pub center_newer: usize,
    pub diverged: usize,
    pub project_only: usize,
}

#[derive(Debug, Serialize)]
pub struct ProjectOverview {
    pub id: String,
    pub name: String,
    pub path: String,
    pub skill_count: usize,
    pub health: SyncHealth,
}

#[derive(Debug, Serialize)]
pub struct ProjectSkillInfo {
    pub name: String,
    pub description: String,
    /// 相对 skills 根的路径，"/" 分隔。
    pub relative_path: String,
    /// 所属槽位（agent 标识），如 ".claude/skills"。
    pub agent_dir: String,
    /// 槽位展示名（多 agent 合并时含 " / "）。
    pub agent_names: String,
    /// 绝对路径。
    pub path: String,
    pub enabled: bool,
    pub is_symlink: bool,
    pub sync_status: String,
    pub center_skill_id: Option<String>,
    pub center_source_name: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct ProjectSlotInfo {
    pub dir: String,
    pub display: String,
    /// 项目内该 skills 目录当前是否存在。
    pub exists: bool,
}

fn skill_desc(skill_dir: &Path) -> String {
    let (_, desc) = crate::scanner::parse_frontmatter(&skill_dir.join("SKILL.md"));
    desc
}

fn find_project(snap: &PersistedState, id: &str) -> Result<Project, String> {
    let uuid = Uuid::parse_str(id).map_err(|_| "项目 id 无效".to_string())?;
    snap.projects
        .iter()
        .find(|p| p.id == uuid)
        .cloned()
        .ok_or_else(|| "项目不存在（可能已被删除）".to_string())
}

/// 对一个项目做全量扫描 + 分类。纯函数，调用方先 snapshot、锁外执行。
fn scan_project(snap: &PersistedState, project: &Project) -> (Vec<ProjectSkillInfo>, SyncHealth) {
    let mut canon_cache: BTreeMap<PathBuf, PathBuf> = BTreeMap::new();
    let mut infos = Vec::new();
    let mut health = SyncHealth::default();

    for slot in agent_slots(&snap.targets) {
        let enabled_root = project.path.join(&slot.dir);
        let disabled_root = project.path.join(format!("{}-disabled", slot.dir));
        for (root, enabled) in [(&enabled_root, true), (&disabled_root, false)] {
            for sk in scan_skills_root(root) {
                let matched = best_center_match(snap, &sk, &mut canon_cache);
                let status = classify_status(matched.as_ref(), &sk);
                match status.as_str() {
                    "in_sync" => health.in_sync += 1,
                    "project_newer" => health.project_newer += 1,
                    "center_newer" => health.center_newer += 1,
                    "diverged" => health.diverged += 1,
                    _ => health.project_only += 1,
                }
                infos.push(ProjectSkillInfo {
                    description: skill_desc(&sk.abs),
                    name: sk.name.clone(),
                    relative_path: sk.relative_path.clone(),
                    agent_dir: slot.dir.clone(),
                    agent_names: slot.display.clone(),
                    path: sk.abs.to_string_lossy().to_string(),
                    enabled,
                    is_symlink: sk.is_symlink,
                    sync_status: status,
                    center_skill_id: matched.as_ref().map(|(_, s)| s.id.clone()),
                    center_source_name: matched.as_ref().and_then(|(_, s)| {
                        snap.sources.iter().find(|x| x.id == s.source_id).map(|x| x.name.clone())
                    }),
                });
            }
        }
    }
    infos.sort_by(|a, b| {
        a.name.to_lowercase().cmp(&b.name.to_lowercase()).then_with(|| a.agent_dir.cmp(&b.agent_dir))
    });
    (infos, health)
}

/// 项目内可用的 agent 槽位（含目录存在性，供安装弹窗渲染）。
#[tauri::command]
pub fn get_project_slots(state: State<'_, Arc<AppState>>, project_id: String) -> Result<Vec<ProjectSlotInfo>, String> {
    let snap = state.snapshot();
    let project = find_project(&snap, &project_id)?;
    Ok(agent_slots(&snap.targets)
        .into_iter()
        .map(|s| {
            let exists = project.path.join(&s.dir).is_dir();
            ProjectSlotInfo { dir: s.dir, display: s.display, exists }
        })
        .collect())
}

/// 项目列表（含每个项目的技能数与五态汇总）。扫描在阻塞线程池执行。
#[tauri::command]
pub async fn get_projects(state: State<'_, Arc<AppState>>) -> Result<Vec<ProjectOverview>, String> {
    let snap = state.snapshot();
    tauri::async_runtime::spawn_blocking(move || {
        let mut out = Vec::new();
        for p in snap.projects.iter() {
            let (_, health) = scan_project(&snap, p);
            out.push(ProjectOverview {
                id: p.id.to_string(),
                name: p.name.clone(),
                path: p.path.to_string_lossy().to_string(),
                skill_count: health.in_sync + health.project_newer + health.center_newer + health.diverged + health.project_only,
                health,
            });
        }
        Ok(out)
    })
    .await
    .map_err(|e| format!("join error: {e}"))?
}

/// 某个项目的技能清单（含五态、槽位、启停状态）。
#[tauri::command]
pub async fn get_project_skills(state: State<'_, Arc<AppState>>, project_id: String) -> Result<Vec<ProjectSkillInfo>, String> {
    let snap = state.snapshot();
    let project = find_project(&snap, &project_id)?;
    tauri::async_runtime::spawn_blocking(move || {
        let (infos, _) = scan_project(&snap, &project);
        Ok(infos)
    })
    .await
    .map_err(|e| format!("join error: {e}"))?
}

#[derive(Debug, Serialize)]
pub struct ProjectDoc {
    pub name: String,
    pub description: String,
    pub content: String,
}

/// 读取项目技能的 SKILL.md 全文。软链白名单：解析目标必须仍在项目根内
/// （防技能里放一条指向 ~/.ssh 之类的链接被读出）。
#[tauri::command]
pub fn get_project_skill_doc(state: State<'_, Arc<AppState>>, project_id: String, agent_dir: String, relative_path: String) -> Result<ProjectDoc, String> {
    let snap = state.snapshot();
    let project = find_project(&snap, &project_id)?;
    ensure_safe_skill_relative_path(&relative_path)?;
    let path = locate_skill_instance(&project, &agent_dir, &relative_path)?;

    let md = path.join("SKILL.md");
    if let Ok(resolved) = std::fs::canonicalize(&md) {
        let root_canon = std::fs::canonicalize(&project.path).unwrap_or_else(|_| project.path.clone());
        if !resolved.starts_with(&root_canon) {
            return Err(format!("SKILL.md 是指向项目外（{}）的软链，拒绝读取", resolved.display()));
        }
    }
    let (name, description) = crate::scanner::parse_frontmatter(&md);
    let content = std::fs::read_to_string(&md).map_err(|e| format!("读取失败：{e}"))?;
    Ok(ProjectDoc { name, description, content })
}

// ---------------------------------------------------------------------------
// 注册 / 移除项目 + 项目发现
// ---------------------------------------------------------------------------

fn expand_path(raw: &str) -> Result<PathBuf, String> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Err("路径不能为空".into());
    }
    if trimmed == "~" {
        return dirs::home_dir().ok_or_else(|| "无法确定主目录".into());
    }
    if let Some(rest) = trimmed.strip_prefix("~/") {
        let home = dirs::home_dir().ok_or_else(|| "无法确定主目录".to_string())?;
        return Ok(home.join(rest.trim_start_matches('/')));
    }
    Ok(PathBuf::from(trimmed))
}

#[tauri::command]
pub fn add_project(path: String, state: State<'_, Arc<AppState>>) -> Result<Project, String> {
    let p = expand_path(&path)?;
    if !p.is_dir() {
        return Err(format!("目录不存在：{}", p.display()));
    }
    let canonical = std::fs::canonicalize(&p).unwrap_or_else(|_| p.clone());
    let name = canonical
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .ok_or_else(|| "无法解析目录名".to_string())?;

    let project = {
        let mut guard = state.inner.write();
        // 同一目录（canonical 后）不允许重复注册。
        if guard.projects.iter().any(|x| {
            std::fs::canonicalize(&x.path).unwrap_or_else(|_| x.path.clone()) == canonical
        }) {
            return Err("该项目已存在".into());
        }
        let max_order = guard.projects.iter().map(|x| x.sort_order).max().unwrap_or(0);
        let project = Project {
            id: Uuid::new_v4(),
            name,
            path: canonical,
            created_at: chrono::Utc::now(),
            sort_order: max_order + 1,
        };
        guard.projects.push(project.clone());
        project
    }; // release write lock before save
    state.save().map_err(|e| e.to_string())?;
    log::info!("projects: added {} ({})", project.name, project.path.display());
    Ok(project)
}

/// 只删注册记录，不动磁盘。
#[tauri::command]
pub fn remove_project(id: String, state: State<'_, Arc<AppState>>) -> Result<(), String> {
    let uuid = Uuid::parse_str(&id).map_err(|_| "项目 id 无效".to_string())?;
    {
        let mut guard = state.inner.write();
        let before = guard.projects.len();
        guard.projects.retain(|p| p.id != uuid);
        if guard.projects.len() == before {
            return Err("项目不存在".into());
        }
    }
    state.save().map_err(|e| e.to_string())?;
    Ok(())
}

/// 在一个根目录下深度 ≤4 找「含任一 agent 项目级 skills 目录」的项目候选。
/// 命中即停（不再深入该项目子树，防嵌套项目重复）；已注册的项目也返回，
/// 由前端标灰去重。
#[tauri::command]
pub async fn scan_project_roots(state: State<'_, Arc<AppState>>, root: String, depth: Option<u8>) -> Result<Vec<String>, String> {
    let snap = state.snapshot();
    let root = expand_path(&root)?;
    if !root.is_dir() {
        return Err(format!("目录不存在：{}", root.display()));
    }
    let max_depth = depth.unwrap_or(4).min(6) as usize;
    tauri::async_runtime::spawn_blocking(move || {
        let slot_dirs: Vec<String> = agent_slots(&snap.targets).into_iter().map(|s| s.dir).collect();
        let mut found = Vec::new();
        find_project_roots(&root, &slot_dirs, 0, max_depth, &mut found);
        found.sort();
        Ok(found
            .into_iter()
            .map(|p| p.to_string_lossy().to_string())
            .collect())
    })
    .await
    .map_err(|e| format!("join error: {e}"))?
}

fn find_project_roots(dir: &Path, slot_dirs: &[String], depth: usize, max_depth: usize, out: &mut Vec<PathBuf>) {
    if depth > max_depth {
        return;
    }
    // 本目录命中任一槽位目录 → 是项目，收录并停止下钻。
    if slot_dirs.iter().any(|s| dir.join(s).is_dir()) {
        out.push(dir.to_path_buf());
        return;
    }
    let entries = match std::fs::read_dir(dir) {
        Ok(e) => e,
        Err(_) => return,
    };
    for entry in entries.flatten() {
        let name = entry.file_name();
        let name_str = name.to_string_lossy().to_string();
        if name_str.starts_with('.') || crate::scanner::should_skip_dir(&name_str) {
            continue;
        }
        let child = entry.path();
        if child.is_dir() {
            find_project_roots(&child, slot_dirs, depth + 1, max_depth, out);
        }
    }
}

// ---------------------------------------------------------------------------
// 相对路径安全校验 + 实例定位
// ---------------------------------------------------------------------------

/// 用户输入的技能相对路径：每个 component 必须是 Normal（一次性拒绝
/// `..`、绝对路径、`.` 等）。
fn ensure_safe_skill_relative_path(rel: &str) -> Result<(), String> {
    let p = Path::new(rel);
    if p.as_os_str().is_empty() {
        return Err("技能路径不能为空".into());
    }
    let mut normal = 0;
    for comp in p.components() {
        match comp {
            Component::Normal(_) => normal += 1,
            _ => return Err(format!("非法的技能路径：{rel}")),
        }
    }
    if normal == 0 {
        return Err(format!("非法的技能路径：{rel}"));
    }
    Ok(())
}

/// 词法前缀校验（第一道）：path 必须落在 root 内（软链解析前）。
fn ensure_dir_within_root(path: &Path, root: &Path) -> Result<(), String> {
    let abs_path = if path.is_absolute() { path.to_path_buf() } else { std::env::current_dir().map_err(|e| e.to_string())?.join(path) };
    let abs_root = if root.is_absolute() { root.to_path_buf() } else { std::env::current_dir().map_err(|e| e.to_string())?.join(root) };
    if !abs_path.starts_with(&abs_root) {
        return Err(format!("路径越界：{} 不在 {} 内", path.display(), root.display()));
    }
    Ok(())
}

/// skills 根 + disabled 根（disabled = `<skills>-disabled` 同级兄弟目录）。
fn slot_roots(project: &Project, agent_dir: &str) -> Result<(PathBuf, PathBuf), String> {
    if agent_dir.is_empty() || agent_dir.contains("..") || agent_dir.starts_with('/') {
        return Err(format!("非法的 agent 目录：{agent_dir}"));
    }
    Ok((
        project.path.join(agent_dir),
        project.path.join(format!("{}-disabled", agent_dir)),
    ))
}

/// 按 (agent_dir, relative_path) 找到技能实例的实际落点（先启用侧后
/// 禁用侧），并校验其落在对应根内。返回 (路径, 是否启用侧)。
fn locate_skill_instance(project: &Project, agent_dir: &str, relative_path: &str) -> Result<PathBuf, String> {
    let (enabled_root, disabled_root) = slot_roots(project, agent_dir)?;
    let enabled_path = enabled_root.join(relative_path);
    let disabled_path = disabled_root.join(relative_path);
    let (path, root) = if enabled_path.symlink_metadata().is_ok() {
        (enabled_path, &enabled_root)
    } else if disabled_path.symlink_metadata().is_ok() {
        (disabled_path, &disabled_root)
    } else {
        return Err(format!("未找到技能：{relative_path}（在 {agent_dir} 及其禁用目录里都没找到）"));
    };
    ensure_dir_within_root(&path, root)?;
    Ok(path)
}

// ---------------------------------------------------------------------------
// 启用 / 禁用（rename，非元数据）
// ---------------------------------------------------------------------------

/// 只删除「确定是软链」的条目；真实目录/文件绝不误删。
fn remove_symlink_entry(path: &Path) -> Result<(), String> {
    match std::fs::symlink_metadata(path) {
        Err(_) => Ok(()), // 不存在 = 已是目标状态
        Ok(m) if m.file_type().is_symlink() => std::fs::remove_file(path).map_err(|e| format!("remove {}: {e}", path.display())),
        Ok(_) => Err(format!("{} 不是软链，请手动处理（拒绝误删真实目录）", path.display())),
    }
}

/// 从 start 向上清理空目录，直到 root（含 root 本身）。只删空目录
/// （remove_dir 天然安全），非空/出界即停。
fn cleanup_empty_dirs_up_to(start: &Path, root: &Path) {
    let Ok(root_canon) = std::fs::canonicalize(root) else { return };
    let mut cur = Some(start.to_path_buf());
    while let Some(dir) = cur {
        let Ok(dir_canon) = std::fs::canonicalize(&dir) else { return };
        if !dir_canon.starts_with(&root_canon) {
            return; // 出界即停
        }
        if std::fs::remove_dir(&dir).is_err() {
            return; // 非空或不可删即停
        }
        cur = dir.parent().map(|p| p.to_path_buf());
    }
}

#[derive(Debug, Deserialize)]
pub struct ToggleProjectSkillRequest {
    pub project_id: String,
    pub agent_dir: String,
    pub relative_path: String,
    pub enabled: bool,
}

/// 启用 = `<skills>-disabled/<rel>` rename 回 `<skills>/<rel>`；
/// 禁用 = 反向。双侧同时存在真实目录时拒绝（两份可能内容不同，
/// 静默保留会让用户以为只有一份生效）。
#[tauri::command]
pub fn toggle_project_skill(req: ToggleProjectSkillRequest, state: State<'_, Arc<AppState>>) -> Result<(), String> {
    let snap = state.snapshot();
    let project = find_project(&snap, &req.project_id)?;
    toggle_on_disk(&project, &req.agent_dir, &req.relative_path, req.enabled)
}

/// toggle 的纯磁盘逻辑（命令与测试共用）。
fn toggle_on_disk(project: &Project, agent_dir: &str, relative_path: &str, enabled: bool) -> Result<(), String> {
    ensure_safe_skill_relative_path(relative_path)?;
    let (enabled_root, disabled_root) = slot_roots(project, agent_dir)?;
    let enabled_path = enabled_root.join(relative_path);
    let disabled_path = disabled_root.join(relative_path);

    let (src, dst, src_root, dst_root) = if enabled {
        (&disabled_path, &enabled_path, &disabled_root, &enabled_root)
    } else {
        (&enabled_path, &disabled_path, &enabled_root, &disabled_root)
    };
    let kind_of = |p: &Path| {
        p.symlink_metadata()
            .map(|m| if m.file_type().is_symlink() { "link" } else if m.is_dir() { "dir" } else { "other" })
            .unwrap_or("absent")
    };

    // 目标位置已是目录 → 已是请求的状态；若源位置残留一条软链（双侧重复），
    // 只清掉软链，真实目录绝不动；双侧真实目录并存则报错让用户手动处理。
    match kind_of(dst) {
        "dir" => {
            match kind_of(src) {
                "link" => {
                    remove_symlink_entry(src)?;
                    if let Some(parent) = src.parent() {
                        cleanup_empty_dirs_up_to(parent, src_root);
                    }
                }
                "dir" => {
                    return Err(format!("{} 里已存在同名条目：{}，请手动处理", dst_root.display(), dst.display()));
                }
                _ => {}
            }
            return Ok(());
        }
        "link" => remove_symlink_entry(dst)?, // 悬空/占位的软链可清
        "other" => return Err(format!("{} 已被占用", dst.display())),
        _ => {}
    }
    // 源位置必须存在（目录或软链目录）。
    match std::fs::symlink_metadata(src) {
        Err(_) => return Err(format!("未找到技能：{relative_path}")),
        Ok(m) if m.is_dir() || m.file_type().is_symlink() => {}
        Ok(_) => return Err(format!("{} 不是目录", src.display())),
    }
    ensure_dir_within_root(src, src_root)?;
    std::fs::create_dir_all(dst.parent().unwrap_or(dst_root)).map_err(|e| format!("create {}: {e}", dst_root.display()))?;
    std::fs::rename(src, dst).map_err(|e| format!("移动失败：{e}"))?;
    // 启用方向清理源侧空目录树（禁用方向的技能占着源侧，不清理）。
    if enabled {
        if let Some(parent) = src.parent() {
            cleanup_empty_dirs_up_to(parent, src_root);
        }
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// 删除
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
pub struct ProjectSkillRef {
    pub project_id: String,
    pub agent_dir: String,
    pub relative_path: String,
}

/// 删除项目里的技能实例：软链只删链接；真实目录移入废纸篓（可恢复）。
#[tauri::command]
pub fn delete_project_skill(req: ProjectSkillRef, state: State<'_, Arc<AppState>>) -> Result<(), String> {
    let snap = state.snapshot();
    let project = find_project(&snap, &req.project_id)?;
    ensure_safe_skill_relative_path(&req.relative_path)?;
    let (enabled_root, disabled_root) = slot_roots(&project, &req.agent_dir)?;
    let path = locate_skill_instance(&project, &req.agent_dir, &req.relative_path)?;
    let meta = std::fs::symlink_metadata(&path).map_err(|e| e.to_string())?;
    if meta.file_type().is_symlink() {
        std::fs::remove_file(&path).map_err(|e| format!("移除软链失败：{e}"))?;
    } else {
        // 调用前已有用户确认（前端二次确认）。
        crate::groups::trash_dir(&path)?;
    }
    // 清掉两侧残留的空父目录。
    let root = if path.starts_with(&enabled_root) { &enabled_root } else { &disabled_root };
    if let Some(parent) = path.parent() {
        cleanup_empty_dirs_up_to(parent, root);
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// 收编（项目 → 中央仓库）
// ---------------------------------------------------------------------------

#[derive(Debug, Serialize)]
pub struct ProjectAdoptResult {
    pub new_path: String,
    pub source_id: String,
    /// true = 中央仓库尚未注册来源，本次自动注册了一个。
    pub source_registered: bool,
    /// true = 技能真身已在中央仓库里（如项目内是条指向仓库的软链），未复制。
    pub already_in_vault: bool,
}

/// 收编：把项目技能复制进中央仓库（重名自动 -2/-3），注册仓库来源并
/// 触发扫描。项目内文件保持原样 —— 项目的 git 仓库不受影响（绝不把项目
/// 内条目改写成软链，否则会被提交进 git）。
#[tauri::command]
pub fn adopt_project_skill(req: ProjectSkillRef, state: State<'_, Arc<AppState>>, app: AppHandle) -> Result<ProjectAdoptResult, String> {
    let snap = state.snapshot();
    let project = find_project(&snap, &req.project_id)?;
    ensure_safe_skill_relative_path(&req.relative_path)?;
    let path = locate_skill_instance(&project, &req.agent_dir, &req.relative_path)?;

    // 解析真身：软链取解析目标（必须存在且是目录）。
    let meta = std::fs::symlink_metadata(&path).map_err(|e| e.to_string())?;
    let real = if meta.file_type().is_symlink() {
        let target = std::fs::read_link(&path).map_err(|e| format!("解析软链失败：{e}"))?;
        let t = if target.is_absolute() { target } else { path.parent().unwrap_or(Path::new("/")).join(target) };
        if !t.is_dir() {
            return Err(format!("软链目标不是目录：{}", t.display()));
        }
        t
    } else {
        path.clone()
    };
    let canonical = std::fs::canonicalize(&real).unwrap_or_else(|_| real.clone());

    let vault = crate::discover::effective_vault_dir(&snap.ui.vault_path).ok_or("无法确定主目录")?;
    let vault_canon = std::fs::canonicalize(&vault).unwrap_or_else(|_| vault.clone());
    if canonical.starts_with(&vault_canon) {
        return Err("该技能的真身已在中央仓库里".into());
    }

    let name = canonical
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .ok_or_else(|| "无法解析目录名".to_string())?;
    std::fs::create_dir_all(&vault).map_err(|e| format!("create {}: {e}", vault.display()))?;
    let mut dest = vault.join(&name);
    let mut n = 2;
    while dest.exists() {
        dest = vault.join(format!("{name}-{n}"));
        n += 1;
    }
    copy_dir_filtered(&canonical, &dest)?;
    log::info!("adopt_project: copied {} -> {}", canonical.display(), dest.display());

    // 注册中央仓库来源（幂等：已注册则复用）。
    let (source_id, source_registered) = {
        let mut guard = state.inner.write();
        if let Some(s) = guard
            .sources
            .iter()
            .find(|s| s.kind == SourceKind::Local && Path::new(&s.location) == vault)
        {
            (s.id, false)
        } else {
            let id = Uuid::new_v4();
            guard.sources.push(Source {
                id,
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
            });
            (id, true)
        }
    }; // release write lock before save
    state.save().map_err(|e| e.to_string())?;

    // 后台扫描中央仓库来源，把新副本索引进技能库。
    let state_for_thread = state.inner().clone();
    let app_for_thread = app.clone();
    std::thread::spawn(move || {
        crate::commands::run_scan_blocking(&app_for_thread, &state_for_thread, source_id, "adopt-project".to_string());
        let _ = app_for_thread.emit("skill-dock://source-scanned", source_id.to_string());
    });

    Ok(ProjectAdoptResult {
        new_path: dest.to_string_lossy().to_string(),
        source_id: source_id.to_string(),
        source_registered,
        already_in_vault: false,
    })
}

/// 递归复制目录，跳过 .git/.DS_Store 等点文件与内部符号链接。
fn copy_dir_filtered(src: &Path, dst: &Path) -> Result<(), String> {
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

// ---------------------------------------------------------------------------
// 导出（中央 → 项目）与更新（中央 → 项目副本）
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
pub struct ExportToProjectRequest {
    pub skill_id: String,
    pub project_id: String,
    /// 槽位标识（如 ".claude/skills"）。
    pub agent_dir: String,
    /// "copy"（默认，随 git 走的独立副本）| "symlink"（链回中央仓库）。
    pub mode: Option<String>,
}

/// 把库内技能安装到项目：写入 `<project>/<agent_dir>/<link_name>`。
/// 启用/禁用两侧任一已存在即整体拒绝（NoClobber —— 项目目录里可能有
/// 用户手写内容，绝静默不覆盖）。
#[tauri::command]
pub fn export_skill_to_project(req: ExportToProjectRequest, state: State<'_, Arc<AppState>>) -> Result<String, String> {
    let snap = state.snapshot();
    let project = find_project(&snap, &req.project_id)?;
    let skill = snap
        .skills
        .get(&req.skill_id)
        .cloned()
        .ok_or_else(|| format!("技能不存在：{}", req.skill_id))?;
    let (enabled_root, disabled_root) = slot_roots(&project, &req.agent_dir)?;

    let link_name = crate::symlinks::link_name_for(&skill);
    let enabled_path = enabled_root.join(&link_name);
    let disabled_path = disabled_root.join(&link_name);
    for p in [&enabled_path, &disabled_path] {
        if p.symlink_metadata().is_ok() {
            return Err(format!("项目中已存在同名条目：{}，拒绝覆盖", p.display()));
        }
    }
    if !skill.absolute_path.is_dir() {
        return Err(format!("技能源目录不存在：{}", skill.absolute_path.display()));
    }

    std::fs::create_dir_all(&enabled_root).map_err(|e| format!("create {}: {e}", enabled_root.display()))?;
    let mode = req.mode.as_deref().unwrap_or("copy");
    match mode {
        "symlink" => {
            #[cfg(unix)]
            {
                std::os::unix::fs::symlink(&skill.absolute_path, &enabled_path)
                    .map_err(|e| format!("创建软链失败：{e}"))?;
            }
            #[cfg(not(unix))]
            {
                return Err("当前平台不支持软链模式".into());
            }
        }
        "copy" => {
            copy_dir_filtered(&skill.absolute_path, &enabled_path)?;
        }
        other => return Err(format!("未知模式：{other}")),
    }
    log::info!("export_to_project: {} -> {} ({mode})", skill.name, enabled_path.display());
    Ok(enabled_path.to_string_lossy().to_string())
}

/// 中央较新 / 分叉时，把库内副本拉回项目（覆盖项目内旧副本）。
/// 项目较新时拒绝 —— 那会覆盖用户刚做过的编辑，这是本功能的安全闸。
#[tauri::command]
pub fn update_project_skill_from_center(req: ProjectSkillRef, state: State<'_, Arc<AppState>>) -> Result<(), String> {
    let snap = state.snapshot();
    let project = find_project(&snap, &req.project_id)?;
    ensure_safe_skill_relative_path(&req.relative_path)?;
    let path = locate_skill_instance(&project, &req.agent_dir, &req.relative_path)?;

    // 重新扫描这一个根，拿到该实例的实时匹配与状态。
    let (enabled_root, disabled_root) = slot_roots(&project, &req.agent_dir)?;
    let root = if path.starts_with(&enabled_root) { enabled_root.clone() } else { disabled_root.clone() };
    let scanned = scan_skills_root(&root)
        .into_iter()
        .find(|s| s.relative_path == req.relative_path)
        .ok_or_else(|| "未找到技能实例".to_string())?;
    let mut canon_cache = BTreeMap::new();
    let matched = best_center_match(&snap, &scanned, &mut canon_cache);
    let status = classify_status(matched.as_ref(), &scanned);
    if status == "project_newer" {
        return Err("项目里的版本比中央库新，已拒绝覆盖。请先收编项目版本。".into());
    }
    let Some((_, skill)) = matched else {
        return Err("中央库里没有匹配的技能".into());
    };
    if !skill.absolute_path.is_dir() {
        return Err(format!("技能源目录不存在：{}", skill.absolute_path.display()));
    }

    // 替换：软链只删链接；真实目录进废纸篓（可恢复），再写入新副本。
    let meta = std::fs::symlink_metadata(&path).map_err(|e| e.to_string())?;
    if meta.file_type().is_symlink() {
        std::fs::remove_file(&path).map_err(|e| format!("移除软链失败：{e}"))?;
    } else {
        crate::groups::trash_dir(&path)?;
    }
    copy_dir_filtered(&skill.absolute_path, &path)?;
    log::info!("update_from_center: {} -> {}", skill.name, path.display());
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::AgentTarget;
    use chrono::Utc;

    fn tmpdir(tag: &str) -> PathBuf {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let p = std::env::temp_dir().join(format!("sm-projects-{tag}-{}-{nanos}", std::process::id()));
        std::fs::create_dir_all(&p).unwrap();
        p
    }

    fn make_skill(dir: &Path, name: &str) -> Skill {
        Skill {
            id: format!("test-source:{name}"),
            source_id: uuid::Uuid::new_v4(),
            name: name.to_string(),
            description: String::new(),
            relative_path: name.into(),
            absolute_path: dir.join(name),
            has_skill_md: true,
            content_hash: blake3::hash(b"same").to_hex().to_string(),
            modified_at: Utc::now(),
            size_bytes: 0,
            is_shared: false,
            is_symlink: false,
            symlink_target: None,
        }
    }

    fn proj_skill(name: &str, hash: &str, mtime_ms: i64) -> ScannedSkill {
        ScannedSkill {
            name: name.into(),
            dir_name: name.into(),
            relative_path: name.into(),
            abs: PathBuf::from(format!("/tmp/{name}")),
            hash: hash.into(),
            mtime_ms: Some(mtime_ms),
            is_symlink: false,
        }
    }

    #[test]
    fn classify_five_states() {
        let mut snap = PersistedState::default();
        let center = make_skill(Path::new("/tmp/center"), "web-search");
        snap.skills.insert(center.id.clone(), center.clone());

        let mut cache = BTreeMap::new();
        let base = 10_000i64;

        // 仅项目：库里没有这个名字/哈希都不同的匹配。
        let lonely = proj_skill("lonely", "hash-x", base);
        assert_eq!(classify_status(best_center_match(&snap, &lonely, &mut cache).as_ref(), &lonely), "project_only");

        // 一致（名称命中 + 哈希相等）。
        let same = proj_skill("web-search", center.content_hash.as_str(), base);
        assert_eq!(classify_status(best_center_match(&snap, &same, &mut cache).as_ref(), &same), "in_sync");

        // 项目较新：哈希不同、项目 mtime 比中央新 2 秒以上。
        let mut newer_center = center.clone();
        newer_center.content_hash = "different".into();
        newer_center.modified_at = chrono::DateTime::from_timestamp_millis(base).unwrap();
        snap.skills.insert(newer_center.id.clone(), newer_center);
        let p_newer = proj_skill("web-search", "other-hash", base + 2_500);
        assert_eq!(classify_status(best_center_match(&snap, &p_newer, &mut cache).as_ref(), &p_newer), "project_newer");

        // 中央较新：中央 mtime 比项目新 2 秒以上。
        let c_newer = proj_skill("web-search", "other-hash", base - 2_500);
        assert_eq!(classify_status(best_center_match(&snap, &c_newer, &mut cache).as_ref(), &c_newer), "center_newer");

        // 分叉：哈希不同、mtime 差在 1 秒容差内。
        let div = proj_skill("web-search", "other-hash", base + 500);
        assert_eq!(classify_status(best_center_match(&snap, &div, &mut cache).as_ref(), &div), "diverged");
    }

    #[test]
    fn safe_relative_path_rejects_traversal() {
        assert!(ensure_safe_skill_relative_path("foo").is_ok());
        assert!(ensure_safe_skill_relative_path("category/sub/skill").is_ok());
        assert!(ensure_safe_skill_relative_path("").is_err());
        assert!(ensure_safe_skill_relative_path("..").is_err());
        assert!(ensure_safe_skill_relative_path("a/../../b").is_err());
        assert!(ensure_safe_skill_relative_path("/abs").is_err());
        assert!(ensure_safe_skill_relative_path("./dot").is_err());
    }

    #[cfg(unix)]
    #[test]
    fn toggle_rename_roundtrip_and_cleanup() {
        let root = tmpdir("toggle");
        let project = Project {
            id: Uuid::new_v4(),
            name: "proj".into(),
            path: root.clone(),
            created_at: Utc::now(),
            sort_order: 0,
        };
        let skills_root = root.join(".claude/skills");
        let disabled_root = root.join(".claude/skills-disabled");
        let skill_dir = skills_root.join("demo");
        std::fs::create_dir_all(&skill_dir).unwrap();
        std::fs::write(skill_dir.join("SKILL.md"), "---\nname: demo\n---\nhi").unwrap();

        // 禁用：移入 .claude/skills-disabled/demo。
        toggle_on_disk(&project, ".claude/skills", "demo", false).unwrap();
        assert!(!skill_dir.exists());
        assert!(disabled_root.join("demo").join("SKILL.md").is_file());

        // 启用：移回，并清空整个 skills-disabled 残树（连根一起删）。
        toggle_on_disk(&project, ".claude/skills", "demo", true).unwrap();
        assert!(skill_dir.join("SKILL.md").is_file());
        assert!(!disabled_root.exists(), "空禁用树应被清理");

        std::fs::remove_dir_all(&root).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn toggle_refuses_when_both_sides_exist() {
        let root = tmpdir("dup");
        let project = Project {
            id: Uuid::new_v4(),
            name: "proj".into(),
            path: root.clone(),
            created_at: Utc::now(),
            sort_order: 0,
        };
        let enabled = root.join(".claude/skills/demo");
        let disabled = root.join(".claude/skills-disabled/demo");
        std::fs::create_dir_all(&enabled).unwrap();
        std::fs::create_dir_all(&disabled).unwrap();

        let err = toggle_on_disk(&project, ".claude/skills", "demo", false).unwrap_err();
        assert!(err.contains("已存在"), "got: {err}");
        // 双侧都保留。
        assert!(enabled.is_dir() && disabled.is_dir());
        std::fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn agent_slots_derive_and_merge() {
        let mk = |id: &str, name: &str, dir: &str| AgentTarget {
            id: id.into(),
            name: name.into(),
            skills_dir: dirs::home_dir().unwrap().join(dir.trim_start_matches("~/")),
            extra_dirs: Vec::new(),
            enabled: true,
            description: String::new(),
            tag: None,
            detected: true,
            user_touched: false,
        };
        let targets = vec![
            mk("claude-code", "Claude Code", "~/.claude/skills/"),
            mk("pi", "Pi", "~/.pi/agent/skills/"), // override → .pi/skills
            mk("a1", "A One", "~/.agents/skills/"),
            mk("a2", "A Two", "~/.agents/skills/"), // 同目录合并
            mk("mm", "MiniMax", "~/.minimax/agents/"), // 非 skills 结尾 → 排除
        ];
        let slots = agent_slots(&targets);
        let dirs: Vec<&str> = slots.iter().map(|s| s.dir.as_str()).collect();
        assert!(dirs.contains(&".claude/skills"));
        assert!(dirs.contains(&".pi/skills"), "pi 应走 override 映射");
        let merged = slots.iter().find(|s| s.dir == ".agents/skills").expect("agents/skills 槽位应存在");
        assert_eq!(merged.display, "A One / A Two");
        assert!(!dirs.iter().any(|d| d.contains("minimax")));
    }
}
