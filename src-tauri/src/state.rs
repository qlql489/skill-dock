//! Persistent state model and storage.

use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use parking_lot::RwLock;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;
use uuid::Uuid;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum SourceKind {
    Local,
    Github,
}

/// Lifecycle of a GitHub source's local clone. `#[serde(default)]` on the
/// field keeps old state.json files loading (treated as `Pending`, which the
/// command layer immediately advances to the correct state).
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "lowercase")]
pub enum CloneStatus {
    /// Not yet cloned / unknown (also the value for local sources, which
    /// never clone).
    #[default]
    Pending,
    /// Background clone in flight.
    Cloning,
    /// Clone present and ready to scan.
    Ready,
    /// Clone failed — see `clone_error` for details.
    Failed,
}

/// How a source's skills are presented and managed. `#[serde(default)]`
/// keeps old state.json loading as `Flat` (the pre-feature behavior).
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "lowercase")]
pub enum SourceMode {
    /// Each skill is independent: install/delete one at a time (legacy).
    #[default]
    Flat,
    /// Skills form a bundle: install/update/delete the whole group at once,
    /// with individual delete still allowed per skill.
    Grouped,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Source {
    pub id: Uuid,
    pub kind: SourceKind,
    pub name: String,
    /// For Local: the local path on disk.
    /// For Github: the remote URL (e.g. https://github.com/foo/bar).
    pub location: String,
    /// For GitHub: the local clone path under the application data `repos/<id>/` directory.
    pub clone_path: Option<PathBuf>,
    /// For Github: default branch (auto-detected on clone).
    pub branch: Option<String>,
    pub created_at: DateTime<Utc>,
    pub last_scanned_at: Option<DateTime<Utc>>,
    pub last_commit_sha: Option<String>,
    /// Number of skills found in the most recent scan.
    pub skill_count: usize,
    /// For Github: lifecycle of the background clone. Defaults to `Pending`
    /// so old state.json entries (and all local sources) load cleanly.
    #[serde(default)]
    pub clone_status: CloneStatus,
    /// For Github: last clone error message (only set when `clone_status ==
    /// Failed`). Surfaced in the UI next to the retry button.
    #[serde(default)]
    pub clone_error: Option<String>,
    /// How this source's skills are managed in the library. `Flat` = each
    /// skill independent; `Grouped` = the skills form a bundle that can be
    /// installed/updated/deleted together.
    #[serde(default)]
    pub mode: SourceMode,
    /// Target agent ids to auto-sync this source's skills into. When non-empty,
    /// a filesystem watcher monitors the source folder (local sources only);
    /// after each scan completes, all of the source's skills are symlinked
    /// into these targets. Empty = no auto-sync (legacy behavior).
    /// `#[serde(default)]` keeps old state.json files loading as empty.
    #[serde(default)]
    pub auto_sync_targets: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Skill {
    /// Stable id = source_id + ":" + relative_path
    pub id: String,
    pub source_id: Uuid,
    pub name: String,
    pub description: String,
    /// Path of the skill directory, relative to source root.
    pub relative_path: PathBuf,
    /// Resolved absolute path on disk.
    pub absolute_path: PathBuf,
    /// Whether this skill contains a SKILL.md.
    pub has_skill_md: bool,
    /// blake3 hash of SKILL.md (or empty file) for change detection.
    pub content_hash: String,
    /// Skill directory mtime.
    pub modified_at: DateTime<Utc>,
    /// Size of the skill directory in bytes.
    pub size_bytes: u64,
    /// True if the skill's real (canonicalized) location lives under the
    /// central vault `~/.agents/skills/` — i.e. multiple agents see it via
    /// symlinks back to the same source. `#[serde(default)]` keeps old
    /// state.json files loading (treated as not-shared).
    #[serde(default)]
    pub is_shared: bool,
    /// True if the skill directory itself is a symlink (as opposed to a real
    /// folder on disk). When true, `symlink_target` holds the link destination.
    #[serde(default)]
    pub is_symlink: bool,
    /// When `is_symlink` is true, the destination the symlink points to
    /// (the real file/folder location). None for non-symlinked skills.
    #[serde(default)]
    pub symlink_target: Option<std::path::PathBuf>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentTarget {
    pub id: String,
    pub name: String,
    /// Absolute path to the target's skills directory (e.g. ~/.claude/skills/).
    /// The **default** dir: new installs always land here; resettable to the
    /// builtin catalog value via reset_target_dir.
    pub skills_dir: PathBuf,
    /// Additional skill dirs this agent also reads from (user-added). Never
    /// receive installs; scanned alongside the default dir. `#[serde(default)]`
    /// keeps older state.json files loading with an empty list.
    #[serde(default)]
    pub extra_dirs: Vec<PathBuf>,
    /// Whether this target is enabled for new installs.
    pub enabled: bool,
    /// Free-form description / branding (e.g. "Anthropic Claude Code").
    pub description: String,
    /// Optional tag for grouping (e.g. "anthropic", "bytedance").
    pub tag: Option<String>,
    /// Runtime flag: true if the agent's CLI binary or home dir was detected
    /// on this machine. Recomputed on every startup / redetect — not a user
    /// preference. Surfaced in the UI so the user can tell "installed but
    /// disabled" from "not installed at all".
    #[serde(default)]
    pub detected: bool,
    /// Internal: true once the user manually toggles `enabled` from the UI.
    /// Used by the startup auto-enable pass to avoid overriding a deliberate
    /// disable (we only auto-enable targets the user has never touched).
    #[serde(default)]
    pub user_touched: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Installation {
    /// skill id
    pub skill_id: String,
    /// target id
    pub target_id: String,
    /// Actual symlink path under target.skills_dir
    pub link_path: PathBuf,
    pub installed_at: DateTime<Utc>,
    /// Health from the last reconcile(): "ok" = live symlink pointing at the
    /// skill; "missing" = entry vanished from disk; "conflict" = the name is
    /// now occupied by something that isn't our link (real dir, repointed or
    /// foreign symlink). Kept in the ledger instead of dropped so the 概览 page
    /// can surface problems; a healthy install always says "ok".
    /// `#[serde(default)]` keeps older state.json loading as "ok"-equivalent
    /// only after reconcile() re-runs — until then it reads "" which the UI
    /// treats as ok.
    #[serde(default)]
    pub status: String,
}

/// A named collection of skills ("组合") that can be applied to / removed
/// from target agents in one action. Skills are referenced by id; ids that no
/// longer resolve (source moved) are surfaced as stale members, not errors.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SkillGroup {
    pub id: Uuid,
    pub name: String,
    pub created_at: DateTime<Utc>,
    /// Skill ids in member order.
    pub skill_ids: Vec<String>,
}

/// 一个项目工作区：用户注册的项目根目录（如某个 git 仓库），用于管理
/// 其中各 agent 的项目级 skills 目录（如 <project>/.claude/skills）。
/// 技能实况永远以磁盘扫描为准，这里只存注册元数据。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Project {
    pub id: Uuid,
    pub name: String,
    /// 项目根目录（绝对路径，落库前已 canonicalize）。
    pub path: PathBuf,
    pub created_at: DateTime<Utc>,
    #[serde(default)]
    pub sort_order: i32,
}

/// UI 级偏好（主题色等外观设置）。与领域数据分开放，新增偏好项
/// 不影响领域模型；整个结构 serde default，旧 state.json 直接兼容。
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct UiPrefs {
    /// 主题色，#rrggbb 形式。空串 = 使用内置默认蓝。
    #[serde(default)]
    pub accent_color: String,
    /// 添加来源后的安装步骤里，是否默认勾选上次安装的 Agents。
    /// `default = true` 让旧 state.json 保持既有行为（预勾选）。
    #[serde(default = "default_true")]
    pub add_default_install_targets: bool,
    /// 收编默认目的地（中央仓库）的绝对路径。空串 = 使用内置默认
    /// 默认位于应用数据目录的 `skills` 子目录。写入时由后端展开 `~` 并存绝对路径。
    #[serde(default)]
    pub vault_path: String,
}

fn default_true() -> bool {
    true
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct PersistedState {
    pub sources: Vec<Source>,
    pub targets: Vec<AgentTarget>,
    pub installations: Vec<Installation>,
    /// Cached skill records keyed by skill id.
    pub skills: BTreeMap<String, Skill>,
    /// Skill ids the user "deleted" (uninstalled + hidden). The scanner
    /// skips these so they don't reappear on rescan. Cleared per-source when
    /// the source itself is removed.
    #[serde(default)]
    pub hidden_skills: BTreeSet<String>,
    /// Named skill collections (组合). `#[serde(default)]` for old state.json.
    #[serde(default)]
    pub groups: Vec<SkillGroup>,
    /// 项目工作区（项目级 skill 管理）。`#[serde(default)]` keeps old
    /// state.json files loading with an empty list.
    #[serde(default)]
    pub projects: Vec<Project>,
    /// UI 外观偏好。
    #[serde(default)]
    pub ui: UiPrefs,
    /// Schema version for migrations.
    pub schema_version: u32,
}

impl PersistedState {
    pub fn current() -> Self {
        Self {
            schema_version: 1,
            ..Default::default()
        }
    }
}

pub struct AppState {
    pub data_dir: PathBuf,
    pub state_path: PathBuf,
    pub inner: RwLock<PersistedState>,
}

impl AppState {
    pub fn load(data_dir: PathBuf) -> Result<Self> {
        let state_path = data_dir.join("state.json");
        let mut inner = if state_path.exists() {
            let bytes = std::fs::read(&state_path)
                .with_context(|| format!("read state from {}", state_path.display()))?;
            match serde_json::from_slice::<PersistedState>(&bytes) {
                Ok(s) => s,
                Err(e) => {
                    log::error!("failed to parse state.json: {e}, starting fresh");
                    PersistedState::current()
                }
            }
        } else {
            let mut s = PersistedState::current();
            s.targets = default_targets();
            s
        };

        // Backfill clone_status for sources loaded from a pre-clone-status
        // state.json (they deserialize to Pending via #[serde(default)]).
        // A github source with an existing clone dir is Ready; otherwise it's
        // a leftover from a failed/interrupted clone and gets Failed so the
        // UI offers a retry. Local sources stay Ready.
        for s in inner.sources.iter_mut() {
            if s.clone_status == CloneStatus::Pending {
                s.clone_status = match s.kind {
                    SourceKind::Local => CloneStatus::Ready,
                    SourceKind::Github => {
                        if s.clone_path.as_ref().map_or(false, |p| p.is_dir()) {
                            CloneStatus::Ready
                        } else {
                            CloneStatus::Failed
                        }
                    }
                };
            }
        }

        // Migrate the codex target: it used to be named "Codex CLI" pointing
        // at ~/.codex/skills/. It now shares the central vault ~/.agents/skills/
        // (the same dir `is_shared_skill` treats as the vault root) and is
        // renamed to just "Codex". Only rewrites targets that still hold the
        // old values, so manual user edits to skills_dir are preserved.
        if let Some(home) = dirs::home_dir() {
            let old_codex_dir = home.join(".codex/skills/");
            let new_codex_dir = home.join(".agents/skills/");
            for t in inner.targets.iter_mut() {
                if t.id == "codex" {
                    if t.name == "Codex CLI" {
                        t.name = "Codex".to_string();
                    }
                    if t.skills_dir == old_codex_dir {
                        t.skills_dir = new_codex_dir.clone();
                    }
                }
            }

            // Migrate the Pi target: early manual entries pointed at
            // ~/.pi/skills/, but the Pi coding agent reads ~/.pi/agent/skills
            // (path verified against the Pi adapter reference). Only the
            // known-wrong path is rewritten.
            let wrong_pi_dir = home.join(".pi/skills/");
            let right_pi_dir = home.join(".pi/agent/skills/");
            for t in inner.targets.iter_mut() {
                if t.id == "pi" && t.skills_dir == wrong_pi_dir {
                    t.skills_dir = right_pi_dir.clone();
                    log::info!("state: migrated Pi skills_dir to ~/.pi/agent/skills");
                }
            }
        }

        // Built-in targets are part of the application catalog rather than
        // user-created state. Backfill any that were added after this
        // state.json was written, while preserving existing targets (and any
        // user edits to their paths, names, or enabled state).
        let added_builtin_targets = ensure_builtin_targets(&mut inner.targets);
        if added_builtin_targets > 0 {
            log::info!("state: added {added_builtin_targets} missing built-in target(s)");
        }

        Ok(Self {
            data_dir,
            state_path,
            inner: RwLock::new(inner),
        })
    }

    pub fn snapshot(&self) -> PersistedState {
        self.inner.read().clone()
    }

    /// Persist current state to disk. Atomically writes to a temp file then
    /// renames. **Do not call this while holding a write lock** — the lock is
    /// non-reentrant and this method acquires a read lock via `snapshot()`,
    /// which will deadlock. Release any `state.inner.write()` guard first.
    pub fn save(&self) -> Result<()> {
        let snap = self.snapshot();
        let bytes = serde_json::to_vec_pretty(&snap)?;
        // Write to a temp file then rename for atomicity.
        let tmp = self.state_path.with_extension("json.tmp");
        std::fs::write(&tmp, &bytes)?;
        std::fs::rename(&tmp, &self.state_path)?;
        Ok(())
    }
}

/// Append built-in targets that are absent from persisted state. Matching by
/// id makes this migration idempotent and ensures a manually-added target
/// with the same id is not overwritten.
fn ensure_builtin_targets(targets: &mut Vec<AgentTarget>) -> usize {
    let mut existing_ids: BTreeSet<String> = targets.iter().map(|target| target.id.clone()).collect();
    let mut added = 0;
    for target in default_targets() {
        if existing_ids.insert(target.id.clone()) {
            targets.push(target);
            added += 1;
        }
    }
    added
}

/// Returns the default agent target list. The user can enable/disable
/// these from the UI. The `enabled` values here are just first-run seeds;
/// on every launch [`crate::detect`] probes the system and auto-enables
/// installed agents the user hasn't manually touched.
pub fn default_targets() -> Vec<AgentTarget> {
    let home = dirs::home_dir().unwrap_or_else(|| PathBuf::from("~"));
    let make = |id: &str, name: &str, sub: &str, desc: &str, tag: Option<&str>, enabled: bool| AgentTarget {
        id: id.to_string(),
        name: name.to_string(),
        skills_dir: home.join(sub.trim_start_matches("~/").trim_start_matches("/")),
        extra_dirs: Vec::new(),
        enabled,
        description: desc.to_string(),
        tag: tag.map(String::from),
        detected: false,
        user_touched: false,
    };

    vec![
        make("claude-code", "Claude Code", "~/.claude/skills/", "Anthropic Claude Code", Some("anthropic"), true),
        make("codex", "Codex", "~/.agents/skills/", "OpenAI Codex CLI", Some("openai"), true),
        // ZCode CLI 从 ~/.zcode/skills/ 加载技能（图标 public/agent-icons/zcode.svg）。
        make("zcode", "ZCode", "~/.zcode/skills/", "Z.ai ZCode CLI", Some("zai"), true),
        make("deepseek-harness", "DeepSeek Harness", "~/.dsh/skills/", "DeepSeek Harness", Some("deepseek"), false),
        make("minimax", "MiniMax Agents", "~/.minimax/agents/", "MiniMax agent root", Some("minimax"), true),
        make("minimax-builtin", "MiniMax Builtins", "~/.minimax/.builtin-skills/", "MiniMax built-in skills (read-only recommended)", Some("minimax"), false),
        make("trae", "Trae", "~/.trae/skills/", "Trae IDE", Some("bytedance"), false),
        make("codebuddy", "CodeBuddy", "~/.codebuddy/skills/", "CodeBuddy IDE", Some("tencent"), false),
        make("kiro", "Kiro", "~/.kiro/skills/", "Kiro / AWS", Some("amazon"), false),
        make("qwen", "Qwen Coder", "~/.qwen/skills/", "Qwen Coder", Some("alibaba"), false),
        make("qoder", "Qoder", "~/.qoder/skills/", "Qoder IDE", Some("qoder"), false),
        make("qoderwork", "QoderWork", "~/.qoderwork/skills/", "QoderWork", Some("qoder"), false),
        make("hanako", "Hanako", "~/.hanako/skills/", "Hanako", Some("hanako"), false),
        make("lingma", "Lingma", "~/.lingma/skills/", "Alibaba Lingma", Some("alibaba"), false),
        make("iflow", "iFlow", "~/.iflow/skills/", "iFlow", Some("iflow"), false),
        make("kode", "Kode", "~/.kode/skills/", "Kode", Some("kode"), false),
        make("continue", "Continue", "~/.continue/skills/", "Continue.dev", Some("continue"), false),
        make("openclaw", "OpenClaw", "~/.openclaw/skills/", "OpenClaw", Some("openclaw"), false),
        make("workbuddy", "WorkBuddy", "~/.workbuddy/skills/", "WorkBuddy", Some("workbuddy"), false),
        make("hermes", "Hermes", "~/.hermes/skills/", "Hermes", Some("hermes"), false),
        make("antigravity", "Antigravity", "~/.antigravity/skills/", "Antigravity", Some("antigravity"), false),
        make("kimi-code", "Kimi Code", "~/.kimi/skills/", "Kimi Code (Moonshot)", Some("moonshot"), false),
        // ---- 以下为第二批目录扩充（路径对标 skills-manager 适配器表）----
        make("cursor", "Cursor", "~/.cursor/skills/", "Cursor IDE", None, false),
        make("windsurf", "Windsurf", "~/.codeium/windsurf/skills/", "Windsurf (Codeium)", None, false),
        make("github-copilot", "GitHub Copilot", "~/.copilot/skills/", "GitHub Copilot CLI", None, false),
        make("gemini-cli", "Gemini CLI", "~/.gemini/skills/", "Google Gemini CLI", None, false),
        make("opencode", "OpenCode", "~/.config/opencode/skills/", "OpenCode", None, false),
        make("goose", "Goose", "~/.config/goose/skills/", "Block Goose", None, false),
        make("roo-code", "Roo Code", "~/.roo/skills/", "Roo Code", None, false),
        make("kilo-code", "Kilo Code", "~/.kilocode/skills/", "Kilo Code", None, false),
        make("droid", "Droid", "~/.factory/skills/", "Factory Droid", None, false),
        make("crush", "Crush", "~/.config/crush/skills/", "Charm Crush", None, false),
        make("pi", "Pi", "~/.pi/agent/skills/", "Pi Coding Agent", None, false),
        make("neovate", "Neovate", "~/.neovate/skills/", "Neovate Code", None, false),
        make("openhands", "OpenHands", "~/.openhands/skills/", "OpenHands", None, false),
        make("grok", "Grok CLI", "~/.grok/skills/", "xAI Grok CLI", None, false),
        make("augment", "Augment", "~/.augment/skills/", "Augment Code", None, false),
        make("junie", "Junie", "~/.junie/skills/", "JetBrains Junie", None, false),
        make("deepagents", "DeepAgents", "~/.deepagents/agent/skills/", "LangChain DeepAgents", None, false),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::time::{SystemTime, UNIX_EPOCH};

    #[test]
    fn default_targets_include_deepseek_harness() {
        let target = default_targets()
            .into_iter()
            .find(|target| target.id == "deepseek-harness")
            .expect("DeepSeek Harness should be a built-in target");
        let home = dirs::home_dir().expect("test home directory");
        assert_eq!(target.name, "DeepSeek Harness");
        assert_eq!(target.skills_dir, home.join(".dsh/skills"));
        assert!(!target.detected);
    }

    #[test]
    fn default_targets_include_zcode() {
        let target = default_targets()
            .into_iter()
            .find(|target| target.id == "zcode")
            .expect("ZCode should be a built-in target");
        let home = dirs::home_dir().expect("test home directory");
        assert_eq!(target.name, "ZCode");
        assert_eq!(target.skills_dir, home.join(".zcode/skills"));
    }

    #[test]
    fn load_backfills_missing_builtin_targets_without_overwriting_custom_state() {
        let root = unique_test_dir();
        let custom = AgentTarget {
            id: "custom-agent".into(),
            name: "Custom Agent".into(),
            skills_dir: root.join("custom-skills"),
            extra_dirs: Vec::new(),
            enabled: false,
            description: "user target".into(),
            tag: Some("custom".into()),
            detected: true,
            user_touched: true,
        };
        let mut persisted = PersistedState::current();
        persisted.targets.push(custom.clone());
        fs::write(
            root.join("state.json"),
            serde_json::to_vec(&persisted).unwrap(),
        )
        .unwrap();

        let loaded = AppState::load(root.clone()).unwrap();
        let snapshot = loaded.snapshot();
        let deepseek = snapshot
            .targets
            .iter()
            .find(|target| target.id == "deepseek-harness")
            .expect("missing built-in target should be backfilled");
        assert_eq!(deepseek.skills_dir, dirs::home_dir().unwrap().join(".dsh/skills"));
        assert_eq!(snapshot.targets.iter().filter(|target| target.id == "deepseek-harness").count(), 1);
        let loaded_custom = snapshot
            .targets
            .iter()
            .find(|target| target.id == custom.id)
            .expect("custom target should be preserved");
        assert_eq!(loaded_custom.name, custom.name);
        assert_eq!(loaded_custom.skills_dir, custom.skills_dir);
        assert_eq!(loaded_custom.enabled, custom.enabled);
        assert_eq!(loaded_custom.user_touched, custom.user_touched);

        fs::remove_dir_all(root).unwrap();
    }

    fn unique_test_dir() -> PathBuf {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let path = std::env::temp_dir().join(format!(
            "skill-dock-state-{nanos}-{}",
            std::process::id()
        ));
        fs::create_dir_all(&path).unwrap();
        path
    }
}
