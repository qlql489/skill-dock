//! Source management: add, remove, scan.
//!
//! Git operations (clone / fetch / rev-parse) go through the system `git`
//! binary via `std::process::Command`. We previously used the `git2` crate
//! (libgit2), but it was slower than native git, had no timeout, no progress
//! callback, and no access to the system credential helper / SSH agent — so
//! cloning large repos hung indefinitely and left empty `.git` shells behind
//! on failure. Native git gives us `--depth 1` shallow clones, credentials
//! for free, parseable stderr, and a killable child process for hard timeouts.

use crate::paths::repos_dir;
use crate::scanner;
use crate::state::{CloneStatus, PersistedState, Skill, Source, SourceKind, SourceMode};
use anyhow::{Context, Result};
use chrono::Utc;
use std::path::Path;
use std::process::Command;
use std::time::Duration;
use uuid::Uuid;

/// Hard cap on a single clone/fetch. Bounded because the whole point of this
/// refactor is that network ops must never hang the app indefinitely. Two
/// minutes is generous for a `--depth 1` clone even on a slow link.
const GIT_TIMEOUT: Duration = Duration::from_secs(120);

pub fn add_local(
    state: &mut PersistedState,
    name: String,
    location: String,
    mode: SourceMode,
    auto_sync_targets: Vec<String>,
) -> Result<Source> {
    let location = expand_home(&location);
    let path = std::path::PathBuf::from(&location);
    if !path.exists() {
        anyhow::bail!("path does not exist: {}", path.display());
    }
    if !path.is_dir() {
        anyhow::bail!("path is not a directory: {}", path.display());
    }

    // Reject duplicates by canonical location.
    if state.sources.iter().any(|s| s.location == location && s.kind == SourceKind::Local) {
        anyhow::bail!("source already added: {location}");
    }

    let src = Source {
        id: Uuid::new_v4(),
        kind: SourceKind::Local,
        name: if name.trim().is_empty() {
            path.file_name()
                .map(|n| n.to_string_lossy().to_string())
                .unwrap_or_else(|| location.clone())
        } else {
            name
        },
        location,
        clone_path: None,
        branch: None,
        created_at: Utc::now(),
        last_scanned_at: None,
        last_commit_sha: None,
        skill_count: 0,
        clone_status: CloneStatus::Ready,
        clone_error: None,
        mode,
        auto_sync_targets,
    };

    state.sources.push(src.clone());
    Ok(src)
}

/// Register a GitHub source WITHOUT cloning it. Allocates the clone path and
/// leaves the source in `Cloning` state; the actual (slow, network-bound)
/// clone is performed separately by [`perform_clone`] on a background thread.
/// Splitting register-from-clone is what lets the command layer keep the
/// write lock short and run `git clone` lock-free.
pub fn add_github(state: &mut PersistedState, name: String, url: String, branch: Option<String>, mode: SourceMode) -> Result<Source> {
    let url = url.trim().to_string();
    // git clone 本身支持任意 Git 主机（内网 GitLab/Gitea 常走 http://），
    // 这里只拦截明显不是 git 地址的输入，不限定 github.com。
    if !(url.starts_with("https://")
        || url.starts_with("http://")
        || url.starts_with("ssh://")
        || url.starts_with("git@")
        || url.starts_with("git://"))
    {
        anyhow::bail!("invalid Git URL: {url}");
    }
    if state.sources.iter().any(|s| s.location == url && s.kind == SourceKind::Github) {
        anyhow::bail!("source already added: {url}");
    }

    let id = Uuid::new_v4();
    let clone_path = repos_dir()?.join(id.to_string());

    let src = Source {
        id,
        kind: SourceKind::Github,
        name: if name.trim().is_empty() {
            repo_name_from_url(&url)
        } else {
            name
        },
        location: url,
        clone_path: Some(clone_path),
        branch,
        created_at: Utc::now(),
        last_scanned_at: None,
        last_commit_sha: None,
        skill_count: 0,
        // Advance to Cloning immediately; perform_clone will move it to
        // Ready/Failed when the background git call returns.
        clone_status: CloneStatus::Cloning,
        clone_error: None,
        mode,
        // GitHub sources don't support folder watching; auto-sync stays empty.
        auto_sync_targets: Vec::new(),
    };

    state.sources.push(src.clone());
    Ok(src)
}

