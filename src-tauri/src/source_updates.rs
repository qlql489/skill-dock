//! 来源更新预览 —— 在真正拉取之前，把"远端有什么变化"摆给用户看。
//!
//! 机制：对 GitHub 来源的本地克隆执行 `git fetch --depth 1`（只更新
//! `origin/<branch>` 引用，绝不碰工作区），然后用 `git diff` 对比当前
//! HEAD 与远端 tip：
//! - `--name-status` 得到文件级变更清单（新增/修改/删除/重命名）
//! - 需要看细节时对单个文件出 `-U3` unified patch
//!
//! 这借鉴了 xingkongliang/skills-manager 的"预览→确认→应用"三段式，
//! 但对比输出直接复用系统 git，不再自己造 LCS。

use crate::sources::{git_command, run_git_output_with_timeout};
use crate::state::{AppState, CloneStatus, SourceKind};
use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tauri::State;

const GIT_TIMEOUT_SECS: u64 = 60;
/// 文件清单条目上限（超大 monorepo diff 兜底）。
const MAX_FILES: usize = 500;
/// 单文件 patch 上限，超出截断。
const MAX_PATCH_BYTES: usize = 256 * 1024;
/// 统计落后提交数时的加深抓取上限：超过 100 个提交就只报 "100+"，
/// 不为精确计数无限制地拉历史。
const DEEPEN_DEPTH: &str = "100";
/// 预览弹窗里展示的远端新提交主题条数上限。
const MAX_RECENT_COMMITS: usize = 10;