/// Outcome of a background clone, consumed by the command layer to update
/// state under a short write lock.
pub struct CloneOutcome {
    pub branch: String,
    pub commit_sha: String,
}

/// Run the actual `git clone` for a source. Intended to run lock-free on a
/// background thread. On success returns the resolved branch + HEAD sha so
/// the caller can update the source record. On failure the partial clone
/// directory is cleaned up (so retries start fresh) and the error is
/// propagated.
pub fn perform_clone(src: &Source) -> Result<CloneOutcome> {
    let url = &src.location;
    let dest = src
        .clone_path
        .as_ref()
        .with_context(|| format!("github source {} has no clone_path", src.id))?;

    // If a previous failed attempt (or interrupted run) left a directory
    // behind, remove it so we start clean — `git clone` refuses to clone
    // into a non-empty dir.
    if dest.exists() {
        let _ = std::fs::remove_dir_all(dest);
    }
    if let Some(parent) = dest.parent() {
        std::fs::create_dir_all(parent)?;
    }

    clone_repo(url, dest, src.branch.as_deref())?;

    let branch = current_branch(dest).unwrap_or_else(|| "main".to_string());
    let commit_sha = head_commit(dest)?;
    log::info!(
        "cloned {url} -> {} (head {branch} @ {commit_sha})",
        dest.display()
    );
    Ok(CloneOutcome { branch, commit_sha })
}

pub fn remove_source(state: &mut PersistedState, id: Uuid) -> Result<()> {
    let pos = state
        .sources
        .iter()
        .position(|s| s.id == id)
        .context("source not found")?;
    let removed = state.sources.remove(pos);

    // Remove all skills associated with this source.
    let prefix = format!("{id}:");
    state.skills.retain(|sid, _| !sid.starts_with(&prefix));

    // Drop hidden-skill records that belonged to this source.
    state.hidden_skills.retain(|sid| !sid.starts_with(&prefix));

    // Optionally remove the clone path.
    if let Some(path) = removed.clone_path {
        if path.exists() {
            let _ = std::fs::remove_dir_all(&path);
        }
    }

    // Remove installations that point to this source.
    state.installations.retain(|inst| {
        let source_id_str = inst.skill_id.split(':').next().unwrap_or("");
        source_id_str != id.to_string()
    });

    Ok(())
}

/// Legacy single-lock rescan. Kept for potential future callers (the updater
/// no longer uses it since the ls-remote refactor).
#[allow(dead_code)]
pub fn rescan_source(state: &mut PersistedState, id: Uuid) -> Result<Vec<Skill>> {
    // Legacy path: does everything under a single write lock. Kept for the
    // updater (which scans a private clone, so holding the shared lock for
    // the duration is not an issue there). The command layer uses the split
    // read_scan_root + apply_scan_results form so the slow scan runs outside
    // any lock — see commands::run_scan_blocking.
    let (src, root) = read_scan_root(state, id)?;
    let vault = crate::discover::effective_vault_dir(&state.ui.vault_path)
        .unwrap_or_else(|| root.clone());
    let new_skills = scanner::scan(&src, &root, &vault)?;
    apply_scan_results(state, id, new_skills)
}

/// Read the source record and resolve the filesystem root to scan, WITHOUT
/// performing the scan. Intended to run under a short read lock; the actual
/// (slow, IO-bound) scan is then done lock-free by the caller.
pub fn read_scan_root(state: &PersistedState, id: Uuid) -> Result<(Source, std::path::PathBuf)> {
    let src = state
        .sources
        .iter()
        .find(|s| s.id == id)
        .cloned()
        .context("source not found")?;
    let root = match src.kind {
        SourceKind::Local => std::path::PathBuf::from(&src.location),
        SourceKind::Github => src.clone_path.clone().context("github source missing clone path")?,
    };
    Ok((src, root))
}

/// Write scan results back into state: replace this source's cached skills,
/// refresh scan metadata, and (for github) record the HEAD commit. Intended
/// to run under a short write lock after the scan has completed.
pub fn apply_scan_results(
    state: &mut PersistedState,
    id: Uuid,
    new_skills: Vec<Skill>,
) -> Result<Vec<Skill>> {
    // Replace skills for this source in the cache.
    state.skills.retain(|sid, _| !sid.starts_with(&format!("{id}:")));
    let mut visible = Vec::with_capacity(new_skills.len());
    for s in &new_skills {
        // Skip skills the user "deleted" (hidden) so they don't come back on
        // rescan. The hidden set is per-skill-id (source:relative_path).
        if state.hidden_skills.contains(&s.id) {
            continue;
        }
        state.skills.insert(s.id.clone(), s.clone());
        visible.push(s.clone());
    }

    // Update source metadata.
    let src_mut = state.sources.iter_mut().find(|s| s.id == id).unwrap();
    src_mut.last_scanned_at = Some(Utc::now());
    src_mut.skill_count = visible.len();

    if src_mut.kind == SourceKind::Github {
        if let Some(path) = &src_mut.clone_path {
            if let Ok(commit) = head_commit(path) {
                src_mut.last_commit_sha = Some(commit);
            }
        }
    }

    Ok(visible)
}

#[allow(dead_code)]
pub fn rescan_all(state: &mut PersistedState) -> Result<usize> {
    let ids: Vec<Uuid> = state.sources.iter().map(|s| s.id).collect();
    let mut total = 0;
    for id in ids {
        let skills = rescan_source(state, id)?;
        total += skills.len();
    }
    Ok(total)
}

/// Pull latest for a GitHub source if a clone already exists. Shallow clones
/// (which is what we create) can't do a normal `pull`, so this does a
/// `fetch --depth 1` of the tracked branch and refreshes `last_commit_sha`
/// from `origin/<branch>`. Runs lock-free — the caller snapshots the source
/// first.
pub fn pull_github_source(src: &mut Source) -> Result<()> {
    let path = src.clone_path.clone().context("no clone path")?;
    let branch = src
        .branch
        .clone()
        .unwrap_or_else(|| current_branch(&path).unwrap_or_else(|| "main".to_string()));

    let mut cmd = git_command(&path);
    cmd.args(["fetch", "--depth", "1", "origin", &branch]);
    run_git_with_timeout(cmd).context("git fetch failed")?;

    let sha = git_command(&path)
        .args(["rev-parse", &format!("origin/{branch}")])
        .output()
        .context("spawn git rev-parse")?;
    if !sha.status.success() {
        anyhow::bail!(
            "git rev-parse origin/{branch} failed: {}",
            String::from_utf8_lossy(&sha.stderr).trim()
        );
    }
    let commit = String::from_utf8_lossy(&sha.stdout).trim().to_string();
    src.last_commit_sha = Some(commit);
    src.branch = Some(branch);
    Ok(())
}

/// Query the remote's current HEAD sha WITHOUT touching the local clone.
/// Used by the update checker so "有更新" stays a passive observation — the
/// actual pull remains an explicit user action. Returns None when the ref
/// can't be resolved (bad URL, offline, auth).
pub fn ls_remote_head(src: &Source) -> Result<Option<String>> {
    let branch = src.branch.clone().unwrap_or_else(|| "HEAD".to_string());
    let mut cmd = git_command(Path::new("."));
    cmd.args(["ls-remote", &src.location, &branch]);
    let out = run_git_output_with_timeout(cmd)?;
    let first = out.lines().next().unwrap_or("").trim();
    let sha = first.split_whitespace().next().unwrap_or("");
    if sha.len() >= 7 {
        Ok(Some(sha.to_string()))
    } else {
        Ok(None)
    }
}