#[derive(Debug, Serialize)]
pub struct FileChange {
    /// added | modified | deleted | renamed
    pub status: String,
    pub path: String,
    /// renamed 时的新路径（path 字段存旧路径）。
    pub new_path: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct UpdatePreview {
    pub source_id: String,
    pub name: String,
    pub branch: String,
    pub current_sha: Option<String>,
    pub remote_sha: Option<String>,
    pub has_update: bool,
    pub files: Vec<FileChange>,
    /// 变更会触及的已入库 skill 名单。
    pub affected_skills: Vec<String>,
    /// true = 文件清单超过 MAX_FILES 被截断。
    pub truncated: bool,
    /// 本地落后远端的提交数（HEAD..origin/branch）。None = 统计不出来
    /// （历史分叉、加深抓取失败等）。
    pub behind_count: Option<u64>,
    /// false = 计数触及加深上限，真实落后更多（显示为 "N+"）。
    pub behind_exact: bool,
    /// 远端新提交主题（最新在前，最多 MAX_RECENT_COMMITS 条）。
    pub recent_commits: Vec<String>,
    pub note: Option<String>,
}

fn source_snapshot(state: &AppState, id: uuid::Uuid) -> Result<crate::state::Source> {
    let guard = state.inner.read();
    guard
        .sources
        .iter()
        .find(|s| s.id == id)
        .cloned()
        .context("source not found")
}

fn validate_github(src: &crate::state::Source) -> Result<PathBuf> {
    if src.kind != SourceKind::Github {
        anyhow::bail!("只有 GitHub 来源支持更新对比");
    }
    let path = src
        .clone_path
        .clone()
        .context("来源缺少本地克隆目录")?;
    if !path.is_dir() {
        anyhow::bail!("本地克隆目录不存在：{}", path.display());
    }
    Ok(path)
}

/// `git fetch --depth 1 origin <branch>`，只移动远端引用不碰工作区。
fn fetch_branch(clone: &Path, branch: &str) -> Result<()> {
    let mut cmd = git_command(clone);
    cmd.args(["fetch", "--depth", "1", "origin", branch]);
    let _ = timeout_git(cmd)?;
    Ok(())
}

fn rev_parse(clone: &Path, what: &str) -> Result<Option<String>> {
    let mut cmd = git_command(clone);
    cmd.args(["rev-parse", what]);
    match run_git_output_with_timeout(cmd) {
        Ok(out) => Ok(Some(out)),
        Err(_) => Ok(None),
    }
}

fn is_shallow(clone: &Path) -> bool {
    rev_parse(clone, "--is-shallow-repository")
        .ok()
        .flatten()
        .map(|v| v == "true")
        .unwrap_or(false)
}

/// 带超时的 git 调用包装（错误带 stderr 详情）。
fn timeout_git(mut cmd: std::process::Command) -> Result<String> {
    // 直接借用 sources 的实现思路：child + try_wait 轮询。
    use std::io::Read;
    use std::time::{Duration, Instant};

    let child = cmd
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .context("failed to spawn git")?;
    let mut child = child;
    let deadline = Instant::now() + Duration::from_secs(GIT_TIMEOUT_SECS);
    loop {
        match child.try_wait().context("polling git child")? {
            Some(status) => {
                let mut out = String::new();
                let mut err_text = String::new();
                if let Some(mut s) = child.stdout.take() {
                    let _ = s.read_to_string(&mut out);
                }
                if let Some(mut s) = child.stderr.take() {
                    let _ = s.read_to_string(&mut err_text);
                }
                if !status.success() {
                    let t = err_text.trim();
                    anyhow::bail!(if t.is_empty() {
                        format!("git exited with status {status}")
                    } else {
                        format!("git exited with status {status}: {t}")
                    });
                }
                return Ok(out.trim().to_string());
            }
            None => {
                if Instant::now() >= deadline {
                    let _ = child.kill();
                    let _ = child.wait();
                    anyhow::bail!("git timed out after {GIT_TIMEOUT_SECS}s");
                }
                std::thread::sleep(Duration::from_millis(150));
            }
        }
    }
}

/// 重命名行形如 `R100\told\tnew`；普通行 `A/M/D/T\tpath`。
fn parse_name_status(out: &str) -> (Vec<FileChange>, bool) {
    let mut files = Vec::new();
    let mut truncated = false;
    for line in out.lines() {
        if line.trim().is_empty() {
            continue;
        }
        let mut parts = line.split('\t');
        let st = parts.next().unwrap_or("").to_string();
        let clean_status = st.chars().next().map(|c| c.to_string()).unwrap_or(st.clone());
        match clean_status.as_str() {
            "A" | "M" | "D" | "T" => {
                let Some(path) = parts.next() else { continue };
                if files.len() >= MAX_FILES {
                    truncated = true;
                    break;
                }
                files.push(FileChange {
                    status: normalize_status(&clean_status),
                    path: path.to_string(),
                    new_path: None,
                });
            }
            "R" | "C" => {
                let (Some(old), Some(new)) = (parts.next(), parts.next()) else { continue };
                if files.len() >= MAX_FILES {
                    truncated = true;
                    break;
                }
                files.push(FileChange {
                    status: if clean_status == "R" { "renamed".into() } else { "added".into() },
                    path: old.to_string(),
                    new_path: Some(new.to_string()),
                });
            }
            _ => {}
        }
    }
    (files, truncated)
}

fn normalize_status(s: &str) -> String {
    match s {
        "A" => "added",
        "D" => "deleted",
        "T" => "modified",
        _ => s,
    }
    .to_string()
}

// ---------------------------------------------------------------------------
// Tauri commands
// ---------------------------------------------------------------------------

#[tauri::command]
pub async fn preview_source_update(
    id: String,
    state: State<'_, Arc<AppState>>,
) -> Result<UpdatePreview, String> {
    let id = uuid::Uuid::parse_str(&id).map_err(|e| e.to_string())?;
    let state_arc = state.inner().clone();
    tauri::async_runtime::spawn_blocking(move || run_preview(&state_arc, id))
        .await
        .map_err(|e| format!("join error: {e}"))?
}

fn run_preview(state: &Arc<AppState>, id: uuid::Uuid) -> Result<UpdatePreview, String> {
    let src = source_snapshot(state, id).map_err(|e| e.to_string())?;
    let clone = validate_github(&src).map_err(|e| e.to_string())?;
    let branch = src.branch.clone().unwrap_or_else(|| "main".to_string());

    let mut preview = UpdatePreview {
        source_id: id.to_string(),
        name: src.name.clone(),
        branch: branch.clone(),
        current_sha: None,
        remote_sha: None,
        has_update: false,
        files: Vec::new(),
        affected_skills: Vec::new(),
        truncated: false,
        behind_count: None,
        behind_exact: true,
        recent_commits: Vec::new(),
        note: None,
    };

    preview.current_sha = rev_parse(&clone, "HEAD").unwrap_or(None);

    if let Err(e) = fetch_branch(&clone, &branch) {
        preview.note = Some(format!("无法连接远端：{e:#}"));
        return Ok(preview);
    }
    preview.remote_sha = rev_parse(&clone, &format!("origin/{branch}")).unwrap_or(None);

    let (current, remote) = match (&preview.current_sha, &preview.remote_sha) {
        (Some(c), Some(r)) => (c.clone(), r.clone()),
        _ => {
            preview.note = Some("无法解析提交号（浅克隆或仓库异常）".into());
            return Ok(preview);
        }
    };
    if current == remote {
        return Ok(preview); // up to date — has_update=false, files empty
    }

    let diff_out = {
        let mut cmd = git_command(&clone);
        cmd.args(["diff", "--name-status", "-M", &current, &remote]);
        timeout_git(cmd).map_err(|e| e.to_string())?
    };
    let (files, truncated) = parse_name_status(&diff_out);
    preview.files = files;
    preview.truncated = truncated;
    preview.has_update = true;

    // 落后多少个提交：浅克隆没有中间历史，数不出来 —— 先加深抓取
    // （最多 DEEPEN_DEPTH 个提交，失败不阻塞预览），再 rev-list 计数。
    if is_shallow(&clone) {
        let mut cmd = git_command(&clone);
        cmd.args(["fetch", "--depth", DEEPEN_DEPTH, "origin", &branch]);
        let _ = timeout_git(cmd);
    }
    let cap = DEEPEN_DEPTH.parse::<u64>().unwrap_or(100);
    if let Some(out) = {
        let mut cmd = git_command(&clone);
        cmd.args(["rev-list", "--count", &format!("{current}..{remote}")]);
        timeout_git(cmd).ok()
    } {
        if let Ok(n) = out.trim().parse::<u64>() {
            // 仍是浅克隆且计数触及加深上限 → 真实落后更多，显示为 N+。
            preview.behind_exact = !(is_shallow(&clone) && n >= cap);
            preview.behind_count = Some(n);
        }
    }
    // 远端新提交主题（最新在前），给预览弹窗一个"这次都更新了啥"的概览。
    if let Some(out) = {
        let mut cmd = git_command(&clone);
        cmd.args([
            "log",
            "--no-decorate",
            "--format=%s",
            &format!("-{}", MAX_RECENT_COMMITS),
            &format!("{current}..{remote}"),
        ]);
        timeout_git(cmd).ok()
    } {
        preview.recent_commits = out
            .lines()
            .map(|l| l.trim())
            .filter(|l| !l.is_empty())
            .take(MAX_RECENT_COMMITS)
            .map(String::from)
            .collect();
    }

    // 受影响的 skill：变更文件落在某个 skill 目录前缀下即算命中。
    let snap = state.snapshot();
    let prefix_all = format!("{id}:");
    for skill in snap.skills.values() {
        if !skill.id.starts_with(&prefix_all) {
            continue;
        }
        let dir = skill.relative_path.to_string_lossy().to_string();
        let hit = preview.files.iter().any(|f| {
            let new_p = f.new_path.as_deref().unwrap_or("");
            f.path.starts_with(&format!("{dir}/"))
                || new_p.starts_with(&format!("{dir}/"))
                || f.path == dir
        });
        if hit {
            preview.affected_skills.push(skill.name.clone());
            if preview.affected_skills.len() >= 50 {
                break;
            }
        }
    }

    Ok(preview)
}

#[derive(Debug, Deserialize)]
pub struct PatchRequest {
    pub source_id: String,
    pub path: String,
}

/// 单个文件的 unified diff（-U3）。超大文件截断并附加说明行。
#[tauri::command]
pub async fn source_file_patch(
    req: PatchRequest,
    state: State<'_, Arc<AppState>>,
) -> Result<String, String> {
    let id = uuid::Uuid::parse_str(&req.source_id).map_err(|e| e.to_string())?;
    let path = req.path.clone();
    let state_arc = state.inner().clone();
    tauri::async_runtime::spawn_blocking(move || {
        let src = source_snapshot(&state_arc, id).map_err(|e| e.to_string())?;
        let clone = validate_github(&src).map_err(|e| e.to_string())?;
        let branch = src.branch.clone().unwrap_or_else(|| "main".to_string());
        let current = rev_parse(&clone, "HEAD").map_err(|e| e.to_string())?.unwrap_or_default();
        let remote = rev_parse(&clone, &format!("origin/{branch}"))
            .map_err(|e| e.to_string())?
            .unwrap_or_default();

        let mut cmd = git_command(&clone);
        cmd.args(["diff", "-U3", "--no-color", &current, &remote, "--", &path]);
        let out = timeout_git(cmd).map_err(|e| e.to_string())?;

        if out.len() > MAX_PATCH_BYTES {
            let mut cut = out;
            cut.truncate(MAX_PATCH_BYTES);
            cut.push_str("\n…（补丁过长已截断）");
            return Ok(cut);
        }
        Ok(out)
    })
    .await
    .map_err(|e| format!("join error: {e}"))?
}