pub(crate) fn head_commit(path: &Path) -> Result<String> {
    let out = git_command(path)
        .args(["rev-parse", "HEAD"])
        .output()
        .context("spawn git rev-parse HEAD")?;
    if !out.status.success() {
        anyhow::bail!(
            "git rev-parse HEAD failed: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        );
    }
    Ok(String::from_utf8_lossy(&out.stdout).trim().to_string())
}

fn current_branch(path: &Path) -> Option<String> {
    let out = git_command(path)
        .args(["rev-parse", "--abbrev-ref", "HEAD"])
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    let b = String::from_utf8_lossy(&out.stdout).trim().to_string();
    if b.is_empty() || b == "HEAD" {
        None
    } else {
        Some(b)
    }
}

/// Shallow-clone a repo into `dest` using the system `git` binary. Returns
/// the resolved branch name. On failure `dest` is removed so callers can
/// retry cleanly. The child is killed if it exceeds [`GIT_TIMEOUT`].
///
/// `GIT_TERMINAL_PROMPT=0` disables interactive credential prompts — a
/// private repo with no cached credential simply fails fast instead of
/// hanging forever waiting on a tty the user will never see.
pub(crate) fn clone_repo(url: &str, dest: &Path, branch: Option<&str>) -> Result<String> {
    let mut cmd = git_command(dest.parent().unwrap_or(Path::new(".")));
    cmd.arg("clone");
    cmd.args(["--depth", "1"]);
    cmd.arg("--single-branch");
    if let Some(b) = branch {
        cmd.args(["--branch", b]);
    }
    cmd.args([url, &dest.to_string_lossy()]);

    let outcome = run_git_with_timeout(cmd);
    if let Err(e) = outcome {
        // Clean up the partial clone so a retry starts fresh.
        let _ = std::fs::remove_dir_all(dest);
        return Err(e).context("git clone failed");
    }

    let resolved = current_branch(dest).unwrap_or_else(|| branch.unwrap_or("main").to_string());
    Ok(resolved)
}

/// Build a `Command` rooted at `cwd` pre-configured with the env vars we want
/// on every git invocation: a non-interactive prompt, a sane user-agent, and
/// a predictable locale so we can parse English status words.
pub(crate) fn git_command(cwd: &Path) -> Command {
    let mut cmd = Command::new("git");
    cmd.current_dir(cwd);
    cmd.env("GIT_TERMINAL_PROMPT", "0");
    cmd.env("GIT_HTTP_USER_AGENT", "skill-dock/1.0");
    cmd.env("LC_ALL", "C");
    cmd
}

/// Spawn a git `Command` and wait with a hard timeout, returning trimmed
/// stdout on success. Stderr feeds the error message on failure.
pub(crate) fn run_git_output_with_timeout(mut cmd: Command) -> Result<String> {
    let mut child = cmd
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .context("failed to spawn git")?;

    let deadline = std::time::Instant::now() + GIT_TIMEOUT;
    loop {
        match child.try_wait().context("polling git child")? {
            Some(status) => {
                let mut out = String::new();
                let mut err_text = String::new();
                if let Some(mut s) = child.stdout.take() {
                    use std::io::Read;
                    let _ = s.read_to_string(&mut out);
                }
                if let Some(s) = child.stderr.as_mut() {
                    use std::io::Read;
                    let _ = s.read_to_string(&mut err_text);
                }
                if !status.success() {
                    let trimmed = err_text.trim();
                    anyhow::bail!(if trimmed.is_empty() {
                        format!("git exited with status {status}")
                    } else {
                        format!("git exited with status {status}: {trimmed}")
                    });
                }
                return Ok(out.trim().to_string());
            }
            None => {
                if std::time::Instant::now() >= deadline {
                    let _ = child.kill();
                    let _ = child.wait();
                    anyhow::bail!("git timed out after {}s", GIT_TIMEOUT.as_secs());
                }
                std::thread::sleep(Duration::from_millis(200));
            }
        }
    }
}

/// Spawn a git `Command` and wait for it with a hard timeout. Stderr is
/// captured into the error message on failure so the UI can show the real
/// reason (auth, host not found, etc.). On timeout the child is killed and
/// reaped so we don't leak a runaway clone or a zombie.
///
/// Implementation note: std's `Child::wait` is blocking with no timeout, so
/// we poll the child with `try_wait` on a short sleep loop. When the deadline
/// passes we `child.kill()` (sends SIGKILL) and then `wait()` to reap.
fn run_git_with_timeout(mut cmd: Command) -> Result<()> {
    let mut child = cmd
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .context("failed to spawn git")?;

    let deadline = std::time::Instant::now() + GIT_TIMEOUT;
    let mut status: Option<std::process::ExitStatus> = None;
    while std::time::Instant::now() < deadline {
        match child.try_wait().context("polling git child")? {
            Some(s) => {
                status = Some(s);
                break;
            }
            None => std::thread::sleep(Duration::from_millis(200)),
        }
    }

    let status = match status {
        Some(s) => s,
        None => {
            // Timed out — kill the still-running child and reap it.
            let _ = child.kill();
            let _ = child.wait();
            anyhow::bail!("git timed out after {}s", GIT_TIMEOUT.as_secs());
        }
    };

    // Drain stderr for a useful error message.
    let mut err_text = String::new();
    if let Some(s) = child.stderr.as_mut() {
        use std::io::Read;
        let _ = s.read_to_string(&mut err_text);
    }

    if status.success() {
        Ok(())
    } else {
        let trimmed = err_text.trim();
        if trimmed.is_empty() {
            anyhow::bail!("git exited with status {status}");
        } else {
            anyhow::bail!("git exited with status {status}: {trimmed}");
        }
    }
}

pub(crate) fn expand_home(p: &str) -> String {
    if let Some(rest) = p.strip_prefix("~/") {
        if let Some(home) = dirs::home_dir() {
            return home.join(rest).to_string_lossy().to_string();
        }
    }
    p.to_string()
}

fn repo_name_from_url(url: &str) -> String {
    let trimmed = url.trim_end_matches('/').trim_end_matches(".git");
    trimmed
        .rsplit('/')
        .next()
        .unwrap_or(trimmed)
        .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::PersistedState;

    #[test]
    fn github_url_validation_accepts_common_git_schemes() {
        // git clone 支持任意 Git 主机：http(s)/ssh/scp 形式都应放行（内网 GitLab/Gitea 场景）。
        for ok in [
            "https://github.com/anthropics/skills",
            "http://gitlab.internal/foo/bar.git",
            "ssh://git@gitlab.internal/foo/bar.git",
            "git@gitlab.internal:foo/bar.git",
            "git://example.com/repo",
        ] {
            let mut st = PersistedState::current();
            let r = add_github(&mut st, "t".into(), ok.to_string(), None, SourceMode::Flat);
            assert!(r.is_ok(), "应放行 {ok}，实际 {:?}", r.err().map(|e| e.to_string()));
        }
    }

    #[test]
    fn github_url_validation_rejects_non_git_input() {
        for bad in ["ftp://example.com/x", "/local/path", "plain-text"] {
            let mut st = PersistedState::current();
            let r = add_github(&mut st, "t".into(), bad.to_string(), None, SourceMode::Flat);
            assert!(r.is_err(), "应拒绝 {bad}");
        }
    }
}
